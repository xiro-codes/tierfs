//! Coordinating tiering engine and IPC server using SQLite.

use crate::config::{Config, ConfigOverrides};
use crate::metadata::Metadata;
use crate::popularity::{calculate_popularity, WEEK};
use crate::tier::{File, Tier};
use crate::tools::{AdHoc, Command};

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{chown, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

/// Coordinates the SQLite database, Unix IPC sockets, sleeping, and tiering logic.
pub struct TierEngine {
    pub config: Config,
    pub tiers: Arc<Mutex<Vec<Tier>>>,
    pub run_path: PathBuf,
    pub mount_point: Arc<Mutex<PathBuf>>,
    pub db_path: PathBuf,
    pub db: Arc<Mutex<rusqlite::Connection>>,
    stop_flag: Arc<AtomicBool>,
    currently_tiering: Arc<AtomicBool>,
    last_tier_time: Arc<Mutex<SystemTime>>,
    adhoc_work: Arc<Mutex<Vec<AdHoc>>>,
    sleep_cv: Arc<Condvar>,
    sleep_mutex: Arc<Mutex<bool>>,
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
            if let Ok(group) = nix::unistd::Group::from_name("tierfs") {
                if let Some(grp) = group {
                    let _ = chown(&config.run_path, None, Some(grp.gid.as_raw()));
                }
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
        ).map_err(|e| format!("Failed to initialize SQLite table: {}", e))?;

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
                while daemon_mode && SystemTime::now() < wake_time && !self.stop_flag.load(Ordering::Relaxed) {
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
        if self.currently_tiering.swap(true, Ordering::SeqCst) {
            return false;
        }

        log::debug!("Gathering files.");
        let mut candidate_files = Vec::new();
        let mut tiers = match self.tiers.lock() {
            Ok(t) => t,
            Err(_) => {
                self.currently_tiering.store(false, Ordering::SeqCst);
                return false;
            }
        };

        // Crawl files in all tiers
        for tier in tiers.iter_mut() {
            let mut usage = 0;
            self.crawl(&tier.path, tier, &mut candidate_files, &mut usage);
            if let Ok(mut u) = tier.usage_bytes.lock() {
                *u = usage;
            }
        }

        // Calculate popularity
        self.calc_popularity(&mut candidate_files);

        // Sort files
        self.sort(&mut candidate_files);

        // Simulate
        self.simulate_tier(&mut candidate_files, &mut tiers);

        // Transfer files
        self.move_files(&mut tiers);

        // Update database
        if let Ok(db_conn) = self.db.lock() {
            for file in &candidate_files {
                let _ = file.metadata.update(&db_conn, &file.relative_path.to_string_lossy(), None);
            }
        }

        self.currently_tiering.store(false, Ordering::SeqCst);
        true
    }

    /// Crawls a tier path recursively, building candidate files.
    pub fn crawl(&self, dir: &Path, tier: &Tier, files: &mut Vec<File>, usage: &mut u64) {
        if !dir.is_dir() {
            return;
        }
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                if metadata.is_dir() {
                    self.crawl(&path, tier, files, usage);
                } else if !metadata.is_symlink() {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    if filename.starts_with('.') && filename.ends_with(".tierfs.hide") {
                        continue;
                    }

                    // Get relative path to tier
                    let rel_path = path.strip_prefix(&tier.path).unwrap_or(&path).to_path_buf();
                    let file_size = metadata.len();
                    *usage += file_size;

                    // Retrieve or create DB metadata
                    let db_meta = if let Ok(db_conn) = self.db.lock() {
                        Metadata::from_db(&db_conn, &rel_path.to_string_lossy(), Some(&tier.path.to_string_lossy()))
                    } else {
                        Metadata::default()
                    };

                    if !db_meta.pinned {
                        files.push(File::new(rel_path, tier.id.clone(), file_size, db_meta));
                    }
                }
            }
        }
    }

    /// Updates candidate popularity based on elapsed time.
    fn calc_popularity(&self, files: &mut [File]) {
        let mut last_time = self.last_tier_time.lock().unwrap();
        let now = SystemTime::now();
        let elapsed = now.duration_since(*last_time).unwrap_or(Duration::from_secs(1));
        *last_time = now;

        let period_secs = elapsed.as_secs_f64();
        for file in files.iter_mut() {
            // Touch access counts or calc EMA
            let current_pop = file.metadata.popularity;
            let accesses = file.metadata.access_count;
            file.metadata.access_count = 0; // reset
            file.metadata.popularity = calculate_popularity(current_pop, accesses, period_secs, WEEK); // mock week age
        }
    }

    /// Sorts candidates by popularity desc, then atime desc.
    fn sort(&self, files: &mut [File]) {
        files.sort_by(|a, b| {
            let pop_a = a.metadata.popularity;
            let pop_b = b.metadata.popularity;
            if (pop_a - pop_b).abs() < 1e-9 {
                b.atime.cmp(&a.atime)
            } else {
                pop_b.partial_cmp(&pop_a).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
    }

    /// Place files into target tiers based on quota limits.
    fn simulate_tier(&self, files: &mut [File], tiers: &mut [Tier]) {
        for tier in tiers.iter_mut() {
            tier.sim_usage_bytes = 0;
            tier.incoming_files.clear();
        }

        for file in files {
            let mut fitted = false;
            for tier in tiers.iter_mut() {
                if !tier.full_test(file.size) {
                    tier.sim_usage_bytes += file.size;
                    let target_tier_id = tier.id.clone();
                    let target_tier_path = tier.path.to_string_lossy().into_owned();
                    
                    if file.tier_id != target_tier_id {
                        let mut enqueued_file = file.clone();
                        // Keep enqueued_file.metadata.tier_path as the old path (source)
                        enqueued_file.tier_id = target_tier_id.clone();
                        tier.incoming_files.push(enqueued_file);
                    }
                    
                    file.metadata.tier_path = target_tier_path;
                    file.tier_id = target_tier_id;
                    fitted = true;
                    break;
                }
            }
            if !fitted {
                log::error!("Could not fit file in any tiers: {:?}", file.relative_path);
            }
        }
    }

    /// Spawns parallel worker threads to execute file transfers.
    fn move_files(&self, tiers: &mut [Tier]) {
        thread::scope(|s| {
            for tier in tiers.iter_mut() {
                let buff_sz = self.config.copy_buff_sz;
                let run_path = &self.config.run_path;
                let db_path = &self.db_path;
                s.spawn(move || {
                    tier.transfer_files(buff_sz, run_path, db_path);
                });
            }
        });
    }

    /// IPC listener socket.
    pub fn process_adhoc_requests(&self) {
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
        if let Ok(group) = nix::unistd::Group::from_name("tierfs") {
            if let Some(grp) = group {
                let _ = chown(&socket_path, None, Some(grp.gid.as_raw()));
            }
        }
        let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o775));

        while !self.stop_flag.load(Ordering::Relaxed) {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = vec![0; 65536];
                if let Ok(n) = stream.read(&mut buffer) {
                    if n > 0 {
                        let payload_str = String::from_utf8_lossy(&buffer[..n]);
                        if let Ok(payload) = serde_json::from_str::<Vec<String>>(&payload_str) {
                            if let Some(work) = AdHoc::from_payload(&payload) {
                                let mut response = Vec::new();
                                self.handle_adhoc_cmd(work, &mut response);
                                let response_str = serde_json::to_string(&response).unwrap_or_default();
                                let _ = stream.write_all(response_str.as_bytes());
                            }
                        }
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
                if let Ok(db_conn) = self.db.lock() {
                    if let Ok(mut stmt) = db_conn.prepare("SELECT relative_path, tier_path FROM metadata WHERE pinned = 1") {
                        if let Ok(rows) = stmt.query_map([], |row| {
                            let rel: String = row.get(0)?;
                            let tp: String = row.get(1)?;
                            Ok(format!("{} : {}\n", rel, tp))
                        }) {
                            for r in rows.flatten() {
                                pins.push_str(&r);
                            }
                        }
                    }
                }
                response.push(pins);
            }
            Command::LPop => {
                response.push("OK".to_string());
                let mut pops = String::new();
                if let Ok(db_conn) = self.db.lock() {
                    if let Ok(mut stmt) = db_conn.prepare("SELECT relative_path, popularity FROM metadata") {
                        if let Ok(rows) = stmt.query_map([], |row| {
                            let rel: String = row.get(0)?;
                            let pop: f64 = row.get(1)?;
                            Ok(format!("{} : {}\n", rel, pop))
                        }) {
                            for r in rows.flatten() {
                                pops.push_str(&r);
                            }
                        }
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
                let rel_path = full_path.strip_prefix(&mp).unwrap_or(&full_path).to_path_buf();
                let rel_str = rel_path.to_string_lossy();

                let mut meta = Metadata::from_db(&db_conn, &rel_str, None);
                meta.pinned = true;

                // Move the file to the target tier immediately
                let old_path = PathBuf::from(&meta.tier_path).join(&rel_path);
                let new_path = target_tier.path.join(&rel_path);
                let mut conflicted = false;

                if old_path != new_path && target_tier.move_file(&old_path, &new_path, self.config.copy_buff_sz, &mut conflicted, &target_tier.id) {
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
                let rel_path = full_path.strip_prefix(&mp).unwrap_or(&full_path).to_path_buf();
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
