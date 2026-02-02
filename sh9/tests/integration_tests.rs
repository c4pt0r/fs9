//! Integration tests for sh9
//!
//! This test harness:
//! 1. Starts an FS9 server on a free port
//! 2. Discovers all .sh9 test scripts
//! 3. Runs each script and compares output with expected .out file
//! 4. Reports differences

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Test server management
struct TestServer {
    #[allow(dead_code)]
    process: Child,
    url: String,
}

impl TestServer {
    fn start() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let server_bin = find_server_binary()?;
        let port = find_free_port()?;
        let url = format!("http://127.0.0.1:{}", port);

        // Find plugins directory
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir.parent().ok_or("Cannot find workspace root")?;
        let plugins_dir = workspace_root.join("plugins");

        let process = Command::new(&server_bin)
            .env("FS9_PORT", port.to_string())
            .env("FS9_HOST", "127.0.0.1")
            .env("FS9_PLUGIN_DIR", plugins_dir)
            .env("RUST_LOG", "warn")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start server at {:?}: {}", server_bin, e))?;

        let server = Self { process, url };
        server.wait_ready()?;
        Ok(server)
    }

    fn wait_ready(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()?;

        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            if client.get(&format!("{}/health", self.url)).send().is_ok() {
                return Ok(());
            }
        }

        Err("Server failed to start within 5 seconds".into())
    }
}

fn find_server_binary() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().ok_or("Cannot find workspace root")?;

    let candidates = [
        workspace_root.join("target/debug/fs9-server"),
        workspace_root.join("target/release/fs9-server"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    Err(format!(
        "fs9-server binary not found. Run `cargo build -p fs9-server` first. Checked: {:?}",
        candidates
    )
    .into())
}

fn find_sh9_binary() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().ok_or("Cannot find workspace root")?;

    let candidates = [
        workspace_root.join("target/debug/sh9"),
        workspace_root.join("target/release/sh9"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    Err(format!(
        "sh9 binary not found. Run `cargo build -p sh9` first. Checked: {:?}",
        candidates
    )
    .into())
}

fn find_free_port() -> Result<u16, Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Discover all .sh9 test scripts
fn discover_test_scripts() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let scripts_dir = manifest_dir.join("tests/integration/scripts");

    if !scripts_dir.exists() {
        return Vec::new();
    }

    let mut scripts = Vec::new();
    if let Ok(entries) = fs::read_dir(&scripts_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "sh9") {
                scripts.push(path);
            }
        }
    }

    scripts.sort();
    scripts
}

/// Run a single test script and compare output
fn run_test_script(
    sh9_bin: &PathBuf,
    script_path: &PathBuf,
    server_url: &str,
) -> Result<TestResult, Box<dyn std::error::Error>> {
    let expected_path = script_path.with_extension("out");

    // Read expected output
    let expected = if expected_path.exists() {
        fs::read_to_string(&expected_path)?
    } else {
        return Ok(TestResult::Skipped {
            reason: format!("Missing expected output file: {:?}", expected_path),
        });
    };

    // Run sh9 with the script
    let output = Command::new(sh9_bin)
        .arg(script_path)
        .env("FS9_SERVER_URL", server_url)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Compare output with pattern matching support
    if matches_pattern(&expected, &stdout) {
        Ok(TestResult::Passed)
    } else {
        Ok(TestResult::Failed {
            expected,
            actual: stdout,
            stderr,
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

#[derive(Debug)]
enum TestResult {
    Passed,
    Failed {
        expected: String,
        actual: String,
        stderr: String,
        exit_code: i32,
    },
    Skipped {
        reason: String,
    },
}

#[test]
fn integration_tests() {
    // Discover test scripts
    let scripts = discover_test_scripts();
    if scripts.is_empty() {
        println!("No test scripts found in tests/integration/scripts/");
        println!("This is expected for TDD - scripts will be added before implementation.");
        return;
    }

    // Start server
    let server = match TestServer::start() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to start test server: {}", e);
            eprintln!("Make sure to build the server first: cargo build -p fs9-server");
            panic!("Cannot run integration tests without server");
        }
    };

    // Find sh9 binary
    let sh9_bin = match find_sh9_binary() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to find sh9 binary: {}", e);
            eprintln!("Make sure to build sh9 first: cargo build -p sh9");
            panic!("Cannot run integration tests without sh9");
        }
    };

    // Run each test
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    for script in &scripts {
        let name = script.file_stem().unwrap().to_string_lossy();
        print!("Running {}... ", name);

        match run_test_script(&sh9_bin, script, &server.url) {
            Ok(TestResult::Passed) => {
                println!("PASSED");
                passed += 1;
            }
            Ok(TestResult::Failed {
                expected,
                actual,
                stderr,
                exit_code,
            }) => {
                println!("FAILED");
                println!("  Exit code: {}", exit_code);
                println!("  Expected:\n{}", indent(&expected, "    "));
                println!("  Actual:\n{}", indent(&actual, "    "));
                if !stderr.is_empty() {
                    println!("  Stderr:\n{}", indent(&stderr, "    "));
                }
                failed += 1;
            }
            Ok(TestResult::Skipped { reason }) => {
                println!("SKIPPED: {}", reason);
                skipped += 1;
            }
            Err(e) => {
                println!("ERROR: {}", e);
                failed += 1;
            }
        }
    }

    println!();
    println!(
        "Results: {} passed, {} failed, {} skipped",
        passed, failed, skipped
    );

    if failed > 0 {
        panic!("{} tests failed", failed);
    }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check if actual output matches expected pattern.
/// Supports wildcards: ____-__-__ __:__:__ matches any timestamp
fn matches_pattern(expected: &str, actual: &str) -> bool {
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();

    if expected_lines.len() != actual_lines.len() {
        return false;
    }

    for (exp, act) in expected_lines.iter().zip(actual_lines.iter()) {
        if !line_matches(exp, act) {
            return false;
        }
    }

    true
}

/// Check if a single line matches the pattern
fn line_matches(pattern: &str, actual: &str) -> bool {
    // Simple wildcard matching for timestamps
    let parts: Vec<&str> = pattern.split("____-__-__ __:__:__").collect();
    
    if parts.len() == 1 {
        // No wildcards, exact match
        return pattern == actual;
    }
    
    // Check prefix and suffix match
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            // Check prefix
            if !actual.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            // Check suffix
            if !actual.ends_with(part) {
                return false;
            }
        } else {
            // Check middle part
            if let Some(idx) = actual[pos..].find(part) {
                pos += idx + part.len();
            } else {
                return false;
            }
        }
    }
    
    true
}

