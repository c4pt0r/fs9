use bytes::Bytes;
use fs9_sdk::{FileInfo, FileType, FsError, FsResult, Handle, OpenFlags, StatChanges};

use crate::{
    keys, systemtime_to_timestamp, timestamp_to_system_time, Inode, KvBackend, Superblock,
    PAGE_SIZE, ROOT_INODE,
};
use std::collections::BTreeMap;
use std::sync::Mutex;

pub struct PageFsProvider {
    pub(crate) kv: Box<dyn KvBackend>,
    handles: Mutex<BTreeMap<u64, (u64, String, OpenFlags)>>,
    next_handle: Mutex<u64>,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

impl PageFsProvider {
    pub fn new(kv: Box<dyn KvBackend>) -> Self {
        Self::with_config(kv, 0, 0)
    }

    pub fn with_config(kv: Box<dyn KvBackend>, uid: u32, gid: u32) -> Self {
        let provider = Self {
            kv,
            handles: Mutex::new(BTreeMap::new()),
            next_handle: Mutex::new(1),
            uid,
            gid,
        };
        provider.init_filesystem();
        provider
    }

    pub fn with_memory_backend() -> Self {
        Self::new(Box::new(crate::InMemoryKv::new()))
    }

    fn init_filesystem(&self) {
        if self.kv.get(&keys::superblock()).is_none() {
            eprintln!("[pagefs] No superblock found, creating fresh filesystem");
            let sb = Superblock::default();
            self.save_superblock(&sb);

            let root = Inode::new_directory(ROOT_INODE, 0o755);
            self.save_inode(&root);
            eprintln!("[pagefs] Created superblock and root inode");
        } else if self.load_inode(ROOT_INODE).is_none() {
            // Superblock exists but root inode is missing (e.g. stale data from
            // a previous session where writes failed silently). Recreate it.
            eprintln!("[pagefs] WARNING: Superblock exists but root inode missing — recreating");
            let root = Inode::new_directory(ROOT_INODE, 0o755);
            self.save_inode(&root);
        } else {
            eprintln!("[pagefs] Filesystem already initialized, superblock and root inode OK");
        }
    }

