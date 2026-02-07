mod common;

use common::{get_server_url, unique_id, MountedFs};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
#[ignore]
fn test_fuse_git_init_add_commit() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-git-init-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let repo_dir = format!("{}/repo-{}", mountpoint, id);
    fs::create_dir(&repo_dir).expect("Failed to create repo dir");

    // git init
    let output = Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .expect("git init failed");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Write a file
    let readme = format!("{}/README.md", repo_dir);
    fs::write(&readme, "# Test Repo\n").expect("Failed to write README");

    // git add
    let output = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&repo_dir)
        .output()
        .expect("git add failed");
    assert!(
        output.status.success(),
        "git add failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // git commit
    let output = Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "initial commit",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("git commit failed");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify clean status
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo_dir)
        .output()
        .expect("git status failed");
    assert!(output.status.success());
    let status = String::from_utf8_lossy(&output.stdout);
    assert!(
        status.trim().is_empty(),
        "Working tree not clean: {}",
        status
    );
}

#[test]
#[ignore]
fn test_fuse_git_executable_preserved() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-git-exec-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let repo_dir = format!("{}/repo-exec-{}", mountpoint, id);
    fs::create_dir(&repo_dir).expect("Failed to create repo dir");

    // git init
    let output = Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .expect("git init failed");
    assert!(output.status.success());

    // Write a script and make executable
    let script = format!("{}/run.sh", repo_dir);
    fs::write(&script, "#!/bin/bash\necho hello\n").expect("Failed to write script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("Failed to chmod");

    // git add + commit
    let output = Command::new("git")
        .args(["add", "run.sh"])
        .current_dir(&repo_dir)
        .output()
        .expect("git add failed");
    assert!(output.status.success());

    let output = Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "add script",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("git commit failed");
    assert!(output.status.success());

    // Verify permissions preserved
    let meta = fs::metadata(&script).expect("Failed to stat script");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "Expected 755, got {:o}", mode);
}

#[test]
#[ignore]
fn test_fuse_git_branch_and_checkout() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-git-branch-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let repo_dir = format!("{}/repo-branch-{}", mountpoint, id);
    fs::create_dir(&repo_dir).expect("Failed to create repo dir");

    // git init + initial commit on main
    let output = Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .expect("git init failed");
    assert!(output.status.success());

    let readme = format!("{}/README.md", repo_dir);
    fs::write(&readme, "main content\n").expect("Failed to write");

    let output = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&repo_dir)
        .output()
        .expect("git add failed");
    assert!(output.status.success());

    let output = Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "main commit",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("git commit failed");
    assert!(output.status.success());

    // Create feature branch and modify
    let output = Command::new("git")
        .args(["checkout", "-b", "feature"])
        .current_dir(&repo_dir)
        .output()
        .expect("git checkout -b failed");
    assert!(output.status.success());

    fs::write(&readme, "feature content\n").expect("Failed to write feature");

    let output = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&repo_dir)
        .output()
        .expect("git add failed");
    assert!(output.status.success());

    let output = Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "feature commit",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("git commit failed");
    assert!(output.status.success());

    // Checkout back to main, verify original content
    let output = Command::new("git")
        .args(["checkout", "master"])
        .current_dir(&repo_dir)
        .output()
        .unwrap_or_else(|_| {
            Command::new("git")
                .args(["checkout", "main"])
                .current_dir(&repo_dir)
                .output()
                .expect("git checkout main/master failed")
        });
    // Try master first, fall back to main
    if !output.status.success() {
        let output2 = Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&repo_dir)
            .output()
            .expect("git checkout main failed");
        assert!(
            output2.status.success(),
            "git checkout main failed: {}",
            String::from_utf8_lossy(&output2.stderr)
        );
    }

    let content = fs::read_to_string(&readme).expect("Failed to read");
    assert_eq!(content, "main content\n");
}

#[test]
#[ignore]
fn test_fuse_git_stash() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-git-stash-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let repo_dir = format!("{}/repo-stash-{}", mountpoint, id);
    fs::create_dir(&repo_dir).expect("Failed to create repo dir");

    // git init + initial commit
    let output = Command::new("git")
        .args(["init"])
        .current_dir(&repo_dir)
        .output()
        .expect("git init failed");
    assert!(output.status.success());

    let file = format!("{}/data.txt", repo_dir);
    fs::write(&file, "original\n").expect("Failed to write");

    let output = Command::new("git")
        .args(["add", "data.txt"])
        .current_dir(&repo_dir)
        .output()
        .expect("git add failed");
    assert!(output.status.success());

    let output = Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "initial",
        ])
        .current_dir(&repo_dir)
        .output()
        .expect("git commit failed");
    assert!(output.status.success());

    // Modify and stash
    fs::write(&file, "modified\n").expect("Failed to write modified");

    let output = Command::new("git")
        .args(["stash"])
        .current_dir(&repo_dir)
        .output()
        .expect("git stash failed");
    assert!(output.status.success());

    // Verify original content restored
    let content = fs::read_to_string(&file).expect("Failed to read");
    assert_eq!(content, "original\n");

    // Pop stash and verify modified content
    let output = Command::new("git")
        .args(["stash", "pop"])
        .current_dir(&repo_dir)
        .output()
        .expect("git stash pop failed");
    assert!(output.status.success());

    let content = fs::read_to_string(&file).expect("Failed to read");
    assert_eq!(content, "modified\n");
}

#[test]
#[ignore]
fn test_fuse_git_clone_local() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-git-clone-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let src_dir = format!("{}/src-repo-{}", mountpoint, id);
    let dst_dir = format!("{}/dst-repo-{}", mountpoint, id);
    fs::create_dir(&src_dir).expect("Failed to create src dir");

    // git init + commit in src
    let output = Command::new("git")
        .args(["init"])
        .current_dir(&src_dir)
        .output()
        .expect("git init failed");
    assert!(output.status.success());

    let readme = format!("{}/README.md", src_dir);
    fs::write(&readme, "# Cloneable\n").expect("Failed to write");

    let output = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&src_dir)
        .output()
        .expect("git add failed");
    assert!(output.status.success());

    let output = Command::new("git")
        .args([
            "-c",
            "user.email=t@t.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "initial",
        ])
        .current_dir(&src_dir)
        .output()
        .expect("git commit failed");
    assert!(output.status.success());

    // Clone
    let output = Command::new("git")
        .args(["clone", &src_dir, &dst_dir])
        .output()
        .expect("git clone failed");
    assert!(
        output.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify README exists in clone
    let cloned_readme = format!("{}/README.md", dst_dir);
    assert!(
        std::path::Path::new(&cloned_readme).exists(),
        "README.md not found in clone"
    );

    let content = fs::read_to_string(&cloned_readme).expect("Failed to read cloned README");
    assert_eq!(content, "# Cloneable\n");
}
