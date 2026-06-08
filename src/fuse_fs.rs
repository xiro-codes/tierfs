//! FUSE filesystem implementation.

use crate::engine::TierEngine;
use crate::open_files::{FusePriv, DirEntry};
use crate::metadata::Metadata;
use fuser::{
    Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyOpen, ReplyWrite, ReplyEmpty, Request, INodeNo, FileHandle, OpenFlags,
    WriteFlags, LockOwner, FileType, FileAttr, Errno, FopenFlags, RenameFlags,
    BsdFileFlags, Generation, TimeOrNow,
};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::time::{SystemTime, Duration};

/// FUSE filesystem implementation for tierfs, matching the `FusePassthrough` class.
pub struct TierFS {
    pub engine: Arc<TierEngine>,
    pub priv_data: Arc<FusePriv>,
}

impl TierFS {
    pub fn new(engine: Arc<TierEngine>, mount_point: PathBuf) -> Self {
        let db_path = &engine.db_path;
        let db = rusqlite::Connection::open(db_path).expect("Failed to open SQLite DB for FUSE");
        let config_path = PathBuf::from(&engine.config.run_path); // using run path as parent fallback
        let priv_data = Arc::new(FusePriv::new(config_path, mount_point, db));
        Self { engine, priv_data }
    }

    /// Resolves a virtual path to the physical path on the appropriate tier.
    #[allow(dead_code)]
    fn resolve_path(&self, relative_path: &Path) -> Option<PathBuf> {
        Some(self.resolve_physical_path(relative_path))
    }

    fn get_relative_path(&self, ino: INodeNo) -> Option<PathBuf> {
        self.priv_data.get_path(ino.0)
    }

    fn resolve_physical_path(&self, relative_path: &Path) -> PathBuf {
        let rel_str = relative_path.to_string_lossy();
        let db_conn = self.priv_data.db.lock().unwrap();
        let meta = Metadata::from_db(&db_conn, &rel_str, None);
        if !meta.not_found {
            PathBuf::from(&meta.tier_path).join(relative_path)
        } else {
            // Check disk on each tier in order
            if let Ok(tiers) = self.engine.tiers.lock() {
                for tier in tiers.iter() {
                    let p = tier.path.join(relative_path);
                    if p.exists() {
                        return p;
                    }
                }
                // Default to top tier
                if let Some(top_tier) = tiers.first() {
                    return top_tier.path.join(relative_path);
                }
            }
            relative_path.to_path_buf()
        }
    }

    fn resolve_dir_paths(&self, relative_path: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Ok(tiers) = self.engine.tiers.lock() {
            for tier in tiers.iter() {
                let p = tier.path.join(relative_path);
                if p.is_dir() {
                    paths.push(p);
                }
            }
        }
        if paths.is_empty() {
            if let Ok(tiers) = self.engine.tiers.lock() {
                if let Some(top_tier) = tiers.first() {
                    paths.push(top_tier.path.join(relative_path));
                }
            }
        }
        paths
    }
}

fn time_to_timespec(t: Option<TimeOrNow>) -> libc::timespec {
    match t {
        None => libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT },
        Some(TimeOrNow::Now) => libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_NOW },
        Some(TimeOrNow::SpecificTime(st)) => {
            let duration = st.duration_since(SystemTime::UNIX_EPOCH).unwrap_or(Duration::ZERO);
            libc::timespec {
                tv_sec: duration.as_secs() as libc::time_t,
                tv_nsec: duration.subsec_nanos() as libc::c_long,
            }
        }
    }
}

fn get_file_attr(ino: u64, meta: &std::fs::Metadata) -> FileAttr {
    let kind = if meta.is_dir() {
        FileType::Directory
    } else if meta.is_symlink() {
        FileType::Symlink
    } else {
        FileType::RegularFile
    };

    let atime = SystemTime::UNIX_EPOCH + Duration::new(meta.atime() as u64, meta.atime_nsec() as u32);
    let mtime = SystemTime::UNIX_EPOCH + Duration::new(meta.mtime() as u64, meta.mtime_nsec() as u32);
    let ctime = SystemTime::UNIX_EPOCH + Duration::new(meta.ctime() as u64, meta.ctime_nsec() as u32);

    FileAttr {
        ino: INodeNo(ino),
        size: meta.len(),
        blocks: meta.blocks(),
        atime,
        mtime,
        ctime,
        crtime: SystemTime::UNIX_EPOCH,
        kind,
        perm: (meta.mode() & 0o7777) as u16,
        nlink: meta.nlink() as u32,
        uid: meta.uid(),
        gid: meta.gid(),
        rdev: meta.rdev() as u32,
        blksize: meta.blksize() as u32,
        flags: 0,
    }
}

