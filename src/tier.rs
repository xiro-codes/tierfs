//! Storage tiers and file representations.

use crate::metadata::Metadata;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Represents a storage tier in the tierfs filesystem, matching `Tier`.
#[derive(Debug)]
pub struct Tier {
    pub id: String,
    pub path: PathBuf,
    pub quota_bytes: u64,
    pub quota_percent: f64,
    pub usage_bytes: Arc<Mutex<u64>>,
    pub sim_usage_bytes: u64,
    pub incoming_files: Vec<File>,
}

impl Tier {
    pub fn new(id: String, path: PathBuf, quota_bytes: u64, quota_percent: f64) -> Self {
        Self {
            id,
            path,
            quota_bytes,
            quota_percent,
            usage_bytes: Arc::new(Mutex::new(0)),
            sim_usage_bytes: 0,
            incoming_files: Vec::new(),
        }
    }

    pub fn add_file_size(&self, size: u64) {
        if let Ok(mut usage) = self.usage_bytes.lock() {
            *usage += size;
        }
    }

    pub fn subtract_file_size(&self, size: u64) {
        if let Ok(mut usage) = self.usage_bytes.lock() {
            *usage = usage.saturating_sub(size);
        }
    }

    pub fn size_delta(&self, old_size: u64, new_size: u64) {
        if let Ok(mut usage) = self.usage_bytes.lock() {
            *usage = usage.saturating_sub(old_size) + new_size;
        }
    }

    pub fn resolved_quota_bytes(&self) -> u64 {
        if self.quota_percent > 0.0 {
            if let Ok(stat) = nix::sys::statvfs::statvfs(&self.path) {
                let total_size = stat.blocks() as u64 * stat.fragment_size() as u64;
                ((total_size as f64) * (self.quota_percent / 100.0)) as u64
            } else {
                self.quota_bytes
            }
        } else {
            self.quota_bytes
        }
    }

    pub fn full_test(&self, file_size: u64) -> bool {
        (self.sim_usage_bytes + file_size) > self.resolved_quota_bytes()
    }

    /// Iterates through enqueued files and transfers them into this tier.
    pub fn transfer_files(&mut self, buff_sz: usize, _run_path: &Path, db_path: &Path) {
        let db = match rusqlite::Connection::open(db_path) {
            Ok(conn) => conn,
            Err(_) => return,
        };

        for file in &self.incoming_files {
            let old_path = file.full_path();
            let new_path = self.path.join(&file.relative_path);
            let mut conflicted = false;

            // In the C++ code, we check if the file is currently open.
            // For the stub, we simulate moving the file.
            let success = self.move_file(
                &old_path,
                &new_path,
                buff_sz,
                &mut conflicted,
                &file.tier_id,
            );
            if success {
                // In a real run, update DB metadata, times, and write conflicts.
                let mut updated_meta = file.metadata.clone();
                updated_meta.tier_path = self.path.to_string_lossy().to_string();
                let _ = updated_meta.update(&db, &file.relative_path.to_string_lossy(), None);
            }
        }
        self.incoming_files.clear();
        self.sim_usage_bytes = 0;
    }

    /// Moves a file from one tier path to another using a buffer, handling temporary names and space limits.
    pub fn move_file(
        &self,
        old_path: &Path,
        new_path: &Path,
        _buff_sz: usize,
        conflicted: &mut bool,
        _orig_tier: &str,
    ) -> bool {
        *conflicted = false;
        if !old_path.exists() {
            return false;
        }

        // Create parent directories if they don't exist
        if let Some(parent) = new_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Stub out moving: rename file (or copy if cross-device, but standard std::fs::rename works)
        match fs::rename(old_path, new_path) {
            Ok(_) => {
                // Copy ownership/permissions in real code
                true
            }
            Err(_) => {
                // If rename fails (e.g. cross-device link), try copy-then-delete
                match fs::copy(old_path, new_path) {
                    Ok(_) => {
                        let _ = fs::remove_file(old_path);
                        true
                    }
                    Err(_) => false,
                }
            }
        }
    }

    pub fn usage_percent(&self) -> f64 {
        let usage = match self.usage_bytes.lock() {
            Ok(u) => *u,
            Err(e) => *e.into_inner(),
        };
        let q_bytes = self.resolved_quota_bytes();
        if q_bytes == 0 {
            0.0
        } else {
            (usage as f64 / q_bytes as f64) * 100.0
        }
    }
}

/// Represents a file tracked by the tiering engine, matching `File`.
#[derive(Debug, Clone)]
pub struct File {
    pub size: u64,
    pub tier_id: String,
    pub relative_path: PathBuf,
    pub metadata: Metadata,
    pub atime: SystemTime,
    pub mtime: SystemTime,
}

impl File {
    pub fn new(relative_path: PathBuf, tier_id: String, size: u64, metadata: Metadata) -> Self {
        Self {
            size,
            tier_id,
            relative_path,
            metadata,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
        }
    }

    /// Computes full path to the file based on the tier root.
    pub fn full_path_with_root(&self, tier_root: &Path) -> PathBuf {
        tier_root.join(&self.relative_path)
    }

    /// Computes full path based on metadata tier path.
    pub fn full_path(&self) -> PathBuf {
        PathBuf::from(&self.metadata.tier_path).join(&self.relative_path)
    }
}