    pub(crate) fn load_superblock(&self) -> Superblock {
        self.kv
            .get(&keys::superblock())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    fn save_superblock(&self, sb: &Superblock) {
        let data = serde_json::to_vec(sb).unwrap();
        self.kv.set(&keys::superblock(), &data);
    }

    fn alloc_inode(&self) -> u64 {
        let mut sb = self.load_superblock();
        let id = sb.next_inode;
        sb.next_inode += 1;
        self.save_superblock(&sb);
        id
    }

    pub(crate) fn load_inode(&self, inode_id: u64) -> Option<Inode> {
        self.kv
            .get(&keys::inode(inode_id))
            .and_then(|data| serde_json::from_slice(&data).ok())
    }

    fn save_inode(&self, inode: &Inode) {
        let data = serde_json::to_vec(inode).unwrap();
        self.kv.set(&keys::inode(inode.id), &data);
    }

    fn delete_inode(&self, inode_id: u64) {
        self.kv.delete(&keys::inode(inode_id));
    }

    fn lookup(&self, parent_inode: u64, name: &str) -> Option<u64> {
        self.kv
            .get(&keys::dir_entry(parent_inode, name))
            .and_then(|data| {
                if data.len() == 8 {
                    Some(u64::from_be_bytes(data.try_into().unwrap()))
                } else {
                    None
                }
            })
    }

    fn link(&self, parent_inode: u64, name: &str, child_inode: u64) {
        self.kv.set(
            &keys::dir_entry(parent_inode, name),
            &child_inode.to_be_bytes(),
        );
    }

    fn unlink(&self, parent_inode: u64, name: &str) {
        self.kv.delete(&keys::dir_entry(parent_inode, name));
    }

    fn list_dir(&self, parent_inode: u64) -> Vec<(String, u64)> {
        let prefix = keys::dir_prefix(parent_inode);
        self.kv
            .scan(&prefix)
            .into_iter()
            .filter_map(|(key, value)| {
                let name_bytes = &key[prefix.len()..];
                let name = String::from_utf8(name_bytes.to_vec()).ok()?;
                let child_inode = u64::from_be_bytes(value.try_into().ok()?);
                Some((name, child_inode))
            })
            .collect()
    }

    pub(crate) fn read_page(&self, inode_id: u64, page_num: u64) -> Option<Vec<u8>> {
        self.kv.get(&keys::page(inode_id, page_num))
    }

    fn write_page(&self, inode_id: u64, page_num: u64, data: &[u8]) {
        let mut page_data = data.to_vec();
        if page_data.len() < PAGE_SIZE {
            page_data.resize(PAGE_SIZE, 0);
        }
        self.kv.set(&keys::page(inode_id, page_num), &page_data);
    }

    fn delete_pages(&self, inode_id: u64) {
        let prefix = keys::page_prefix(inode_id);
        let pages: Vec<_> = self.kv.scan(&prefix).into_iter().map(|(k, _)| k).collect();
        for key in pages {
            self.kv.delete(&key);
        }
    }

    pub(crate) fn resolve_path(&self, path: &str) -> FsResult<(u64, Inode)> {
        let path = self.normalize_path(path);
        if path == "/" {
            let inode = self
                .load_inode(ROOT_INODE)
                .ok_or_else(|| FsError::internal("root inode missing"))?;
            return Ok((ROOT_INODE, inode));
        }

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current_inode = ROOT_INODE;

        for (i, part) in parts.iter().enumerate() {
            let child_inode = self
                .lookup(current_inode, part)
                .ok_or_else(|| FsError::not_found(&path))?;

            if i < parts.len() - 1 {
                let inode = self
                    .load_inode(child_inode)
                    .ok_or_else(|| FsError::not_found(&path))?;
                if !inode.is_directory() {
                    return Err(FsError::not_directory(*part));
                }
            }
            current_inode = child_inode;
        }

        let inode = self
            .load_inode(current_inode)
            .ok_or_else(|| FsError::not_found(&path))?;
        Ok((current_inode, inode))
    }

    fn resolve_parent(&self, path: &str) -> FsResult<(u64, String)> {
        let path = self.normalize_path(path);
        if path == "/" {
            return Err(FsError::invalid_argument("cannot get parent of root"));
        }

        let (parent, name) = path.rsplit_once('/').unwrap_or(("", &path));
        let parent_path = if parent.is_empty() { "/" } else { parent };

        let (parent_inode, parent_node) = self.resolve_path(parent_path)?;
        if !parent_node.is_directory() {
            return Err(FsError::not_directory(parent_path));
        }

        Ok((parent_inode, name.to_string()))
    }

    fn normalize_path(&self, path: &str) -> String {
        let path = if path.is_empty() { "/" } else { path };
        if path == "/" {
            "/".to_string()
        } else {
            path.trim_end_matches('/').to_string()
        }
    }

    fn pages_needed(size: u64) -> u64 {
        if size == 0 {
            0
        } else {
            (size + PAGE_SIZE as u64 - 1) / PAGE_SIZE as u64
        }
    }

    pub fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let path = self.normalize_path(path);
        let (_, inode) = self.resolve_path(&path)?;

        Ok(FileInfo {
            path: path.clone(),
            size: inode.size,
            file_type: if inode.is_directory() {
                FileType::Directory
            } else {
                FileType::Regular
            },
            mode: inode.mode,
            uid: self.uid,
            gid: self.gid,
            atime: timestamp_to_system_time(inode.atime),
            mtime: timestamp_to_system_time(inode.mtime),
            ctime: timestamp_to_system_time(inode.ctime),
            etag: String::new(),
            symlink_target: None,
        })
    }

    pub fn open(&self, path: &str, flags: OpenFlags) -> FsResult<(Handle, FileInfo)> {
        let path = self.normalize_path(path);

        let inode_id = if flags.create {
            match self.resolve_path(&path) {
                Ok((id, _)) => id,
                Err(FsError::NotFound(_)) => {
                    let (parent_inode, name) = self.resolve_parent(&path)?;
                    let new_id = self.alloc_inode();

                    let inode = if flags.directory {
                        Inode::new_directory(new_id, 0o755)
                    } else {
                        let mut f = Inode::new_file(new_id, 0o644);
                        f.page_count = 1;
                        self.write_page(new_id, 0, &vec![0u8; PAGE_SIZE]);
                        f
                    };

                    self.save_inode(&inode);
                    self.link(parent_inode, &name, new_id);

                    new_id
                }
                Err(e) => return Err(e),
            }
        } else {
            let (id, _) = self.resolve_path(&path)?;
            id
        };

        if flags.truncate {
            if let Some(mut inode) = self.load_inode(inode_id) {
                if !inode.is_directory() {
                    self.delete_pages(inode_id);
                    inode.size = 0;
                    inode.page_count = 1;
                    self.write_page(inode_id, 0, &vec![0u8; PAGE_SIZE]);
                    inode.touch_mtime();
                    self.save_inode(&inode);
                }
            }
        }

        let info = self.stat(&path)?;

        let mut next = self.next_handle.lock().unwrap();
        let handle_id = *next;
        *next += 1;

        self.handles
            .lock()
            .unwrap()
            .insert(handle_id, (inode_id, path, flags));

        Ok((Handle::new(handle_id), info))
    }

