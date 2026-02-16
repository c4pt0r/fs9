use std::path::{Path, PathBuf};
use std::{ffi::OsString, fs, io::Write};

use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileInfo {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub mode: u32,
    pub mtime: u64,
    pub uid: u32,
    pub gid: u32,
}

pub fn safe_resolve(mount_source: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let canonical_mount = mount_source.canonicalize().map_err(|e| e.to_string())?;
    let joined = mount_source.join(relative_path);

    let resolved = if joined.exists() {
        joined.canonicalize().map_err(|e| e.to_string())?
    } else {
        resolve_nonexistent_path(&canonical_mount, &joined)?
    };

    if resolved.starts_with(&canonical_mount) {
        Ok(resolved)
    } else {
        Err("path escapes mount boundary".to_string())
    }
}

fn resolve_nonexistent_path(canonical_mount: &Path, path: &Path) -> Result<PathBuf, String> {
    let mut existing = path;
    let mut missing_segments: Vec<OsString> = Vec::new();

    while !existing.exists() {
        if let Some(name) = existing.file_name() {
            missing_segments.push(name.to_os_string());
        } else {
            return Err("path escapes mount boundary".to_string());
        }

        existing = existing
            .parent()
            .ok_or_else(|| "path escapes mount boundary".to_string())?;
    }

    let existing_canonical = existing.canonicalize().map_err(|e| e.to_string())?;
    if !existing_canonical.starts_with(canonical_mount) {
        return Err("path escapes mount boundary".to_string());
    }

    let mut rebuilt = existing_canonical;
    for segment in missing_segments.iter().rev() {
        rebuilt.push(segment);
    }

    Ok(rebuilt)
}

pub fn local_stat(path: &Path) -> Result<LocalFileInfo, String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    let mtime = u64::try_from(metadata.mtime()).unwrap_or(0);

    let name = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );

    Ok(LocalFileInfo {
        name,
        size: metadata.len(),
        is_dir: metadata.is_dir(),
        mode: metadata.mode(),
        mtime,
        uid: metadata.uid(),
        gid: metadata.gid(),
    })
}

pub fn local_readdir(path: &Path) -> Result<Vec<LocalFileInfo>, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .map(|entry| {
            let entry = entry.map_err(|e| e.to_string())?;
            local_stat(&entry.path())
        })
        .collect::<Result<Vec<_>, _>>()?;

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

pub fn local_read_file(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| e.to_string())
}

pub fn local_write_file(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    fs::write(path, data).map_err(|e| e.to_string())
}

pub fn local_append_file(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())
}

pub fn local_mkdir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| e.to_string())
}

