//! Track open files and manage FUSE private state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, LazyLock};

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

/// FUSE Private data structure, matching `FusePriv`.
pub struct FusePriv {
    pub config_path: PathBuf,
    pub mount_point: PathBuf,
    pub db: Mutex<rusqlite::Connection>,
    fd_to_path: Mutex<HashMap<u64, String>>,
    size_at_open: Mutex<HashMap<u64, u64>>,
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
        Self {
            config_path,
            mount_point,
            db: Mutex::new(db),
            fd_to_path: Mutex::new(HashMap::new()),
            size_at_open: Mutex::new(HashMap::new()),
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
}