    pub fn read(&self, handle: u64, offset: u64, size: usize) -> FsResult<Bytes> {
        let handles = self.handles.lock().unwrap();
        let (inode_id, path, _) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?
            .clone();
        drop(handles);

        let mut inode = self
            .load_inode(inode_id)
            .ok_or_else(|| FsError::not_found(&path))?;

        if inode.is_directory() {
            return Err(FsError::is_directory(&path));
        }

        let file_size = inode.size;
        if offset >= file_size {
            return Ok(Bytes::new());
        }

        let read_end = ((offset + size as u64).min(file_size)) as usize;
        let read_start = offset as usize;
        let total_to_read = read_end - read_start;

        let mut result = vec![0u8; total_to_read];
        let mut bytes_read = 0usize;
        let mut current_offset = offset as usize;

        while bytes_read < total_to_read {
            let page_num = (current_offset / PAGE_SIZE) as u64;
            let page_offset = current_offset % PAGE_SIZE;
            let bytes_in_page = (PAGE_SIZE - page_offset).min(total_to_read - bytes_read);

            if let Some(page_data) = self.read_page(inode_id, page_num) {
                let available = page_data.len().saturating_sub(page_offset);
                let to_copy = bytes_in_page.min(available);
                if to_copy > 0 {
                    result[bytes_read..bytes_read + to_copy]
                        .copy_from_slice(&page_data[page_offset..page_offset + to_copy]);
                }
            }

            bytes_read += bytes_in_page;
            current_offset += bytes_in_page;
        }

        inode.touch_atime();
        self.save_inode(&inode);

        Ok(Bytes::from(result))
    }

    pub fn write(&self, handle: u64, offset: u64, data: &[u8]) -> FsResult<usize> {
        let handles = self.handles.lock().unwrap();
        let (inode_id, path, flags) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?
            .clone();
        drop(handles);

        let mut inode = self
            .load_inode(inode_id)
            .ok_or_else(|| FsError::not_found(&path))?;

        if inode.is_directory() {
            return Err(FsError::is_directory(&path));
        }

        let write_offset = if flags.append {
            inode.size as usize
        } else {
            offset as usize
        };

        let mut bytes_written = 0usize;
        let mut current_offset = write_offset;

        while bytes_written < data.len() {
            let page_num = (current_offset / PAGE_SIZE) as u64;
            let page_offset = current_offset % PAGE_SIZE;
            let bytes_to_write = (PAGE_SIZE - page_offset).min(data.len() - bytes_written);

            let mut page_data = self
                .read_page(inode_id, page_num)
                .unwrap_or_else(|| vec![0u8; PAGE_SIZE]);

            if page_data.len() < PAGE_SIZE {
                page_data.resize(PAGE_SIZE, 0);
            }

            page_data[page_offset..page_offset + bytes_to_write]
                .copy_from_slice(&data[bytes_written..bytes_written + bytes_to_write]);

            self.write_page(inode_id, page_num, &page_data);

            bytes_written += bytes_to_write;
            current_offset += bytes_to_write;
        }

        let new_size = (write_offset + data.len()) as u64;
        if new_size > inode.size {
            inode.size = new_size;
            inode.page_count = Self::pages_needed(new_size).max(1);
        }
        inode.touch_mtime();
        self.save_inode(&inode);

        Ok(data.len())
    }

    pub fn close(&self, handle: u64) -> FsResult<()> {
        self.handles
            .lock()
            .unwrap()
            .remove(&handle)
            .map(|_| ())
            .ok_or_else(|| FsError::invalid_handle(handle))
    }

    pub fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let path = self.normalize_path(path);
        let (inode_id, inode) = self.resolve_path(&path)?;

        if !inode.is_directory() {
            return Err(FsError::not_directory(&path));
        }

        let entries = self.list_dir(inode_id);
        let mut result = Vec::with_capacity(entries.len());

        for (name, child_inode_id) in entries {
            if let Some(child_inode) = self.load_inode(child_inode_id) {
                let child_path = if path == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", path, name)
                };

                result.push(FileInfo {
                    path: child_path,
                    size: child_inode.size,
                    file_type: if child_inode.is_directory() {
                        FileType::Directory
                    } else {
                        FileType::Regular
                    },
                    mode: child_inode.mode,
                    uid: self.uid,
                    gid: self.gid,
                    atime: timestamp_to_system_time(child_inode.atime),
                    mtime: timestamp_to_system_time(child_inode.mtime),
                    ctime: timestamp_to_system_time(child_inode.ctime),
                    etag: String::new(),
                    symlink_target: None,
                });
            }
        }

