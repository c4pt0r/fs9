use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn unique_id() -> u64 {
    TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
}

pub struct MountedFs {
    fuse_process: Child,
    mountpoint: String,
}

impl MountedFs {
    pub fn mount(server_url: &str, mountpoint: &str) -> Result<Self, Box<dyn std::error::Error>> {
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

pub fn get_server_url() -> String {
    std::env::var("FS9_SERVER_ENDPOINTS").unwrap_or_else(|_| "http://localhost:9999".to_string())
}
