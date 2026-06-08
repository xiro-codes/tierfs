//! FUSE filesystem implementation.

use crate::engine::TierEngine;
use crate::open_files::FusePriv;
use crate::metadata::Metadata;
use fuser::{
    Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyOpen, ReplyWrite, Request, INodeNo, FileHandle, FopenFlags, OpenFlags,
    WriteFlags, LockOwner, Errno,
};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// FUSE filesystem implementation for autotier, matching the `FusePassthrough` class.
pub struct AutotierFS {
    pub engine: Arc<TierEngine>,
    pub priv_data: Arc<FusePriv>,
}

impl AutotierFS {
    pub fn new(engine: Arc<TierEngine>, mount_point: PathBuf) -> Self {
        let db_path = &engine.db_path;
        let db = rusqlite::Connection::open(db_path).expect("Failed to open SQLite DB for FUSE");
        let config_path = PathBuf::from(&engine.config.run_path); // using run path as parent fallback
        let priv_data = Arc::new(FusePriv::new(config_path, mount_point, db));
        Self { engine, priv_data }
    }

    /// Resolves a virtual path to the physical path on the appropriate tier.
    fn resolve_path(&self, relative_path: &Path) -> Option<PathBuf> {
        let rel_str = relative_path.to_string_lossy();
        let db_conn = self.priv_data.db.lock().unwrap();
        let meta = Metadata::from_db(&db_conn, &rel_str, None);
        if meta.not_found {
            // Default to the first tier (top tier)
            let tiers = self.engine.tiers.lock().ok()?;
            tiers.first().map(|t| t.path.join(relative_path))
        } else {
            Some(PathBuf::from(&meta.tier_path).join(relative_path))
        }
    }
}

impl Filesystem for AutotierFS {
    fn init(&mut self, _req: &Request, _config: &mut fuser::KernelConfig) -> Result<(), std::io::Error> {
        log::debug!("FUSE init called, spawning background threads.");
        
        let engine_clone1 = Arc::clone(&self.engine);
        std::thread::spawn(move || {
            engine_clone1.begin(true);
        });

        let engine_clone2 = Arc::clone(&self.engine);
        std::thread::spawn(move || {
            engine_clone2.process_adhoc_requests();
        });

        Ok(())
    }

    fn destroy(&mut self) {
        log::debug!("FUSE destroy called");
        self.engine.stop();
    }

    fn lookup(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, _reply: ReplyEntry) {
        todo!("lookup")
    }

    fn getattr(&self, _req: &Request, _ino: INodeNo, _fh: Option<FileHandle>, _reply: ReplyAttr) {
        todo!("getattr")
    }

    fn open(&self, _req: &Request, _ino: INodeNo, _flags: OpenFlags, _reply: ReplyOpen) {
        todo!("open")
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _offset: u64,
        _size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _reply: ReplyData,
    ) {
        todo!("read")
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _offset: u64,
        _data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _reply: ReplyWrite,
    ) {
        todo!("write")
    }

    fn create(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        _reply: ReplyCreate,
    ) {
        todo!("create")
    }

    fn readdir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _offset: u64,
        _reply: ReplyDirectory,
    ) {
        todo!("readdir")
    }
}