        result.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }

    pub fn remove(&self, path: &str) -> FsResult<()> {
        let path = self.normalize_path(path);
        if path == "/" {
            return Err(FsError::permission_denied("cannot remove root"));
        }

        let (inode_id, inode) = self.resolve_path(&path)?;

        if inode.is_directory() {
            let children = self.list_dir(inode_id);
            if !children.is_empty() {
                return Err(FsError::directory_not_empty(&path));
            }
        } else {
            self.delete_pages(inode_id);
        }

        let (parent_inode, name) = self.resolve_parent(&path)?;
        self.unlink(parent_inode, &name);
        self.delete_inode(inode_id);

        Ok(())
    }

    pub fn wstat(&self, path: &str, changes: &StatChanges) -> FsResult<()> {
        let path = self.normalize_path(path);

        if let Some(new_name) = &changes.name {
            return self.rename(&path, new_name);
        }

        let (inode_id, mut inode) = self.resolve_path(&path)?;

        if let Some(m) = changes.mode {
            inode.mode = m;
        }

        if let Some(new_size) = changes.size {
            if inode.is_directory() {
                return Err(FsError::is_directory(&path));
            }

            let old_page_count = inode.page_count;
            let new_page_count = Self::pages_needed(new_size).max(1);

            if new_page_count < old_page_count {
                for page_num in new_page_count..old_page_count {
                    self.kv.delete(&keys::page(inode_id, page_num));
                }
            } else if new_page_count > old_page_count {
                for page_num in old_page_count..new_page_count {
                    self.write_page(inode_id, page_num, &vec![0u8; PAGE_SIZE]);
                }
            }

            if new_size < inode.size {
                let last_page = new_page_count - 1;
                let page_offset = (new_size % PAGE_SIZE as u64) as usize;
                if page_offset > 0 {
                    if let Some(mut page_data) = self.read_page(inode_id, last_page) {
                        for i in page_offset..PAGE_SIZE {
                            page_data[i] = 0;
                        }
                        self.write_page(inode_id, last_page, &page_data);
                    }
                }
            }

            inode.size = new_size;
            inode.page_count = new_page_count;
        }

        if let Some(atime) = changes.atime {
            inode.atime = systemtime_to_timestamp(atime);
        }

        if let Some(mtime) = changes.mtime {
            inode.mtime = systemtime_to_timestamp(mtime);
        } else {
            inode.touch_mtime();
        }

        self.save_inode(&inode);

        Ok(())
    }

    fn rename(&self, old_path: &str, new_name: &str) -> FsResult<()> {
        let new_path = if new_name.starts_with('/') {
            self.normalize_path(new_name)
        } else {
            let parent = self
                .parent_path(old_path)
                .unwrap_or_else(|| "/".to_string());
            if parent == "/" {
                self.normalize_path(&format!("/{new_name}"))
            } else {
                self.normalize_path(&format!("{parent}/{new_name}"))
            }
        };

        if old_path == new_path {
            return Ok(());
        }

        let (src_inode_id, src_inode) = self.resolve_path(old_path)?;

        if let Ok((dst_inode_id, dst_inode)) = self.resolve_path(&new_path) {
            if dst_inode.is_directory() {
                if !src_inode.is_directory() {
                    return Err(FsError::is_directory(&new_path));
                }
                let entries = self.list_dir(dst_inode_id);
                if !entries.is_empty() {
                    return Err(FsError::directory_not_empty(&new_path));
                }
            } else if src_inode.is_directory() {
                return Err(FsError::not_directory(&new_path));
            }
            if !dst_inode.is_directory() {
                self.delete_pages(dst_inode_id);
            }
            self.delete_inode(dst_inode_id);
        }

        let (old_parent_id, old_name) = self.resolve_parent(old_path)?;
        self.unlink(old_parent_id, &old_name);

        let (new_parent_id, new_entry_name) = self.resolve_parent(&new_path)?;
        self.link(new_parent_id, &new_entry_name, src_inode_id);

        Ok(())
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        if path == "/" {
            return None;
        }
        let path = path.trim_end_matches('/');
        match path.rfind('/') {
            Some(0) => Some("/".to_string()),
            Some(idx) => Some(path[..idx].to_string()),
            None => None,
        }
    }
}