fn get_file_attr_from_stat(ino: u64, st: &libc::stat) -> FileAttr {
    let kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFDIR => FileType::Directory,
        libc::S_IFLNK => FileType::Symlink,
        _ => FileType::RegularFile,
    };

    let atime = SystemTime::UNIX_EPOCH + Duration::new(st.st_atime as u64, st.st_atime_nsec as u32);
    let mtime = SystemTime::UNIX_EPOCH + Duration::new(st.st_mtime as u64, st.st_mtime_nsec as u32);
    let ctime = SystemTime::UNIX_EPOCH + Duration::new(st.st_ctime as u64, st.st_ctime_nsec as u32);

    FileAttr {
        ino: INodeNo(ino),
        size: st.st_size as u64,
        blocks: st.st_blocks as u64,
        atime,
        mtime,
        ctime,
        crtime: SystemTime::UNIX_EPOCH,
        kind,
        perm: (st.st_mode & 0o7777) as u16,
        nlink: st.st_nlink as u32,
        uid: st.st_uid,
        gid: st.st_gid,
        rdev: st.st_rdev as u32,
        blksize: st.st_blksize as u32,
        flags: 0,
    }
}

impl Filesystem for TierFS {
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

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let parent_path = match self.get_relative_path(parent) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let relative_path = parent_path.join(name);
        
        let is_dir = {
            let mut found = false;
            if let Ok(tiers) = self.engine.tiers.lock() {
                for tier in tiers.iter() {
                    if tier.path.join(&relative_path).is_dir() {
                        found = true;
                        break;
                    }
                }
            }
            found
        };

        let physical_path = if is_dir {
            if let Ok(tiers) = self.engine.tiers.lock() {
                tiers.first().unwrap().path.join(&relative_path)
            } else {
                relative_path.clone()
            }
        } else {
            self.resolve_physical_path(&relative_path)
        };

