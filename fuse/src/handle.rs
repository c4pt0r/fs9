use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use fs9_client::FileHandle;

pub struct HandleTable {
    next_fh: AtomicU64,
    fh_to_handle: RwLock<HashMap<u64, HandleEntry>>,
}

struct HandleEntry {
    handle: FileHandle,
    flags: i32,
}

impl HandleTable {
    pub fn new() -> Self {
        Self {
            next_fh: AtomicU64::new(1),
            fh_to_handle: RwLock::new(HashMap::new()),
        }
    }

    pub fn insert(&self, handle: FileHandle, flags: i32) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::SeqCst);
        self.fh_to_handle
            .write()
            .unwrap()
            .insert(fh, HandleEntry { handle, flags });
        fh
    }

    pub fn get(&self, fh: u64) -> Option<FileHandle> {
        self.fh_to_handle
            .read()
            .unwrap()
            .get(&fh)
            .map(|e| e.handle.clone())
    }

    pub fn get_flags(&self, fh: u64) -> Option<i32> {
        self.fh_to_handle.read().unwrap().get(&fh).map(|e| e.flags)
    }

    pub fn remove(&self, fh: u64) -> Option<FileHandle> {
        self.fh_to_handle
            .write()
            .unwrap()
            .remove(&fh)
            .map(|e| e.handle)
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_handle(id: &str) -> FileHandle {
        FileHandle {
            id: id.to_string(),
            path: "/test".to_string(),
            metadata: fs9_client::FileInfo {
                path: "/test".to_string(),
                size: 0,
                file_type: fs9_client::FileType::Regular,
                mode: 0o644,
                uid: 0,
                gid: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                etag: String::new(),
                symlink_target: None,
            },
        }
    }

    #[test]
    fn test_insert_and_get() {
        let table = HandleTable::new();
        let handle = mock_handle("test-1");
        let fh = table.insert(handle, libc::O_RDONLY);

        assert!(table.get(fh).is_some());
        assert_eq!(table.get_flags(fh), Some(libc::O_RDONLY));
    }

    #[test]
    fn test_remove() {
        let table = HandleTable::new();
        let handle = mock_handle("test-2");
        let fh = table.insert(handle, libc::O_RDWR);

        assert!(table.remove(fh).is_some());
        assert!(table.get(fh).is_none());
    }

    #[test]
    fn test_unique_fh() {
        let table = HandleTable::new();
        let fh1 = table.insert(mock_handle("1"), 0);
        let fh2 = table.insert(mock_handle("2"), 0);
        assert_ne!(fh1, fh2);
    }
}
