use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use fs9_client::Fs9Client;
use fs9_config::Fs9Config;
use fuser::MountOption;
use tracing::{error, info};

mod fs;
mod handle;
mod inode;

use fs::Fs9Fuse;

#[derive(Parser, Debug)]
#[command(name = "fs9-fuse")]
#[command(version, about = "Mount FS9 as a local FUSE filesystem")]
struct Args {
    mountpoint: PathBuf,

    #[arg(short, long)]
    server: Option<String>,

    #[arg(short, long)]
    token: Option<String>,

    #[arg(long)]
    allow_other: Option<bool>,

    #[arg(long)]
    allow_root: Option<bool>,

    #[arg(short, long)]
    foreground: bool,

    #[arg(long)]
    cache_ttl: Option<u64>,

    #[arg(long)]
    auto_unmount: Option<bool>,

    #[arg(short = 'r', long)]
    read_only: Option<bool>,

    #[arg(short, long)]
    debug: bool,

    #[arg(short, long)]
    config: Option<String>,
}

fn main() {
    let args = Args::parse();

    let config = match &args.config {
        Some(path) => fs9_config::load_from_file(path).unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load config: {e}, using defaults");
            Fs9Config::default()
        }),
        None => fs9_config::load().unwrap_or_default(),
    };

    let log_level = if args.debug {
        "debug"
    } else {
        config.logging.level.as_str()
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    let server = args.server.unwrap_or_else(|| config.fuse.server.clone());
    let token = args.token.or_else(|| {
        if config.fuse.token.is_empty() {
            None
        } else {
            Some(config.fuse.token.clone())
        }
    });
    let allow_other = args.allow_other.unwrap_or(config.fuse.options.allow_other);
    let allow_root = args.allow_root.unwrap_or(config.fuse.options.allow_root);
    let auto_unmount = args
        .auto_unmount
        .unwrap_or(config.fuse.options.auto_unmount);
    let read_only = args.read_only.unwrap_or(config.fuse.options.read_only);
    let cache_ttl = args
        .cache_ttl
        .unwrap_or_else(|| parse_duration(&config.fuse.cache.attr_ttl));

    info!("FS9 FUSE starting");
    info!("Server: {}", server);
    info!("Mount point: {}", args.mountpoint.display());

    if !args.mountpoint.exists() {
        error!("Mount point does not exist: {}", args.mountpoint.display());
        std::process::exit(1);
    }
    if !args.mountpoint.is_dir() {
        error!(
            "Mount point is not a directory: {}",
            args.mountpoint.display()
        );
        std::process::exit(1);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");

    let client = {
        let mut builder = Fs9Client::builder(&server);
        if let Some(ref t) = token {
            builder = builder.token(t);
        }
        match builder.build() {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to create FS9 client: {}", e);
                std::process::exit(1);
            }
        }
    };

    let rt_handle = runtime.handle().clone();
    if let Err(e) = rt_handle.block_on(client.stat("/")) {
        error!("Failed to connect to FS9 server: {}", e);
        error!("Make sure the server is running at {}", server);
        std::process::exit(1);
    }
    info!("Connected to FS9 server");

    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    let fs = Fs9Fuse::new(
        client,
        rt_handle.clone(),
        uid,
        gid,
        Duration::from_secs(cache_ttl),
    );

    let mut options = vec![
        MountOption::FSName("fs9".to_string()),
        MountOption::Subtype("fs9".to_string()),
        MountOption::DefaultPermissions,
    ];

    if allow_other {
        options.push(MountOption::AllowOther);
    }
    if allow_root {
        options.push(MountOption::AllowRoot);
    }
    if auto_unmount {
        options.push(MountOption::AutoUnmount);
    }
    if read_only {
        options.push(MountOption::RO);
    }

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        info!("Received shutdown signal, unmounting...");
        r.store(false, Ordering::SeqCst);
    })
    .expect("Failed to set Ctrl-C handler");

    info!("Mounting filesystem...");

    let mut session = match fuser::Session::new(fs, &args.mountpoint, &options) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create FUSE session: {}", e);
            std::process::exit(1);
        }
    };

    if args.foreground {
        info!("Running in foreground. Press Ctrl-C to unmount.");

        let mut unmounter = session.unmount_callable();
        let running_clone = running.clone();

        std::thread::spawn(move || {
            while running_clone.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
            }
            info!("Unmounting...");
            if let Err(e) = unmounter.unmount() {
                error!("Failed to unmount: {}", e);
            }
        });

        if let Err(e) = session.run() {
            if running.load(Ordering::SeqCst) {
                error!("FUSE session error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        let guard = session.spawn().expect("Failed to spawn FUSE session");

        info!(
            "Filesystem mounted. Use 'fusermount -u {}' to unmount.",
            args.mountpoint.display()
        );

        while running.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }

        drop(guard);
    }

    info!("FS9 FUSE stopped");
}

fn parse_duration(s: &str) -> u64 {
    let s = s.trim();
    if s.ends_with('s') {
        s[..s.len() - 1].parse().unwrap_or(1)
    } else if s.ends_with('m') {
        s[..s.len() - 1].parse::<u64>().unwrap_or(1) * 60
    } else if s.ends_with('h') {
        s[..s.len() - 1].parse::<u64>().unwrap_or(1) * 3600
    } else {
        s.parse().unwrap_or(1)
    }
}