        match std::fs::symlink_metadata(&physical_path) {
            Ok(meta) => {
                let ino = self.priv_data.get_ino(&relative_path);
                let attr = get_file_attr(ino, &meta);
                reply.entry(&Duration::from_secs(1), &attr, Generation(0));
            }
            Err(e) => {
                reply.error(Errno::from(e));
            }
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, fh: Option<FileHandle>, reply: ReplyAttr) {
        let relative_path = match self.get_relative_path(ino) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        if let Some(fd) = fh {
            let fd = fd.0 as i32;
            let mut st = unsafe { std::mem::zeroed::<libc::stat>() };
            let res = unsafe { libc::fstat(fd, &mut st) };
            if res == -1 {
                reply.error(Errno::from(std::io::Error::last_os_error()));
                return;
            }
            let attr = get_file_attr_from_stat(ino.0, &st);
            reply.attr(&Duration::from_secs(1), &attr);
        } else {
            let is_dir = {
                let mut found = false;
                if let Ok(tiers) = self.engine.tiers.lock() {
                    for tier in tiers.iter() {
                        if tier.path.join(&relative_path).is_dir() {
                            found = true;
                            break;
                        }
                    }
                }
                found
            };

            let physical_path = if is_dir {
                if let Ok(tiers) = self.engine.tiers.lock() {
                    tiers.first().unwrap().path.join(&relative_path)
                } else {
                    relative_path.clone()
                }
            } else {
                self.resolve_physical_path(&relative_path)
            };

            match std::fs::symlink_metadata(&physical_path) {
                Ok(meta) => {
                    let attr = get_file_attr(ino.0, &meta);
                    reply.attr(&Duration::from_secs(1), &attr);
                }
                Err(e) => {
                    reply.error(Errno::from(e));
                }
            }
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let relative_path = match self.get_relative_path(ino) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let is_dir = {
            let mut found = false;
            if let Ok(tiers) = self.engine.tiers.lock() {
                for tier in tiers.iter() {
                    if tier.path.join(&relative_path).is_dir() {
                        found = true;
                        break;
                    }
                }
            }
            found
        };

        let physical_paths = if is_dir {
            self.resolve_dir_paths(&relative_path)
        } else {
            vec![self.resolve_physical_path(&relative_path)]
        };

        for path in &physical_paths {
            let path_str = path.to_string_lossy().to_string();
            let path_c = std::ffi::CString::new(path_str).unwrap();

            if let Some(m) = mode {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(m));
            }

            if uid.is_some() || gid.is_some() {
                let u = uid.unwrap_or(u32::MAX);
                let g = gid.unwrap_or(u32::MAX);
                unsafe {
                    libc::chown(path_c.as_ptr(), u, g);
                }
            }

            if let Some(sz) = size {
                if !is_dir {
                    if let Some(fd) = fh {
                        unsafe {
                            libc::ftruncate(fd.0 as i32, sz as libc::off_t);
                        }
                    } else {
                        let _ = std::fs::OpenOptions::new()
                            .write(true)
                            .open(path)
                            .and_then(|f| f.set_len(sz));
                    }
                }
            }

            if atime.is_some() || mtime.is_some() {
                let ts = [
                    time_to_timespec(atime),
                    time_to_timespec(mtime),
                ];
                unsafe {
                    libc::utimensat(libc::AT_FDCWD, path_c.as_ptr(), ts.as_ptr(), libc::AT_SYMLINK_NOFOLLOW);
                }
            }
        }

        if let Some(first_path) = physical_paths.first() {
            match std::fs::symlink_metadata(first_path) {
                Ok(meta) => {
                    let attr = get_file_attr(ino.0, &meta);
                    reply.attr(&Duration::from_secs(1), &attr);
                }
                Err(e) => {
                    reply.error(Errno::from(e));
                }
            }
        } else {
            reply.error(Errno::ENOENT);
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let relative_path = match self.get_relative_path(ino) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let is_dir = {
            let mut found = false;
            if let Ok(tiers) = self.engine.tiers.lock() {
                for tier in tiers.iter() {
                    if tier.path.join(&relative_path).is_dir() {
                        found = true;
                        break;
                    }
                }
            }
            found
        };

        if is_dir {
            reply.error(Errno::EISDIR);
            return;
        }

        let physical_path = self.resolve_physical_path(&relative_path);
        let path_str = physical_path.to_string_lossy().to_string();
        let file_size = std::fs::metadata(&physical_path).map(|m| m.len()).unwrap_or(0);

        crate::open_files::register_open_file(&path_str);

        let path_c = std::ffi::CString::new(path_str.clone()).unwrap();
        let fd = unsafe { libc::open(path_c.as_ptr(), flags.0, 0o777) };

        if fd == -1 {
            crate::open_files::release_open_file(&path_str);
            reply.error(Errno::from(std::io::Error::last_os_error()));
            return;
        }

        self.priv_data.insert_fd_to_path(fd as u64, path_str);
        self.priv_data.insert_size_at_open(fd as u64, file_size);

        let rel_str = relative_path.to_string_lossy();
        if let Ok(db_conn) = self.priv_data.db.lock() {
            let mut meta = Metadata::from_db(&db_conn, &rel_str, None);
            meta.touch();
            let _ = meta.update(&db_conn, &rel_str, None);
        }

        reply.opened(FileHandle(fd as u64), FopenFlags::empty());
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let mut buf = vec![0u8; size as usize];
        let res = unsafe {
            libc::pread(fh.0 as i32, buf.as_mut_ptr() as *mut libc::c_void, size as usize, offset as libc::off_t)
        };

        if res == -1 {
            reply.error(Errno::from(std::io::Error::last_os_error()));
        } else {
            reply.data(&buf[..res as usize]);
        }
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let fd = fh.0 as i32;
        let mut res = -1;

        loop {
            res = unsafe {
                libc::pwrite(fd, data.as_ptr() as *const libc::c_void, data.len(), offset as libc::off_t)
            };

            if res == -1 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ENOSPC) {
                    if self.engine.config.strict_period {
                        reply.error(Errno::ENOSPC);
                        return;
                    } else {
                        self.engine.tier();
                        std::thread::yield_now();
                    }
                } else {
                    reply.error(Errno::from(err));
                    return;
                }
            } else {
                break;
            }
        }

