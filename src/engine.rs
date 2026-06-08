//! Coordinating tiering engine and IPC server using SQLite.

use crate::config::{Config, ConfigOverrides};
use crate::metadata::Metadata;
use crate::strategy::{PopularityTieringStrategy, TieringStrategy};
use crate::tier::Tier;
use crate::tools::{AdHoc, Command};

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, chown};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

/// Coordinates the SQLite database, Unix IPC sockets, sleeping, and tiering logic.
pub struct TierEngine {
    pub config: Config,
    pub tiers: Arc<Mutex<Vec<Tier>>>,
    pub run_path: PathBuf,
    pub mount_point: Arc<Mutex<PathBuf>>,
    pub db_path: PathBuf,
    pub db: Arc<Mutex<rusqlite::Connection>>,
    pub stop_flag: Arc<AtomicBool>,
    pub currently_tiering: Arc<AtomicBool>,
    pub last_tier_time: Arc<Mutex<SystemTime>>,
    adhoc_work: Arc<Mutex<Vec<AdHoc>>>,
    sleep_cv: Arc<Condvar>,
    sleep_mutex: Arc<Mutex<bool>>,
    pub strategy: Box<dyn TieringStrategy>,
}

impl TierEngine {
    /// Constructs a new TierEngine, initializing SQLite.
    pub fn new(config_path: &Path, overrides: &ConfigOverrides) -> Result<Self, String> {
        let (config, tiers) = Config::load(config_path, overrides)?;

        // Ensure run path exists
        if !config.run_path.exists() {
            fs::create_dir_all(&config.run_path)
                .map_err(|e| format!("Failed to create run path: {}", e))?;
            // Chown run_path to root:tierfs if possible
            if let Ok(group) = nix::unistd::Group::from_name("tierfs")
                && let Some(grp) = group
            {
                let _ = chown(&config.run_path, None, Some(grp.gid.as_raw()));
            }
            let _ = fs::set_permissions(&config.run_path, fs::Permissions::from_mode(0o775));
        }

        // Open SQLite database
        let db_path = config.run_path.join("metadata.db");
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| format!("Failed to open SQLite DB: {}", e))?;

