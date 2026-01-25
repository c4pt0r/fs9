use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::sync::OnceCell;

static TEST_SERVER_URL: OnceCell<String> = OnceCell::const_new();

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

        let process = Command::new(&server_bin)
            .env("FS9_PORT", port.to_string())
            .env("FS9_HOST", "127.0.0.1")
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

fn find_free_port() -> Result<u16, Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

async fn start_server_once() -> String {
    tokio::task::spawn_blocking(|| {
        let server = TestServer::start().expect("Failed to start test server");
        let url = server.url.clone();
        // Intentionally leak the server process to keep it alive for the test duration
        std::mem::forget(server);
        url
    })
    .await
    .expect("Failed to spawn blocking task")
}

pub async fn get_server_url() -> String {
    TEST_SERVER_URL
        .get_or_init(start_server_once)
        .await
        .clone()
}

pub fn generate_test_path(prefix: &str) -> String {
    use rand::Rng;
    let suffix: u32 = rand::thread_rng().gen();
    format!("/{}_{}", prefix, suffix)
}
