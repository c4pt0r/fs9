use std::ffi::OsStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs9_client::{Fs9Client, OpenFlags};
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request,
};
use tokio::runtime::Handle as TokioHandle;
use tracing::{debug, error, warn};

use crate::handle::HandleTable;
use crate::inode::{InodeTable, ROOT_INO};

const BLOCK_SIZE: u32 = 4096;

pub struct Fs9Fuse {
    client: Arc<Fs9Client>,
    inodes: InodeTable,
    handles: HandleTable,
    runtime: TokioHandle,
    uid: u32,
    gid: u32,
    ttl: Duration,
}

impl Fs9Fuse {
    pub fn new(
        client: Fs9Client,
        runtime: TokioHandle,
        uid: u32,
        gid: u32,
        cache_ttl: Duration,
    ) -> Self {
        Self {
            client: Arc::new(client),
            inodes: InodeTable::new(cache_ttl),
            handles: HandleTable::new(),
            runtime,
            uid,
            gid,
            ttl: cache_ttl,
        }
    }

    fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.runtime.block_on(f)
    }

    fn fs9_to_file_type(ft: fs9_client::FileType) -> FileType {
        match ft {
            fs9_client::FileType::Directory => FileType::Directory,
            fs9_client::FileType::Symlink => FileType::Symlink,
            fs9_client::FileType::Regular => FileType::RegularFile,
        }
    }

    fn info_to_attr(&self, info: &fs9_client::FileInfo, ino: u64) -> FileAttr {
        let file_type = Self::fs9_to_file_type(info.file_type);
        let nlink = if file_type == FileType::Directory {
            2
        } else {
            1
        };

        FileAttr {
            ino,
            size: info.size,
            blocks: (info.size + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64,
            atime: UNIX_EPOCH + Duration::from_secs(info.atime),
            mtime: UNIX_EPOCH + Duration::from_secs(info.mtime),
            ctime: UNIX_EPOCH + Duration::from_secs(info.ctime),
            crtime: UNIX_EPOCH,
            kind: file_type,
            perm: info.mode as u16,
            nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: BLOCK_SIZE,
            flags: 0,
        }
    }

    fn fetch_attr(&self, path: &str, ino: u64) -> Result<FileAttr, i32> {
        if let Some(cached) = self.inodes.get_cached_attr(ino) {
            return Ok(cached);
        }

        match self.block_on(self.client.stat(path)) {
            Ok(info) => {
                let attr = self.info_to_attr(&info, ino);
                self.inodes.cache_attr(ino, attr);
                Ok(attr)
            }
            Err(e) => {
                debug!("stat failed for {}: {}", path, e);
                Err(error_to_errno(&e))
            }
        }
    }

    fn flags_to_open_flags(flags: i32) -> OpenFlags {
        let read = (flags & libc::O_ACCMODE) != libc::O_WRONLY;
        let write = (flags & libc::O_ACCMODE) != libc::O_RDONLY;
        let create = (flags & libc::O_CREAT) != 0;
        let truncate = (flags & libc::O_TRUNC) != 0;
        let append = (flags & libc::O_APPEND) != 0;

        OpenFlags {
            read,
            write,
            create,
            truncate,
            append,
            directory: false,
        }
    }
}