        // Initialize table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS metadata (
                relative_path TEXT PRIMARY KEY,
                access_count INTEGER NOT NULL DEFAULT 0,
                popularity REAL NOT NULL DEFAULT 0.0,
                pinned INTEGER NOT NULL DEFAULT 0,
                tier_path TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| format!("Failed to initialize SQLite table: {}", e))?;

        Ok(Self {
            config,
            tiers: Arc::new(Mutex::new(tiers)),
            run_path: config_path.parent().unwrap_or(Path::new("")).to_path_buf(),
            mount_point: Arc::new(Mutex::new(PathBuf::new())),
            db_path,
            db: Arc::new(Mutex::new(conn)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            currently_tiering: Arc::new(AtomicBool::new(false)),
            last_tier_time: Arc::new(Mutex::new(SystemTime::now())),
            adhoc_work: Arc::new(Mutex::new(Vec::new())),
            sleep_cv: Arc::new(Condvar::new()),
            sleep_mutex: Arc::new(Mutex::new(false)),
            strategy: Box::new(PopularityTieringStrategy),
        })
    }

    /// Sets the mountpoint path.
    pub fn set_mount_point(&self, mount_point: PathBuf) {
        if let Ok(mut mp) = self.mount_point.lock() {
            *mp = mount_point;
        }
    }

    /// Primary daemon loop.
    pub fn begin(&self, daemon_mode: bool) {
        log::info!("tierfs started.");
        if self.config.tier_period_s.as_secs() == 0 {
            *self.last_tier_time.lock().unwrap() = SystemTime::now();
            while daemon_mode && !self.stop_flag.load(Ordering::Relaxed) {
                self.execute_queued_work();
                self.sleep_until_woken();
            }
        } else {
            *self.last_tier_time.lock().unwrap() = SystemTime::now() - self.config.tier_period_s;
            loop {
                let wake_time = SystemTime::now() + self.config.tier_period_s;
                if !self.tier() {
                    log::debug!("tierfs already moving files.");
                }
                while daemon_mode
                    && SystemTime::now() < wake_time
                    && !self.stop_flag.load(Ordering::Relaxed)
                {
                    self.execute_queued_work();
                    self.sleep_until(wake_time);
                }
                if !daemon_mode || self.stop_flag.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    }

    /// Executes a single tiering batch.
    pub fn tier(&self) -> bool {
        self.strategy.execute(self)
    }

    /// IPC listener socket.
    pub fn process_adhoc_requests(&self) {
        log::trace!("process_adhoc_requests: starting listener");
        let socket_path = self.config.run_path.join("adhoc.socket");
        let _ = fs::remove_file(&socket_path);

        let listener = match UnixListener::bind(&socket_path) {
            Ok(l) => l,
            Err(e) => {
                log::error!("Failed to bind Unix socket: {}", e);
                return;
            }
        };

        // Chown socket to tierfs group
        if let Ok(group) = nix::unistd::Group::from_name("tierfs")
            && let Some(grp) = group
        {
            let _ = chown(&socket_path, None, Some(grp.gid.as_raw()));
        }
        let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o775));

        while !self.stop_flag.load(Ordering::Relaxed) {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = vec![0; 65536];
                if let Ok(n) = stream.read(&mut buffer)
                    && n > 0
                {
                    let payload_str = String::from_utf8_lossy(&buffer[..n]);
                    if let Ok(payload) = serde_json::from_str::<Vec<String>>(&payload_str)
                        && let Some(work) = AdHoc::from_payload(&payload)
                    {
                        let mut response = Vec::new();
                        self.handle_adhoc_cmd(work, &mut response);
                        let response_str = serde_json::to_string(&response).unwrap_or_default();
                        let _ = stream.write_all(response_str.as_bytes());
                    }
                }
            }
        }
    }

    /// Process ad-hoc IPC commands.
    fn handle_adhoc_cmd(&self, work: AdHoc, response: &mut Vec<String>) {
        match work.cmd {
            Command::OneShot => {
                if self.currently_tiering.load(Ordering::Relaxed) {
                    response.push("ERR".to_string());
                    response.push("tierfs already tiering.".to_string());
                } else {
                    self.enqueue_work(work);
                    response.push("OK".to_string());
                    response.push("Work queued.".to_string());
                }
            }
            Command::Pin | Command::Unpin => {
                self.enqueue_work(work);
                response.push("OK".to_string());
                response.push("Work queued.".to_string());
            }
            Command::Status => {
                response.push("OK".to_string());
                // Simple status dump
                let tiers = self.tiers.lock().unwrap();
                let combined_status = self.config.dump(&tiers);
                response.push(combined_status);
            }
            Command::Config => {
                response.push("OK".to_string());
                let tiers = self.tiers.lock().unwrap();
                response.push(self.config.dump(&tiers));
            }
            Command::LPin => {
                response.push("OK".to_string());
                let mut pins = String::new();
                if let Ok(db_conn) = self.db.lock()
                    && let Ok(mut stmt) = db_conn
                        .prepare("SELECT relative_path, tier_path FROM metadata WHERE pinned = 1")
                    && let Ok(rows) = stmt.query_map([], |row| {
                        let rel: String = row.get(0)?;
                        let tp: String = row.get(1)?;
                        Ok(format!("{} : {}\n", rel, tp))
                    })
                {
                    for r in rows.flatten() {
                        pins.push_str(&r);
                    }
                }
                response.push(pins);
            }
            Command::LPop => {
                response.push("OK".to_string());
                let mut pops = String::new();
                if let Ok(db_conn) = self.db.lock()
                    && let Ok(mut stmt) =
                        db_conn.prepare("SELECT relative_path, popularity FROM metadata")
                    && let Ok(rows) = stmt.query_map([], |row| {
                        let rel: String = row.get(0)?;
                        let pop: f64 = row.get(1)?;
                        Ok(format!("{} : {}\n", rel, pop))
                    })
                {
                    for r in rows.flatten() {
                        pops.push_str(&r);
                    }
                }
                response.push(pops);
            }
            Command::WhichTier => {
                response.push("OK".to_string());
                let mut result = String::new();
                if let Ok(db_conn) = self.db.lock() {
                    for arg in &work.args {
                        let meta = Metadata::from_db(&db_conn, arg, None);
                        if meta.not_found {
                            result.push_str(&format!("{} : not found\n", arg));
                        } else {
                            result.push_str(&format!("{} : {}\n", arg, meta.tier_path));
                        }
                    }
                }
                response.push(result);
            }
            _ => {
                response.push("ERR".to_string());
                response.push("Unknown or unimplemented command.".to_string());
            }
        }
    }

    fn enqueue_work(&self, work: AdHoc) {
        if let Ok(mut queue) = self.adhoc_work.lock() {
            queue.push(work);
        }
        let mut lock = self.sleep_mutex.lock().unwrap();
        *lock = true;
        self.sleep_cv.notify_one();
    }

    fn execute_queued_work(&self) {
        log::trace!("execute_queued_work: checking for adhoc tasks");
        let work_items = {
            let mut queue = self.adhoc_work.lock().unwrap();
            std::mem::take(&mut *queue)
        };

        for work in work_items {
            match work.cmd {
                Command::OneShot => {
                    self.tier();
                }
                Command::Pin => {
                    self.pin_files(&work.args);
                }
                Command::Unpin => {
                    self.unpin_files(&work.args);
                }
                _ => {}
            }
        }
    }

    fn pin_files(&self, args: &[String]) {
        if args.is_empty() {
            return;
        }
        let tier_name = &args[0];
        let file_paths = &args[1..];

        let tiers = self.tiers.lock().unwrap();
        let target_tier = match tiers.iter().find(|t| t.id == *tier_name) {
            Some(t) => t,
            None => return,
        };

        let mp = self.mount_point.lock().unwrap().clone();

        if let Ok(db_conn) = self.db.lock() {
            for path_str in file_paths {
                let full_path = PathBuf::from(path_str);
                let rel_path = full_path
                    .strip_prefix(&mp)
                    .unwrap_or(&full_path)
                    .to_path_buf();
                let rel_str = rel_path.to_string_lossy();

                let mut meta = Metadata::from_db(&db_conn, &rel_str, None);
                meta.pinned = true;

                // Move the file to the target tier immediately
                let old_path = PathBuf::from(&meta.tier_path).join(&rel_path);
                let new_path = target_tier.path.join(&rel_path);
                let mut conflicted = false;

                if old_path != new_path
                    && target_tier.move_file(
                        &old_path,
                        &new_path,
                        self.config.copy_buff_sz,
                        &mut conflicted,
                        &target_tier.id,
                    )
                {
                    meta.tier_path = target_tier.path.to_string_lossy().to_string();
                }

                let _ = meta.update(&db_conn, &rel_str, None);
            }
        }
    }

    fn unpin_files(&self, args: &[String]) {
        let mp = self.mount_point.lock().unwrap().clone();
        if let Ok(db_conn) = self.db.lock() {
            for path_str in args {
                let full_path = PathBuf::from(path_str);
                let rel_path = full_path
                    .strip_prefix(&mp)
                    .unwrap_or(&full_path)
                    .to_path_buf();
                let rel_str = rel_path.to_string_lossy();

                let mut meta = Metadata::from_db(&db_conn, &rel_str, None);
                meta.pinned = false;
                let _ = meta.update(&db_conn, &rel_str, None);
            }
        }
    }

    fn sleep_until_woken(&self) {
        let lock = self.sleep_mutex.lock().unwrap();
        let _unused = self.sleep_cv.wait(lock).unwrap();
    }

    fn sleep_until(&self, wake_time: SystemTime) {
        let lock = self.sleep_mutex.lock().unwrap();
        if let Ok(duration) = wake_time.duration_since(SystemTime::now()) {
            let _unused = self.sleep_cv.wait_timeout(lock, duration).unwrap();
        }
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        let mut lock = self.sleep_mutex.lock().unwrap();
        *lock = true;
        self.sleep_cv.notify_one();
    }
}
