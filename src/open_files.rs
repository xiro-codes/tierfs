//! Track open files and manage FUSE private state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

// Global thread-safe registry for open files, matching the C++ OpenFiles namespace.
static OPEN_FILES: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn register_open_file(path: &str) {
    if let Ok(mut lock) = OPEN_FILES.lock() {
        lock.insert(path.to_string());
    }
}

pub fn release_open_file(path: &str) {
    if let Ok(mut lock) = OPEN_FILES.lock() {
        lock.remove(path);
    }
}

pub fn is_open(path: &str) -> bool {
    if let Ok(lock) = OPEN_FILES.lock() {
        lock.contains(path)
    } else {
        false
    }
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: std::ffi::OsString,
    pub ino: u64,
    pub kind: fuser::FileType,
}

/// FUSE Private data structure, matching `FusePriv`.
pub struct FusePriv {
    pub config_path: PathBuf,
    pub mount_point: PathBuf,
    pub db: Mutex<rusqlite::Connection>,
    fd_to_path: Mutex<HashMap<u64, String>>,
    size_at_open: Mutex<HashMap<u64, u64>>,
    ino_to_path: Mutex<HashMap<u64, PathBuf>>,
    path_to_ino: Mutex<HashMap<PathBuf, u64>>,
    next_ino: std::sync::atomic::AtomicU64,
    dir_handles: Mutex<HashMap<u64, Vec<DirEntry>>>,
}

impl std::fmt::Debug for FusePriv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FusePriv")
            .field("config_path", &self.config_path)
            .field("mount_point", &self.mount_point)
            .finish()
    }
}

impl FusePriv {
    pub fn new(config_path: PathBuf, mount_point: PathBuf, db: rusqlite::Connection) -> Self {
        let mut ino_to_path = HashMap::new();
        let mut path_to_ino = HashMap::new();
        // Root inode is always 1
        ino_to_path.insert(1, PathBuf::from(""));
        path_to_ino.insert(PathBuf::from(""), 1);

        Self {
            config_path,
            mount_point,
            db: Mutex::new(db),
            fd_to_path: Mutex::new(HashMap::new()),
            size_at_open: Mutex::new(HashMap::new()),
            ino_to_path: Mutex::new(ino_to_path),
            path_to_ino: Mutex::new(path_to_ino),
            next_ino: std::sync::atomic::AtomicU64::new(2),
            dir_handles: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_ino(&self, relative_path: &std::path::Path) -> u64 {
        let mut path_map = self.path_to_ino.lock().unwrap();
        if let Some(&ino) = path_map.get(relative_path) {
            return ino;
        }
        let ino = self
            .next_ino
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        path_map.insert(relative_path.to_path_buf(), ino);
        let mut ino_map = self.ino_to_path.lock().unwrap();
        ino_map.insert(ino, relative_path.to_path_buf());
        ino
    }

    pub fn get_path(&self, ino: u64) -> Option<PathBuf> {
        let ino_map = self.ino_to_path.lock().unwrap();
        ino_map.get(&ino).cloned()
    }

    pub fn remove_ino(&self, relative_path: &std::path::Path) {
        let mut path_map = self.path_to_ino.lock().unwrap();
        if let Some(ino) = path_map.remove(relative_path) {
            let mut ino_map = self.ino_to_path.lock().unwrap();
            ino_map.remove(&ino);
        }
    }

    pub fn rename_path(&self, old_path: &std::path::Path, new_path: &std::path::Path) {
        let mut path_map = self.path_to_ino.lock().unwrap();
        let mut ino_map = self.ino_to_path.lock().unwrap();

        let to_update: Vec<(PathBuf, PathBuf, u64)> = path_map
            .iter()
            .filter(|(p, _)| p.starts_with(old_path))
            .map(|(p, &ino)| {
                let suffix = p.strip_prefix(old_path).unwrap();
                let updated = new_path.join(suffix);
                (p.clone(), updated, ino)
            })
            .collect();

        for (old, new, ino) in to_update {
            path_map.remove(&old);
            path_map.insert(new.clone(), ino);
            ino_map.insert(ino, new);
        }
    }

    pub fn insert_fd_to_path(&self, fd: u64, path: String) {
        if let Ok(mut map) = self.fd_to_path.lock() {
            map.insert(fd, path);
        }
    }

    pub fn remove_fd_to_path(&self, fd: u64) {
        if let Ok(mut map) = self.fd_to_path.lock() {
            map.remove(&fd);
        }
    }

    pub fn fd_to_path(&self, fd: u64) -> Option<String> {
        if let Ok(map) = self.fd_to_path.lock() {
            map.get(&fd).cloned()
        } else {
            None
        }
    }

    pub fn insert_size_at_open(&self, fd: u64, size: u64) {
        if let Ok(mut map) = self.size_at_open.lock() {
            map.insert(fd, size);
        }
    }

    pub fn remove_size_at_open(&self, fd: u64) {
        if let Ok(mut map) = self.size_at_open.lock() {
            map.remove(&fd);
        }
    }

    pub fn size_at_open(&self, fd: u64) -> Option<u64> {
        if let Ok(map) = self.size_at_open.lock() {
            map.get(&fd).cloned()
        } else {
            None
        }
    }

    pub fn insert_dir_handle(&self, fh: u64, entries: Vec<DirEntry>) {
        if let Ok(mut map) = self.dir_handles.lock() {
            map.insert(fh, entries);
        }
    }

    pub fn remove_dir_handle(&self, fh: u64) {
        if let Ok(mut map) = self.dir_handles.lock() {
            map.remove(&fh);
        }
    }

    pub fn get_dir_entries(&self, fh: u64) -> Option<Vec<DirEntry>> {
        if let Ok(map) = self.dir_handles.lock() {
            map.get(&fh).cloned()
        } else {
            None
        }
    }
}