pub fn local_remove(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if metadata.is_dir() {
        fs::remove_dir(path).map_err(|e| e.to_string())
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

pub fn local_remove_recursive(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

pub fn local_rename(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    fs::rename(from, to).map_err(|e| e.to_string())
}

pub fn local_copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    fs::copy(from, to).map(|_| ()).map_err(|e| e.to_string())
}

pub fn local_chmod(path: &Path, mode: u32) -> Result<(), String> {
    let perms = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, perms).map_err(|e| e.to_string())
}

pub fn local_truncate(path: &Path, size: u64) -> Result<(), String> {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.set_len(size).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "sh9_local_fs_test_{}_{}",
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&path).expect("failed to create temp test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn local_fs_stat_file_returns_correct_info() {
        let temp = TempDirGuard::new();
        let file = temp.path().join("sample.txt");
        fs::write(&file, b"hello").expect("write failed");

        let info = local_stat(&file).expect("stat failed");
        assert_eq!(info.name, "sample.txt");
        assert_eq!(info.size, 5);
        assert!(!info.is_dir);
    }

    #[test]
    fn local_fs_stat_directory_returns_directory_info() {
        let temp = TempDirGuard::new();
        let dir = temp.path().join("subdir");
        fs::create_dir_all(&dir).expect("mkdir failed");

        let info = local_stat(&dir).expect("stat failed");
        assert_eq!(info.name, "subdir");
        assert!(info.is_dir);
    }

    #[test]
    fn local_fs_stat_nonexistent_returns_error() {
        let temp = TempDirGuard::new();
        let file = temp.path().join("missing.txt");

        let err = local_stat(&file).expect_err("expected error");
        assert!(!err.is_empty());
    }

    #[test]
    fn local_fs_readdir_lists_entries_sorted_by_name() {
        let temp = TempDirGuard::new();
        fs::write(temp.path().join("z.txt"), b"z").expect("write failed");
        fs::create_dir_all(temp.path().join("a_dir")).expect("mkdir failed");
        fs::write(temp.path().join("m.txt"), b"m").expect("write failed");

        let entries = local_readdir(temp.path()).expect("readdir failed");
        let names: Vec<String> = entries.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["a_dir", "m.txt", "z.txt"]);
    }

    #[test]
    fn local_fs_read_write_roundtrip() {
        let temp = TempDirGuard::new();
        let file = temp.path().join("roundtrip.txt");

        local_write_file(&file, b"hello world").expect("write failed");
        let read = local_read_file(&file).expect("read failed");
        assert_eq!(read, b"hello world");
    }

    #[test]
    fn local_fs_append_appends_existing_content() {
        let temp = TempDirGuard::new();
        let file = temp.path().join("append.txt");

        local_write_file(&file, b"hello").expect("write failed");
        local_append_file(&file, b" world").expect("append failed");
        let read = local_read_file(&file).expect("read failed");
        assert_eq!(read, b"hello world");
    }

    #[test]
    fn local_fs_mkdir_creates_directory() {
        let temp = TempDirGuard::new();
        let dir = temp.path().join("nested").join("dir");

        local_mkdir(&dir).expect("mkdir failed");
        assert!(dir.is_dir());
    }

    #[test]
    fn local_fs_remove_file_works() {
        let temp = TempDirGuard::new();
        let file = temp.path().join("remove_me.txt");
        fs::write(&file, b"x").expect("write failed");

        local_remove(&file).expect("remove failed");
        assert!(!file.exists());
    }

    #[test]
    fn local_fs_remove_empty_directory_works() {
        let temp = TempDirGuard::new();
        let dir = temp.path().join("emptydir");
        fs::create_dir_all(&dir).expect("mkdir failed");

        local_remove(&dir).expect("remove failed");
        assert!(!dir.exists());
    }

    #[test]
    fn local_fs_rename_within_same_directory() {
        let temp = TempDirGuard::new();
        let from = temp.path().join("old.txt");
        let to = temp.path().join("new.txt");
        fs::write(&from, b"abc").expect("write failed");

        local_rename(&from, &to).expect("rename failed");
        assert!(!from.exists());
        assert_eq!(fs::read(&to).expect("read failed"), b"abc");
    }

    #[test]
    fn local_fs_copy_creates_independent_copy() {
        let temp = TempDirGuard::new();
        let from = temp.path().join("src.txt");
        let to = temp.path().join("dst.txt");
        fs::write(&from, b"original").expect("write failed");

        local_copy(&from, &to).expect("copy failed");
        fs::write(&from, b"changed").expect("write failed");

        assert_eq!(fs::read(&to).expect("read failed"), b"original");
    }

    #[test]
    fn local_fs_safe_resolve_blocks_path_traversal() {
        let temp = TempDirGuard::new();

        let err =
            safe_resolve(temp.path(), "../../etc/passwd").expect_err("expected traversal error");
        assert_eq!(err, "path escapes mount boundary");
    }

    #[test]
    fn local_fs_safe_resolve_allows_valid_relative_path() {
        let temp = TempDirGuard::new();
        let nested = temp.path().join("sub");
        fs::create_dir_all(&nested).expect("mkdir failed");
        fs::write(nested.join("file.txt"), b"ok").expect("write failed");

        let resolved = safe_resolve(temp.path(), "sub/file.txt").expect("resolve failed");
        assert_eq!(
            resolved,
            nested
                .join("file.txt")
                .canonicalize()
                .expect("canonicalize failed")
        );
    }

    #[test]
    fn local_fs_safe_resolve_handles_nonexistent_target_for_write() {
        let temp = TempDirGuard::new();
        let parent = temp.path().join("newdir");
        fs::create_dir_all(&parent).expect("mkdir failed");

        let resolved = safe_resolve(temp.path(), "newdir/newfile.txt").expect("resolve failed");
        assert_eq!(resolved, parent.join("newfile.txt"));
    }

    #[test]
    fn local_fs_remove_recursive_removes_nested_tree() {
        let temp = TempDirGuard::new();
        let root = temp.path().join("tree");
        fs::create_dir_all(root.join("a").join("b")).expect("mkdir failed");
        fs::write(root.join("a").join("b").join("f.txt"), b"x").expect("write failed");

        local_remove_recursive(&root).expect("remove recursive failed");
        assert!(!root.exists());
    }

    #[test]
    fn local_fs_chmod_and_truncate_work() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDirGuard::new();
        let file = temp.path().join("perm.txt");
        fs::write(&file, b"0123456789").expect("write failed");

        local_chmod(&file, 0o640).expect("chmod failed");
        local_truncate(&file, 4).expect("truncate failed");

        let mode = fs::metadata(&file)
            .expect("metadata failed")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o640);
        assert_eq!(fs::metadata(&file).expect("metadata failed").len(), 4);
    }
}
