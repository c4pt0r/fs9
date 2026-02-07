mod common;

use common::{get_server_url, unique_id, MountedFs};
use std::fs;
use std::process::Command;

fn bash(script: &str, dir: &str) -> (bool, String, String) {
    let output = Command::new("bash")
        .args(["-c", script])
        .current_dir(dir)
        .output()
        .expect("Failed to run bash");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_redirect() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-redir-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/redir-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    // Write with >
    let (ok, _, err) = bash("echo 'hello' > out.txt", &dir);
    assert!(ok, "echo > failed: {}", err);

    let content = fs::read_to_string(format!("{}/out.txt", dir)).expect("Failed to read");
    assert_eq!(content.trim(), "hello");

    // Append with >>
    let (ok, _, err) = bash("echo 'world' >> out.txt", &dir);
    assert!(ok, "echo >> failed: {}", err);

    let content = fs::read_to_string(format!("{}/out.txt", dir)).expect("Failed to read");
    assert_eq!(content.trim(), "hello\nworld");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_grep() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-grep-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/grep-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    fs::write(
        format!("{}/data.txt", dir),
        "apple\nbanana\napricot\ncherry\navocado\n",
    )
    .expect("Failed to write");

    // cat | grep
    let (ok, stdout, err) = bash("cat data.txt | grep '^a'", &dir);
    assert!(ok, "grep failed: {}", err);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines, vec!["apple", "apricot", "avocado"]);

    // grep -c
    let (ok, stdout, err) = bash("grep -c '^a' data.txt", &dir);
    assert!(ok, "grep -c failed: {}", err);
    assert_eq!(stdout.trim(), "3");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_sort_uniq() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-sort-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/sort-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    fs::write(
        format!("{}/data.txt", dir),
        "banana\napple\nbanana\ncherry\napple\napple\n",
    )
    .expect("Failed to write");

    // sort | uniq
    let (ok, stdout, err) = bash("sort data.txt | uniq", &dir);
    assert!(ok, "sort|uniq failed: {}", err);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines, vec!["apple", "banana", "cherry"]);

    // sort | uniq -c | sort -rn (most frequent first)
    let (ok, stdout, err) = bash("sort data.txt | uniq -c | sort -rn", &dir);
    assert!(ok, "sort|uniq -c failed: {}", err);
    let first_line = stdout.trim().lines().next().unwrap();
    assert!(
        first_line.contains("apple"),
        "Expected apple as most frequent, got: {}",
        first_line
    );
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_wc() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-wc-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/wc-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    fs::write(
        format!("{}/data.txt", dir),
        "one two\nthree four five\nsix\n",
    )
    .expect("Failed to write");

    // wc -l
    let (ok, stdout, err) = bash("wc -l < data.txt", &dir);
    assert!(ok, "wc -l failed: {}", err);
    assert_eq!(stdout.trim(), "3");

    // wc -w
    let (ok, stdout, err) = bash("wc -w < data.txt", &dir);
    assert!(ok, "wc -w failed: {}", err);
    assert_eq!(stdout.trim(), "6");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_head_tail() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-ht-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/ht-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    let lines: String = (1..=10).map(|i| format!("line{}\n", i)).collect();
    fs::write(format!("{}/data.txt", dir), &lines).expect("Failed to write");

    // head -n 3
    let (ok, stdout, err) = bash("head -n 3 data.txt", &dir);
    assert!(ok, "head failed: {}", err);
    let result: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(result, vec!["line1", "line2", "line3"]);

    // tail -n 3
    let (ok, stdout, err) = bash("tail -n 3 data.txt", &dir);
    assert!(ok, "tail failed: {}", err);
    let result: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(result, vec!["line8", "line9", "line10"]);

    // head | tail (get lines 4-6)
    let (ok, stdout, err) = bash("head -n 6 data.txt | tail -n 3", &dir);
    assert!(ok, "head|tail failed: {}", err);
    let result: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(result, vec!["line4", "line5", "line6"]);
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_sed_awk() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-sedawk-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/sedawk-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    fs::write(
        format!("{}/data.csv", dir),
        "alice,30\nbob,25\ncharlie,35\n",
    )
    .expect("Failed to write");

    // sed
    let (ok, stdout, err) = bash("sed 's/,/ is /' data.csv", &dir);
    assert!(ok, "sed failed: {}", err);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines, vec!["alice is 30", "bob is 25", "charlie is 35"]);

    // awk -F
    let (ok, stdout, err) = bash("awk -F, '{print $1}' data.csv", &dir);
    assert!(ok, "awk failed: {}", err);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines, vec!["alice", "bob", "charlie"]);
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_tee() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-tee-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/tee-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    // tee to 2 files
    let (ok, stdout, err) = bash("echo 'tee test' | tee out1.txt out2.txt", &dir);
    assert!(ok, "tee failed: {}", err);
    assert_eq!(stdout.trim(), "tee test");

    let c1 = fs::read_to_string(format!("{}/out1.txt", dir)).expect("Failed to read out1");
    let c2 = fs::read_to_string(format!("{}/out2.txt", dir)).expect("Failed to read out2");
    assert_eq!(c1.trim(), "tee test");
    assert_eq!(c2.trim(), "tee test");
}

#[test]
#[ignore]
fn test_fuse_bash_pipe_xargs() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-xargs-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/xargs-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    fs::write(format!("{}/a.txt", dir), "AAA").expect("Failed to write a");
    fs::write(format!("{}/b.txt", dir), "BBB").expect("Failed to write b");

    // ls | xargs cat
    let (ok, stdout, err) = bash("ls *.txt | sort | xargs cat", &dir);
    assert!(ok, "xargs failed: {}", err);
    assert!(stdout.contains("AAA"), "Missing AAA in output: {}", stdout);
    assert!(stdout.contains("BBB"), "Missing BBB in output: {}", stdout);
}

#[test]
#[ignore]
fn test_fuse_bash_subshell_and_redirect() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-sub-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/sub-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    // (echo; echo; echo) > file
    let (ok, _, err) = bash(
        "(echo 'line1'; echo 'line2'; echo 'line3') > combined.txt",
        &dir,
    );
    assert!(ok, "subshell redirect failed: {}", err);

    let content = fs::read_to_string(format!("{}/combined.txt", dir)).expect("Failed to read");
    let lines: Vec<&str> = content.trim().lines().collect();
    assert_eq!(lines, vec!["line1", "line2", "line3"]);

    // Pipe to subshell
    let (ok, stdout, err) = bash(
        "echo 'hello world' | (read line; echo \"got: $line\")",
        &dir,
    );
    assert!(ok, "pipe to subshell failed: {}", err);
    assert_eq!(stdout.trim(), "got: hello world");
}

#[test]
#[ignore]
fn test_fuse_bash_here_document() {
    let id = unique_id();
    let mountpoint = format!("/tmp/fs9-fuse-test-bash-heredoc-{}", id);
    let _mount = MountedFs::mount(&get_server_url(), &mountpoint).expect("Failed to mount");

    let dir = format!("{}/heredoc-{}", mountpoint, id);
    fs::create_dir(&dir).expect("Failed to create dir");

    // cat > file << 'EOF'
    let script = r#"cat > doc.txt << 'EOF'
Hello from heredoc
Line two
Line three
EOF"#;
    let (ok, _, err) = bash(script, &dir);
    assert!(ok, "heredoc failed: {}", err);

    let content = fs::read_to_string(format!("{}/doc.txt", dir)).expect("Failed to read");
    let lines: Vec<&str> = content.trim().lines().collect();
    assert_eq!(lines, vec!["Hello from heredoc", "Line two", "Line three"]);
}