impl Filesystem for Fs9Fuse {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let parent_path = match self.inodes.get_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = if parent_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent_path, name)
        };

        match self.block_on(self.client.stat(&child_path)) {
            Ok(info) => {
                let ino = self.inodes.get_or_create_ino(&child_path);
                let attr = self.info_to_attr(&info, ino);
                self.inodes.cache_attr(ino, attr);
                reply.entry(&self.ttl, &attr, 0);
            }
            Err(e) => {
                debug!("lookup failed for {}: {}", child_path, e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        match self.fetch_attr(&path, ino) {
            Ok(attr) => reply.attr(&self.ttl, &attr),
            Err(e) => reply.error(e),
        }
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<fuser::TimeOrNow>,
        mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let changes = fs9_client::StatChanges {
            mode,
            uid,
            gid,
            size,
            atime: atime.map(|t| time_or_now_to_secs(t)),
            mtime: mtime.map(|t| time_or_now_to_secs(t)),
            ..Default::default()
        };

        match self.block_on(self.client.wstat(&path, changes)) {
            Ok(()) => {
                self.inodes.invalidate_attr(ino);
                match self.fetch_attr(&path, ino) {
                    Ok(attr) => reply.attr(&self.ttl, &attr),
                    Err(e) => reply.error(e),
                }
            }
            Err(e) => {
                error!("setattr failed for {}: {}", path, e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let entries = match self.block_on(self.client.readdir(&path)) {
            Ok(e) => e,
            Err(e) => {
                error!("readdir failed for {}: {}", path, e);
                reply.error(error_to_errno(&e));
                return;
            }
        };

        let mut full_entries: Vec<(u64, FileType, String)> = vec![
            (ino, FileType::Directory, ".".to_string()),
            (
                if ino == ROOT_INO {
                    ROOT_INO
                } else {
                    self.inodes.get_or_create_ino(&parent_path(&path))
                },
                FileType::Directory,
                "..".to_string(),
            ),
        ];

        for info in entries {
            let child_ino = self.inodes.get_or_create_ino(&info.path);
            let child_attr = self.info_to_attr(&info, child_ino);
            self.inodes.cache_attr(child_ino, child_attr);
            let name = info.name().to_string();
            full_entries.push((child_ino, Self::fs9_to_file_type(info.file_type), name));
        }

        for (i, (ino, kind, name)) in full_entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*ino, (i + 1) as i64, *kind, name) {
                break;
            }
        }

        reply.ok();
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let open_flags = Self::flags_to_open_flags(flags);

        match self.block_on(self.client.open(&path, open_flags)) {
            Ok(handle) => {
                let fh = self.handles.insert(handle, flags);
                reply.opened(fh, 0);
            }
            Err(e) => {
                error!("open failed for {}: {}", path, e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let handle = match self.handles.get(fh) {
            Some(h) => h,
            None => {
                reply.error(libc::EBADF);
                return;
            }
        };

        match self.block_on(self.client.read(&handle, offset as u64, size as usize)) {
            Ok(data) => reply.data(&data),
            Err(e) => {
                error!("read failed: {}", e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let handle = match self.handles.get(fh) {
            Some(h) => h,
            None => {
                reply.error(libc::EBADF);
                return;
            }
        };

        match self.block_on(self.client.write(&handle, offset as u64, data)) {
            Ok(written) => {
                self.inodes.invalidate_attr(ino);
                reply.written(written as u32);
            }
            Err(e) => {
                error!("write failed: {}", e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if let Some(handle) = self.handles.remove(fh) {
            if let Err(e) = self.block_on(self.client.close(handle)) {
                warn!("close failed: {}", e);
            }
        }
        reply.ok();
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let parent_path = match self.inodes.get_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = if parent_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent_path, name)
        };

        let mut open_flags = Self::flags_to_open_flags(flags);
        open_flags.create = true;

        match self.block_on(self.client.open(&child_path, open_flags)) {
            Ok(handle) => {
                let ino = self.inodes.get_or_create_ino(&child_path);
                let fh = self.handles.insert(handle.clone(), flags);

                let file_mode = mode & 0o777;
                let changes = fs9_client::StatChanges {
                    mode: Some(file_mode),
                    ..Default::default()
                };
                let _ = self.block_on(self.client.wstat(&child_path, changes));

                match self.block_on(self.client.stat(&child_path)) {
                    Ok(info) => {
                        let attr = self.info_to_attr(&info, ino);
                        self.inodes.cache_attr(ino, attr);
                        reply.created(&self.ttl, &attr, 0, fh, 0);
                    }
                    Err(_) => {
                        let attr = self.info_to_attr(handle.metadata(), ino);
                        self.inodes.cache_attr(ino, attr);
                        reply.created(&self.ttl, &attr, 0, fh, 0);
                    }
                }
            }
            Err(e) => {
                error!("create failed for {}: {}", child_path, e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let parent_path = match self.inodes.get_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = if parent_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent_path, name)
        };

        match self.block_on(self.client.mkdir(&child_path)) {
            Ok(()) => match self.block_on(self.client.stat(&child_path)) {
                Ok(info) => {
                    let ino = self.inodes.get_or_create_ino(&child_path);
                    let attr = self.info_to_attr(&info, ino);
                    self.inodes.cache_attr(ino, attr);
                    reply.entry(&self.ttl, &attr, 0);
                }
                Err(e) => {
                    error!("stat after mkdir failed: {}", e);
                    reply.error(error_to_errno(&e));
                }
            },
            Err(e) => {
                error!("mkdir failed for {}: {}", child_path, e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let parent_path = match self.inodes.get_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = if parent_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent_path, name)
        };

        match self.block_on(self.client.remove(&child_path)) {
            Ok(()) => {
                self.inodes.remove(&child_path);
                reply.ok();
            }
            Err(e) => {
                error!("unlink failed for {}: {}", child_path, e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        self.unlink(_req, parent, name, reply);
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let newname = match newname.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let parent_path = match self.inodes.get_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let newparent_path = match self.inodes.get_path(newparent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let old_path = if parent_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent_path, name)
        };

        let new_path = if newparent_path == "/" {
            format!("/{}", newname)
        } else {
            format!("{}/{}", newparent_path, newname)
        };

        let changes = fs9_client::StatChanges::new().rename(&new_path);

        match self.block_on(self.client.wstat(&old_path, changes)) {
            Ok(()) => {
                self.inodes.rename(&old_path, &new_path);
                reply.ok();
            }
            Err(e) => {
                error!("rename failed from {} to {}: {}", old_path, new_path, e);
                reply.error(error_to_errno(&e));
            }
        }
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        match self.block_on(self.client.statfs("/")) {
            Ok(stats) => {
                reply.statfs(
                    stats.total_bytes / BLOCK_SIZE as u64,
                    stats.free_bytes / BLOCK_SIZE as u64,
                    stats.free_bytes / BLOCK_SIZE as u64,
                    stats.total_inodes,
                    stats.free_inodes,
                    BLOCK_SIZE,
                    stats.max_name_len,
                    BLOCK_SIZE,
                );
            }
            Err(e) => {
                warn!("statfs failed: {}", e);
                reply.statfs(0, 0, 0, 0, 0, BLOCK_SIZE, 255, BLOCK_SIZE);
            }
        }
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn fsync(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }
}

fn parent_path(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }
    match path.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
        None => "/".to_string(),
    }
}

fn error_to_errno(e: &fs9_client::Fs9Error) -> i32 {
    use fs9_client::Fs9Error;
    match e {
        Fs9Error::NotFound(_) => libc::ENOENT,
        Fs9Error::PermissionDenied(_) => libc::EACCES,
        Fs9Error::AlreadyExists(_) => libc::EEXIST,
        Fs9Error::InvalidArgument(_) => libc::EINVAL,
        Fs9Error::NotDirectory(_) => libc::ENOTDIR,
        Fs9Error::IsDirectory(_) => libc::EISDIR,
        Fs9Error::DirectoryNotEmpty(_) => libc::ENOTEMPTY,
        Fs9Error::InvalidHandle => libc::EBADF,
        _ => libc::EIO,
    }
}

fn time_or_now_to_secs(t: fuser::TimeOrNow) -> u64 {
    match t {
        fuser::TimeOrNow::SpecificTime(st) => st
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        fuser::TimeOrNow::Now => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}