        reply.written(res as u32);
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let fd = fh.0 as i32;
        let mut old_size = None;
        let mut new_size = None;
        let mut path_str = None;

        if let Some(path) = self.priv_data.fd_to_path(fh.0) {
            path_str = Some(path.clone());
            old_size = self.priv_data.size_at_open(fh.0);
            
            let mut st = unsafe { std::mem::zeroed::<libc::stat>() };
            let res = unsafe { libc::fstat(fd, &mut st) };
            if res != -1 {
                new_size = Some(st.st_size as u64);
            }

            crate::open_files::release_open_file(&path);
            self.priv_data.remove_fd_to_path(fh.0);
            self.priv_data.remove_size_at_open(fh.0);
        }

        let res = unsafe { libc::close(fd) };

        if let (Some(old), Some(new), Some(path)) = (old_size, new_size, path_str) {
            let path_buf = PathBuf::from(&path);
            let mut tiers = self.engine.tiers.lock().unwrap();
            let mut matched_tier = None;
            for tier in tiers.iter_mut() {
                if path_buf.starts_with(&tier.path) {
                    tier.size_delta(old, new);
                    matched_tier = Some(tier.id.clone());
                    break;
                }
            }

            if let Some(_) = matched_tier {
                if !self.engine.config.strict_period {
                    let mut quota_exceeded = false;
                    for tier in tiers.iter() {
                        let usage = match tier.usage_bytes.lock() {
                            Ok(u) => *u,
                            Err(e) => *e.into_inner(),
                        };
                        if usage > tier.quota_bytes {
                            quota_exceeded = true;
                            break;
                        }
                    }
                    if quota_exceeded {
                        let engine_clone = Arc::clone(&self.engine);
                        std::thread::spawn(move || {
                            engine_clone.tier();
                        });
                    }
                }
            }
        }

        if res == -1 {
            reply.error(Errno::from(std::io::Error::last_os_error()));
        } else {
            reply.ok();
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let parent_path = match self.get_relative_path(parent) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let relative_path = parent_path.join(name);
        let rel_str = relative_path.to_string_lossy();

        let top_tier = {
            let tiers = self.engine.tiers.lock().unwrap();
            tiers.first().unwrap().path.clone()
        };

        let physical_path = top_tier.join(&relative_path);
        let path_str = physical_path.to_string_lossy().to_string();

        crate::open_files::register_open_file(&path_str);

        let path_c = std::ffi::CString::new(path_str.clone()).unwrap();
        let mode = mode & !umask;
        let fd = unsafe { libc::open(path_c.as_ptr(), flags | libc::O_CREAT | libc::O_TRUNC, mode) };

        if fd == -1 {
            crate::open_files::release_open_file(&path_str);
            reply.error(Errno::from(std::io::Error::last_os_error()));
            return;
        }

        if let Ok(db_conn) = self.priv_data.db.lock() {
            let mut meta = Metadata::default();
            meta.tier_path = top_tier.to_string_lossy().to_string();
            let _ = meta.update(&db_conn, &rel_str, None);
        }

        let ino = self.priv_data.get_ino(&relative_path);
        self.priv_data.insert_fd_to_path(fd as u64, path_str);
        self.priv_data.insert_size_at_open(fd as u64, 0);

        match std::fs::symlink_metadata(&physical_path) {
            Ok(meta) => {
                let attr = get_file_attr(ino, &meta);
                reply.created(&Duration::from_secs(1), &attr, Generation(0), FileHandle(fd as u64), FopenFlags::empty());
            }
            Err(e) => {
                reply.error(Errno::from(e));
            }
        }
    }

