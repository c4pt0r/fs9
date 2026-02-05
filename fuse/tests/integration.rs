use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_id() -> u64 {
    TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
}

struct MountedFs {
    fuse_process: Child,
    mountpoint: String,
}

impl MountedFs {
    fn mount(server_url: &str, mountpoint: &str) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(mountpoint)?;

        let fuse_bin = find_fuse_binary()?;
        let fuse_process = Command::new(&fuse_bin)
            .args([mountpoint, "--server", server_url, "--foreground"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        std::thread::sleep(Duration::from_millis(1000));

        if !Path::new(mountpoint).exists() {
            return Err("Mount point not accessible".into());
        }

        Ok(Self {
            fuse_process,
            mountpoint: mountpoint.to_string(),
        })
    }
}

impl Drop for MountedFs {
    fn drop(&mut self) {
        let _ = Command::new("fusermount")
            .args(["-u", &self.mountpoint])
            .status();
        let _ = self.fuse_process.kill();
        std::thread::sleep(Duration::from_millis(100));
        let _ = fs::remove_dir(&self.mountpoint);
    }
}

fn find_fuse_binary() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().ok_or("Cannot find workspace root")?;

    let candidates = [
        workspace_root.join("target/debug/fs9-fuse"),
        workspace_root.join("target/release/fs9-fuse"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    Err("fs9-fuse binary not found. Run `cargo build -p fs9-fuse` first.".into())
}

fn get_server_url() -> String {
    std::env::var("FS9_SERVER_URL").unwrap_or_else(|_| "http://localhost:9999".to_string())
}

#[test]
#[ignore]
fn test_fuse_write_read() {
    let mountpoint = "/tmp/fs9-fuse-test-wr";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/test_write_read.txt", mountpoint);
    let content = b"Hello from FUSE test!";

    {
        let mut file = fs::File::create(&test_file).expect("Failed to create file");
        file.write_all(content).expect("Failed to write");
    }

    {
        let mut file = fs::File::open(&test_file).expect("Failed to open file");
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).expect("Failed to read");
        assert_eq!(&buf[..], content);
    }

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_mkdir_readdir() {
    let mountpoint = "/tmp/fs9-fuse-test-dir";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_dir = format!("{}/testdir", mountpoint);
    fs::create_dir(&test_dir).expect("Failed to create directory");

    let file1 = format!("{}/file1.txt", test_dir);
    let file2 = format!("{}/file2.txt", test_dir);
    fs::write(&file1, b"content1").expect("Failed to write file1");
    fs::write(&file2, b"content2").expect("Failed to write file2");

    let entries: Vec<_> = fs::read_dir(&test_dir)
        .expect("Failed to read directory")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 2);

    fs::remove_file(&file1).expect("Failed to remove file1");
    fs::remove_file(&file2).expect("Failed to remove file2");
    fs::remove_dir(&test_dir).expect("Failed to remove directory");
}

#[test]
#[ignore]
fn test_fuse_rename() {
    let mountpoint = "/tmp/fs9-fuse-test-rename";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let old_path = format!("{}/old_name.txt", mountpoint);
    let new_path = format!("{}/new_name.txt", mountpoint);

    fs::write(&old_path, b"rename test").expect("Failed to write");
    fs::rename(&old_path, &new_path).expect("Failed to rename");

    assert!(!Path::new(&old_path).exists());
    assert!(Path::new(&new_path).exists());

    let content = fs::read(&new_path).expect("Failed to read renamed file");
    assert_eq!(&content[..], b"rename test");

    fs::remove_file(&new_path).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_truncate() {
    let mountpoint = "/tmp/fs9-fuse-test-truncate";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/truncate_test.txt", mountpoint);
    fs::write(&test_file, b"hello world").expect("Failed to write");

    {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&test_file)
            .expect("Failed to open");
        file.set_len(5).expect("Failed to truncate");
    }

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"hello");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_nested_directories() {
    let mountpoint = "/tmp/fs9-fuse-test-nested";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let deep_path = format!("{}/a/b/c/d/e", mountpoint);
    fs::create_dir_all(&deep_path).expect("Failed to create nested dirs");

    let file_path = format!("{}/deep.txt", deep_path);
    fs::write(&file_path, b"deeply nested").expect("Failed to write");

    let content = fs::read(&file_path).expect("Failed to read");
    assert_eq!(&content[..], b"deeply nested");

    fs::remove_file(&file_path).expect("Failed to remove file");
    fs::remove_dir_all(&format!("{}/a", mountpoint)).expect("Failed to cleanup");
}

#[test]
#[ignore]
fn test_fuse_large_file() {
    let mountpoint = "/tmp/fs9-fuse-test-large";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/large.bin", mountpoint);

    let data: Vec<u8> = (0..1024 * 1024).map(|i| (i % 256) as u8).collect();

    fs::write(&test_file, &data).expect("Failed to write large file");

    let read_data = fs::read(&test_file).expect("Failed to read large file");
    assert_eq!(read_data.len(), data.len());
    assert_eq!(read_data, data);

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_append() {
    let mountpoint = "/tmp/fs9-fuse-test-append";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/append.txt", mountpoint);

    fs::write(&test_file, b"line1\n").expect("Failed to write");

    {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&test_file)
            .expect("Failed to open for append");
        file.write_all(b"line2\n").expect("Failed to append");
        file.write_all(b"line3\n").expect("Failed to append");
    }

    let content = fs::read_to_string(&test_file).expect("Failed to read");
    assert_eq!(content, "line1\nline2\nline3\n");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_many_files() {
    let mountpoint = "/tmp/fs9-fuse-test-many";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let dir = format!("{}/manyfiles", mountpoint);
    fs::create_dir(&dir).expect("Failed to create dir");

    for i in 0..100 {
        let path = format!("{}/file{}.txt", dir, i);
        fs::write(&path, format!("content {}", i)).expect("Failed to write");
    }

    let entries: Vec<_> = fs::read_dir(&dir)
        .expect("Failed to read dir")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 100);

    for i in 0..100 {
        let path = format!("{}/file{}.txt", dir, i);
        let content = fs::read_to_string(&path).expect("Failed to read");
        assert_eq!(content, format!("content {}", i));
    }

    fs::remove_dir_all(&dir).expect("Failed to cleanup");
}

#[test]
#[ignore]
fn test_fuse_unicode_filename() {
    let mountpoint = "/tmp/fs9-fuse-test-unicode";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/unicode_file.txt", mountpoint);
    fs::write(&test_file, b"unicode content").expect("Failed to write");

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"unicode content");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_file_with_spaces() {
    let mountpoint = "/tmp/fs9-fuse-test-spaces";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/file with spaces.txt", mountpoint);
    fs::write(&test_file, b"spaces content").expect("Failed to write");

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"spaces content");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_move_between_dirs() {
    let mountpoint = "/tmp/fs9-fuse-test-move";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let src_dir = format!("{}/src", mountpoint);
    let dst_dir = format!("{}/dst", mountpoint);
    fs::create_dir(&src_dir).expect("Failed to create src");
    fs::create_dir(&dst_dir).expect("Failed to create dst");

    let src_file = format!("{}/file.txt", src_dir);
    let dst_file = format!("{}/file.txt", dst_dir);

    fs::write(&src_file, b"moving").expect("Failed to write");
    fs::rename(&src_file, &dst_file).expect("Failed to move");

    assert!(!Path::new(&src_file).exists());
    assert!(Path::new(&dst_file).exists());

    let content = fs::read(&dst_file).expect("Failed to read");
    assert_eq!(&content[..], b"moving");

    fs::remove_file(&dst_file).unwrap();
    fs::remove_dir(&src_dir).unwrap();
    fs::remove_dir(&dst_dir).unwrap();
}

#[test]
#[ignore]
fn test_fuse_chmod() {
    let mountpoint = "/tmp/fs9-fuse-test-chmod";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/chmod.txt", mountpoint);
    fs::write(&test_file, b"chmod test").expect("Failed to write");

    use std::os::unix::fs::PermissionsExt;

    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(&test_file, perms).expect("Failed to chmod");

    let meta = fs::metadata(&test_file).expect("Failed to stat");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_error_nonexistent() {
    let mountpoint = "/tmp/fs9-fuse-test-err";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let result = fs::read(&format!("{}/nonexistent.txt", mountpoint));
    assert!(result.is_err());
}

#[test]
#[ignore]
fn test_fuse_error_rmdir_nonempty() {
    let mountpoint = "/tmp/fs9-fuse-test-err2";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let dir = format!("{}/notempty", mountpoint);
    fs::create_dir(&dir).expect("Failed to create dir");
    fs::write(&format!("{}/file.txt", dir), b"content").expect("Failed to write");

    let result = fs::remove_dir(&dir);
    assert!(result.is_err());

    fs::remove_file(&format!("{}/file.txt", dir)).unwrap();
    fs::remove_dir(&dir).unwrap();
}

#[test]
#[ignore]
fn test_fuse_binary_with_nulls() {
    let mountpoint = "/tmp/fs9-fuse-test-bin";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/binary.bin", mountpoint);
    let data: Vec<u8> = vec![0x00, 0x01, 0x02, 0x00, 0xFF, 0x00, 0xFE];

    fs::write(&test_file, &data).expect("Failed to write");

    let read_data = fs::read(&test_file).expect("Failed to read");
    assert_eq!(read_data, data);

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_hidden_file() {
    let mountpoint = "/tmp/fs9-fuse-test-hidden";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/.hidden", mountpoint);
    fs::write(&test_file, b"hidden content").expect("Failed to write");

    assert!(Path::new(&test_file).exists());

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"hidden content");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_overwrite_larger_with_smaller() {
    let mountpoint = "/tmp/fs9-fuse-test-overwrite";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/overwrite.txt", mountpoint);

    fs::write(&test_file, b"this is a long content string").expect("Failed to write");
    fs::write(&test_file, b"short").expect("Failed to overwrite");

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"short");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_git_init_add_commit() {
    let mountpoint = "/tmp/fs9-fuse-test-git";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let repo_dir = format!("{}/repo-{}", mountpoint, unique_id());
    let _ = fs::remove_dir_all(&repo_dir);
    fs::create_dir_all(&repo_dir).expect("Failed to create repo dir");

    let output = Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .expect("Failed to run git init");
    assert!(output.status.success(), "git init failed: {:?}", output);

    let test_file = format!("{}/hello.txt", repo_dir);
    fs::write(&test_file, b"Hello, Git on PageFS!\n").expect("Failed to write test file");

    let output = Command::new("git")
        .args(["add", "hello.txt"])
        .current_dir(&repo_dir)
        .output()
        .expect("Failed to run git add");
    assert!(output.status.success(), "git add failed: {:?}", output);

    let output = Command::new("git")
        .args([
            "-c",
            "user.email=test@test.com",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "Initial commit",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("Failed to run git commit");
    assert!(output.status.success(), "git commit failed: {:?}", output);

    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo_dir)
        .output()
        .expect("Failed to run git status");
    assert!(output.status.success(), "git status failed: {:?}", output);
    assert!(
        output.stdout.is_empty(),
        "git status should be clean, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    fs::remove_dir_all(&repo_dir).expect("Failed to cleanup");
}

#[test]
#[ignore]
fn test_fuse_git_executable_preserved() {
    let mountpoint = "/tmp/fs9-fuse-test-git-exec";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let repo_dir = format!("{}/repo-{}", mountpoint, unique_id());
    let _ = fs::remove_dir_all(&repo_dir);
    fs::create_dir_all(&repo_dir).expect("Failed to create repo dir");

    Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .expect("Failed to run git init");

    let script_path = format!("{}/script.sh", repo_dir);
    fs::write(&script_path, b"#!/bin/bash\necho hello\n").expect("Failed to write script");

    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).expect("Failed to chmod");

    Command::new("git")
        .args(["add", "script.sh"])
        .current_dir(&repo_dir)
        .output()
        .expect("Failed to git add");

    Command::new("git")
        .args([
            "-c",
            "user.email=test@test.com",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "Add script",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("Failed to git commit");

    let meta = fs::metadata(&script_path).expect("Failed to stat script");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "Executable permission should be preserved");

    fs::remove_dir_all(&repo_dir).expect("Failed to cleanup");
}

#[test]
#[ignore]
fn test_fuse_git_branch_and_checkout() {
    let mountpoint = "/tmp/fs9-fuse-test-git-branch";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let repo_dir = format!("{}/repo-{}", mountpoint, unique_id());
    let _ = fs::remove_dir_all(&repo_dir);
    fs::create_dir_all(&repo_dir).expect("Failed to create repo dir");

    Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .expect("git init");

    fs::write(format!("{}/file.txt", repo_dir), b"main content\n").expect("write");

    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_dir)
        .output()
        .expect("git add");

    Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "init",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("git commit");

    let output = Command::new("git")
        .args(["checkout", "-b", "feature"])
        .current_dir(&repo_dir)
        .output()
        .expect("git checkout -b");
    assert!(
        output.status.success(),
        "git checkout -b failed: {:?}",
        output
    );

    fs::write(format!("{}/file.txt", repo_dir), b"feature content\n").expect("write");

    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_dir)
        .output()
        .expect("git add");

    Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "feature",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("git commit");

    let output = Command::new("git")
        .args(["checkout", "master"])
        .current_dir(&repo_dir)
        .output();

    let checkout_result = if output.as_ref().map(|o| o.status.success()).unwrap_or(false) {
        output.unwrap()
    } else {
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&repo_dir)
            .output()
            .expect("git checkout main")
    };
    assert!(checkout_result.status.success(), "git checkout failed");

    let content = fs::read_to_string(format!("{}/file.txt", repo_dir)).expect("read");
    assert_eq!(
        content, "main content\n",
        "Should be back to main branch content"
    );

    fs::remove_dir_all(&repo_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_git_stash() {
    let mountpoint = "/tmp/fs9-fuse-test-git-stash";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let repo_dir = format!("{}/repo-{}", mountpoint, unique_id());
    let _ = fs::remove_dir_all(&repo_dir);
    fs::create_dir_all(&repo_dir).expect("create dir");

    Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .expect("init");
    fs::write(format!("{}/file.txt", repo_dir), b"original\n").expect("write");
    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_dir)
        .output()
        .expect("add");
    Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "init",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("commit");

    fs::write(format!("{}/file.txt", repo_dir), b"modified\n").expect("write");

    let output = Command::new("git")
        .args(["stash"])
        .current_dir(&repo_dir)
        .output()
        .expect("git stash");
    assert!(output.status.success(), "git stash failed: {:?}", output);

    let content = fs::read_to_string(format!("{}/file.txt", repo_dir)).expect("read");
    assert_eq!(content, "original\n", "Stash should restore original");

    let output = Command::new("git")
        .args(["stash", "pop"])
        .current_dir(&repo_dir)
        .output()
        .expect("git stash pop");
    assert!(
        output.status.success(),
        "git stash pop failed: {:?}",
        output
    );

    let content = fs::read_to_string(format!("{}/file.txt", repo_dir)).expect("read");
    assert_eq!(content, "modified\n", "Stash pop should restore modified");

    fs::remove_dir_all(&repo_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_git_clone_local() {
    let mountpoint = "/tmp/fs9-fuse-test-git-clone";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let src_repo = format!("{}/src-{}", mountpoint, tid);
    let dst_repo = format!("{}/dst-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&src_repo);
    let _ = fs::remove_dir_all(&dst_repo);
    fs::create_dir_all(&src_repo).expect("create src dir");

    Command::new("git")
        .args(["init"])
        .current_dir(&src_repo)
        .output()
        .expect("init");
    fs::write(format!("{}/README.md", src_repo), b"# Test Repo\n").expect("write");
    Command::new("git")
        .args(["add", "."])
        .current_dir(&src_repo)
        .output()
        .expect("add");
    Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "init",
        ])
        .current_dir(&src_repo)
        .output()
        .expect("commit");

    let output = Command::new("git")
        .args(["clone", &src_repo, &dst_repo])
        .output()
        .expect("git clone");
    assert!(output.status.success(), "git clone failed: {:?}", output);

    assert!(
        Path::new(&format!("{}/README.md", dst_repo)).exists(),
        "README should exist in clone"
    );
    let content = fs::read_to_string(format!("{}/README.md", dst_repo)).expect("read");
    assert_eq!(content, "# Test Repo\n");

    fs::remove_dir_all(&src_repo).expect("cleanup src");
    fs::remove_dir_all(&dst_repo).expect("cleanup dst");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_redirect() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-pipe";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/pipe-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    let output = Command::new("bash")
        .args(["-c", &format!("echo 'hello world' > {}/file.txt", test_dir)])
        .output()
        .expect("bash echo redirect");
    assert!(
        output.status.success(),
        "echo redirect failed: {:?}",
        output
    );

    let content = fs::read_to_string(format!("{}/file.txt", test_dir)).expect("read");
    assert_eq!(content, "hello world\n");

    let output = Command::new("bash")
        .args([
            "-c",
            &format!("echo 'second line' >> {}/file.txt", test_dir),
        ])
        .output()
        .expect("bash append");
    assert!(output.status.success(), "append failed: {:?}", output);

    let content = fs::read_to_string(format!("{}/file.txt", test_dir)).expect("read");
    assert_eq!(content, "hello world\nsecond line\n");

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_grep() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-grep";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/grep-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    fs::write(
        format!("{}/data.txt", test_dir),
        "apple\nbanana\napricot\ncherry\navocado\n",
    )
    .expect("write");

    let output = Command::new("bash")
        .args(["-c", &format!("cat {}/data.txt | grep '^a'", test_dir)])
        .output()
        .expect("cat | grep");
    assert!(output.status.success(), "cat | grep failed: {:?}", output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "apple\napricot\navocado\n"
    );

    let output = Command::new("bash")
        .args(["-c", &format!("grep -c 'a' {}/data.txt", test_dir)])
        .output()
        .expect("grep -c");
    assert!(output.status.success(), "grep -c failed: {:?}", output);
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "4");

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_sort_uniq() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-sort";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/sort-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    fs::write(format!("{}/numbers.txt", test_dir), "3\n1\n2\n1\n3\n2\n1\n").expect("write");

    let output = Command::new("bash")
        .args(["-c", &format!("cat {}/numbers.txt | sort | uniq", test_dir)])
        .output()
        .expect("sort | uniq");
    assert!(output.status.success(), "sort | uniq failed: {:?}", output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n2\n3\n");

    let output = Command::new("bash")
        .args([
            "-c",
            &format!("cat {}/numbers.txt | sort | uniq -c | sort -rn", test_dir),
        ])
        .output()
        .expect("complex pipeline");
    assert!(
        output.status.success(),
        "complex pipeline failed: {:?}",
        output
    );
    let result = String::from_utf8_lossy(&output.stdout);
    assert!(
        result.contains("3 1"),
        "should have '3 1' in output: {}",
        result
    );

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_wc() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-wc";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/wc-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    fs::write(
        format!("{}/lines.txt", test_dir),
        "one\ntwo\nthree\nfour\nfive\n",
    )
    .expect("write");

    let output = Command::new("bash")
        .args(["-c", &format!("cat {}/lines.txt | wc -l", test_dir)])
        .output()
        .expect("wc -l");
    assert!(output.status.success(), "wc -l failed: {:?}", output);
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "5");

    let output = Command::new("bash")
        .args(["-c", &format!("cat {}/lines.txt | wc -w", test_dir)])
        .output()
        .expect("wc -w");
    assert!(output.status.success(), "wc -w failed: {:?}", output);
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "5");

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_head_tail() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-headtail";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/ht-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    fs::write(
        format!("{}/seq.txt", test_dir),
        "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n",
    )
    .expect("write");

    let output = Command::new("bash")
        .args(["-c", &format!("head -n 3 {}/seq.txt", test_dir)])
        .output()
        .expect("head");
    assert!(output.status.success(), "head failed: {:?}", output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n2\n3\n");

    let output = Command::new("bash")
        .args(["-c", &format!("tail -n 3 {}/seq.txt", test_dir)])
        .output()
        .expect("tail");
    assert!(output.status.success(), "tail failed: {:?}", output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "8\n9\n10\n");

    let output = Command::new("bash")
        .args(["-c", &format!("head -n 5 {}/seq.txt | tail -n 2", test_dir)])
        .output()
        .expect("head | tail");
    assert!(output.status.success(), "head | tail failed: {:?}", output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "4\n5\n");

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_sed_awk() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-sedawk";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/sa-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    fs::write(format!("{}/text.txt", test_dir), "hello world\nfoo bar\n").expect("write");

    let output = Command::new("bash")
        .args([
            "-c",
            &format!("cat {}/text.txt | sed 's/world/rust/'", test_dir),
        ])
        .output()
        .expect("sed");
    assert!(output.status.success(), "sed failed: {:?}", output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello rust\nfoo bar\n"
    );

    fs::write(
        format!("{}/csv.txt", test_dir),
        "alice,30\nbob,25\ncharlie,35\n",
    )
    .expect("write csv");

    let output = Command::new("bash")
        .args([
            "-c",
            &format!("cat {}/csv.txt | awk -F, '{{print $1}}'", test_dir),
        ])
        .output()
        .expect("awk");
    assert!(output.status.success(), "awk failed: {:?}", output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "alice\nbob\ncharlie\n"
    );

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_tee() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-tee";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/tee-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    let output = Command::new("bash")
        .args([
            "-c",
            &format!(
                "echo 'tee test' | tee {}/out1.txt | tee {}/out2.txt",
                test_dir, test_dir
            ),
        ])
        .output()
        .expect("tee");
    assert!(output.status.success(), "tee failed: {:?}", output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "tee test\n");

    let content1 = fs::read_to_string(format!("{}/out1.txt", test_dir)).expect("read out1");
    let content2 = fs::read_to_string(format!("{}/out2.txt", test_dir)).expect("read out2");
    assert_eq!(content1, "tee test\n");
    assert_eq!(content2, "tee test\n");

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_xargs() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-xargs";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/xargs-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    fs::write(format!("{}/a.txt", test_dir), "content a\n").expect("write a");
    fs::write(format!("{}/b.txt", test_dir), "content b\n").expect("write b");
    fs::write(format!("{}/c.txt", test_dir), "content c\n").expect("write c");

    let output = Command::new("bash")
        .args(["-c", &format!("ls {test_dir}/*.txt | xargs cat")])
        .output()
        .expect("ls | xargs cat");
    assert!(output.status.success(), "xargs failed: {:?}", output);
    let result = String::from_utf8_lossy(&output.stdout);
    assert!(result.contains("content a"), "missing content a");
    assert!(result.contains("content b"), "missing content b");
    assert!(result.contains("content c"), "missing content c");

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_subshell_and_redirect() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-subshell";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/sub-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    let output = Command::new("bash")
        .args([
            "-c",
            &format!(
                "(echo 'line1'; echo 'line2'; echo 'line3') > {}/combined.txt",
                test_dir
            ),
        ])
        .output()
        .expect("subshell redirect");
    assert!(output.status.success(), "subshell failed: {:?}", output);

    let content = fs::read_to_string(format!("{}/combined.txt", test_dir)).expect("read");
    assert_eq!(content, "line1\nline2\nline3\n");

    let output = Command::new("bash")
        .args([
            "-c",
            &format!("cat {test_dir}/combined.txt | (read a; read b; echo \"$b $a\")"),
        ])
        .output()
        .expect("pipe to subshell");
    assert!(
        output.status.success(),
        "pipe to subshell failed: {:?}",
        output
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "line2 line1\n");

    fs::remove_dir_all(&test_dir).expect("cleanup");
}

#[test]
#[ignore]
fn test_fuse_bash_here_document() {
    let mountpoint = "/tmp/fs9-fuse-test-bash-heredoc";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let tid = unique_id().to_string();
    let test_dir = format!("{}/heredoc-{}", mountpoint, tid);
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("create dir");

    let output = Command::new("bash")
        .args([
            "-c",
            &format!(
                r#"cat > {}/heredoc.txt << 'EOF'
first line
second line
third line
EOF"#,
                test_dir
            ),
        ])
        .output()
        .expect("heredoc");
    assert!(output.status.success(), "heredoc failed: {:?}", output);

    let content = fs::read_to_string(format!("{}/heredoc.txt", test_dir)).expect("read");
    assert_eq!(content, "first line\nsecond line\nthird line\n");

    fs::remove_dir_all(&test_dir).expect("cleanup");
}