    fn mkdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, mode: u32, _umask: u32, reply: ReplyEntry) {
        let parent_path = match self.get_relative_path(parent) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let relative_path = parent_path.join(name);
        
        let top_tier = {
            let tiers = self.engine.tiers.lock().unwrap();
            tiers.first().unwrap().path.clone()
        };

        let physical_path = top_tier.join(&relative_path);

        match std::fs::create_dir(&physical_path) {
            Ok(_) => {
                let _ = std::fs::set_permissions(&physical_path, std::fs::Permissions::from_mode(mode));
                let ino = self.priv_data.get_ino(&relative_path);
                match std::fs::symlink_metadata(&physical_path) {
                    Ok(meta) => {
                        let attr = get_file_attr(ino, &meta);
                        reply.entry(&Duration::from_secs(1), &attr, Generation(0));
                    }
                    Err(e) => {
                        reply.error(Errno::from(e));
                    }
                }
            }
            Err(e) => {
                reply.error(Errno::from(e));
            }
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_relative_path(parent) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let relative_path = parent_path.join(name);

        let mut removed = false;
        let mut last_err = Errno::ENOENT;

        if let Ok(tiers) = self.engine.tiers.lock() {
            for tier in tiers.iter() {
                let dir_path = tier.path.join(&relative_path);
                if dir_path.is_dir() {
                    match std::fs::remove_dir(&dir_path) {
                        Ok(_) => {
                            removed = true;
                        }
                        Err(e) => {
                            last_err = Errno::from(e);
                        }
                    }
                }
            }
        }

        if removed {
            self.priv_data.remove_ino(&relative_path);
            reply.ok();
        } else {
            reply.error(last_err);
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_relative_path(parent) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let relative_path = parent_path.join(name);
        let rel_str = relative_path.to_string_lossy();

        let physical_path = self.resolve_physical_path(&relative_path);
        let file_size = std::fs::metadata(&physical_path).map(|m| m.len()).unwrap_or(0);

        match std::fs::remove_file(&physical_path) {
            Ok(_) => {
                if let Ok(db_conn) = self.priv_data.db.lock() {
                    let delete_query = "DELETE FROM metadata WHERE relative_path = ?1";
                    let _ = db_conn.execute(delete_query, rusqlite::params![rel_str]);
                }

                if let Ok(mut tiers) = self.engine.tiers.lock() {
                    for tier in tiers.iter_mut() {
                        if physical_path.starts_with(&tier.path) {
                            tier.subtract_file_size(file_size);
                            break;
                        }
                    }
                }

                self.priv_data.remove_ino(&relative_path);
                reply.ok();
            }
            Err(e) => {
                reply.error(Errno::from(e));
            }
        }
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let parent_path = match self.get_relative_path(parent) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let newparent_path = match self.get_relative_path(newparent) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let old_rel_path = parent_path.join(name);
        let new_rel_path = newparent_path.join(newname);

        let old_rel_str = old_rel_path.to_string_lossy();
        let new_rel_str = new_rel_path.to_string_lossy();

        let is_dir = {
            let mut found = false;
            if let Ok(tiers) = self.engine.tiers.lock() {
                for tier in tiers.iter() {
                    if tier.path.join(&old_rel_path).is_dir() {
                        found = true;
                        break;
                    }
                }
            }
            found
        };

        if is_dir {
            let mut moved = false;
            let mut last_err = Errno::ENOENT;

            if let Ok(tiers) = self.engine.tiers.lock() {
                for tier in tiers.iter() {
                    let old_dir = tier.path.join(&old_rel_path);
                    let new_dir = tier.path.join(&new_rel_path);
                    if old_dir.is_dir() {
                        if let Some(parent) = new_dir.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        match std::fs::rename(&old_dir, &new_dir) {
                            Ok(_) => {
                                moved = true;
                            }
                            Err(e) => {
                                last_err = Errno::from(e);
                            }
                        }
                    }
                }
            }

            if moved {
                if let Ok(db_conn) = self.priv_data.db.lock() {
                    let _ = crate::tools::update_keys_in_directory(&db_conn, &old_rel_str, &new_rel_str);
                }

                self.priv_data.rename_path(&old_rel_path, &new_rel_path);
                reply.ok();
            } else {
                reply.error(last_err);
            }
        } else {
            let old_physical = self.resolve_physical_path(&old_rel_path);
            
            let tier_root = {
                let mut root = PathBuf::new();
                if let Ok(tiers) = self.engine.tiers.lock() {
                    for tier in tiers.iter() {
                        if old_physical.starts_with(&tier.path) {
                            root = tier.path.clone();
                            break;
                        }
                    }
                }
                if root.as_os_str().is_empty() {
                    if let Ok(tiers) = self.engine.tiers.lock() {
                        tiers.first().unwrap().path.clone()
                    } else {
                        PathBuf::new()
                    }
                } else {
                    root
                }
            };

            let new_physical = tier_root.join(&new_rel_path);
            if let Some(parent) = new_physical.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match std::fs::rename(&old_physical, &new_physical) {
                Ok(_) => {
                    if let Ok(db_conn) = self.priv_data.db.lock() {
                        let mut meta = Metadata::from_db(&db_conn, &old_rel_str, None);
                        meta.tier_path = tier_root.to_string_lossy().to_string();
                        let _ = meta.update(&db_conn, &new_rel_str, Some(&old_rel_str));
                    }

                    self.priv_data.rename_path(&old_rel_path, &new_rel_path);
                    reply.ok();
                }
                Err(e) => {
                    reply.error(Errno::from(e));
                }
            }
        }
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let relative_path = match self.get_relative_path(ino) {
            Some(path) => path,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();

        entries.push(DirEntry {
            name: std::ffi::OsString::from("."),
            ino: ino.0,
            kind: FileType::Directory,
        });
        seen.insert(std::ffi::OsString::from("."));

        if ino != INodeNo::ROOT {
            if let Some(parent_path) = relative_path.parent() {
                let parent_ino = self.priv_data.get_ino(parent_path);
                entries.push(DirEntry {
                    name: std::ffi::OsString::from(".."),
                    ino: parent_ino,
                    kind: FileType::Directory,
                });
                seen.insert(std::ffi::OsString::from(".."));
            }
        } else {
            entries.push(DirEntry {
                name: std::ffi::OsString::from(".."),
                ino: 1,
                kind: FileType::Directory,
            });
            seen.insert(std::ffi::OsString::from(".."));
        }

        if let Ok(tiers) = self.engine.tiers.lock() {
            for tier in tiers.iter() {
                let dir_path = tier.path.join(&relative_path);
                if let Ok(read_dir) = std::fs::read_dir(dir_path) {
                    for entry_res in read_dir {
                        if let Ok(entry) = entry_res {
                            let name = entry.file_name();
                            let name_str = name.to_string_lossy();
                            if name_str.starts_with('.') && name_str.ends_with(".tierfs.hide") {
                                continue;
                            }
                            if !seen.contains(&name) {
                                seen.insert(name.clone());
                                if let Ok(meta) = entry.metadata() {
                                    let kind = if meta.is_dir() {
                                        FileType::Directory
                                    } else if meta.is_symlink() {
                                        FileType::Symlink
                                    } else {
                                        FileType::RegularFile
                                    };
                                    let child_relative = relative_path.join(&name);
                                    let child_ino = self.priv_data.get_ino(&child_relative);
                                    entries.push(DirEntry {
                                        name,
                                        ino: child_ino,
                                        kind,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        static NEXT_DIR_FH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let dfh = NEXT_DIR_FH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        self.priv_data.insert_dir_handle(dfh, entries);

        reply.opened(FileHandle(dfh), FopenFlags::empty());
    }

    fn readdir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let entries = match self.priv_data.get_dir_entries(fh.0) {
            Some(e) => e,
            None => {
                reply.error(Errno::EBADF);
                return;
            }
        };

        if offset as usize >= entries.len() {
            reply.ok();
            return;
        }

        for (idx, entry) in entries.iter().enumerate().skip(offset as usize) {
            let next_offset = (idx + 1) as u64;
            let buffer_full = reply.add(INodeNo(entry.ino), next_offset, entry.kind, &entry.name);
            if buffer_full {
                break;
            }
        }

        reply.ok();
    }

    fn releasedir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        self.priv_data.remove_dir_handle(fh.0);
        reply.ok();
    }
}
