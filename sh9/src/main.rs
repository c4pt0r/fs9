use clap::Parser;
use fs9_config::Fs9Config;
use sh9::{Sh9Error, Shell};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

mod completer;

/// sh9 - Interactive shell for the FS9 distributed filesystem
#[derive(Parser, Debug)]
#[command(name = "sh9", version, about)]
struct Args {
    /// FS9 server URL
    #[arg(short, long, env = "FS9_SERVER_ENDPOINTS")]
    server: Option<String>,

    /// Authentication token for multi-tenant access
    #[arg(short, long, env = "FS9_TOKEN")]
    token: Option<String>,

    /// Execute command and exit
    #[arg(short = 'c')]
    command: Option<String>,

    /// Script file to execute
    script: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let config = fs9_config::load().unwrap_or_else(|_| Fs9Config::default());

    // Server URL priority: CLI arg > env > config > default
    let server_url = args
        .server
        .or_else(|| {
            if config.shell.server.is_empty() {
                None
            } else {
                Some(config.shell.server.clone())
            }
        })
        .unwrap_or_else(|| "http://localhost:9999".to_string());

    let mut shell = Shell::new(&server_url);

    // Set token if provided
    if let Some(token) = args.token {
        if token.trim().is_empty() {
            eprintln!("Error: Token is empty.");
            eprintln!();
            eprintln!("Please provide a valid JWT token. To generate one:");
            eprintln!("  fs9-admin token generate -u <user> -n <namespace> -q");
            std::process::exit(1);
        }
        shell.set_token(token);
    }

    // Connect to FS9 server
    if let Err(e) = shell.connect().await {
        eprintln!("Warning: Could not connect to FS9 server: {}", e);
    }

    if let Some(command) = args.command {
        // Execute command from -c argument
        match shell.execute(&command).await {
            Ok(code) => std::process::exit(code),
            Err(Sh9Error::Exit(code)) => std::process::exit(code),
            Err(e) => {
                eprintln!("sh9: {}", e);
                std::process::exit(1);
            }
        }
    } else if let Some(script_path) = args.script {
        // Execute script file
        match std::fs::read_to_string(&script_path) {
            Ok(content) => match shell.execute(&content).await {
                Ok(code) => std::process::exit(code),
                Err(Sh9Error::Exit(code)) => std::process::exit(code),
                Err(e) => {
                    eprintln!("sh9: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("sh9: cannot read '{}': {}", script_path, e);
                std::process::exit(1);
            }
        }
    } else {
        run_repl(&mut shell, &config.shell, &server_url).await?;
    }

    Ok(())
}

/// Get the current username from $USER env var or "anonymous"
fn get_prompt_user() -> String {
    env::var("USER").unwrap_or_else(|_| "anonymous".to_string())
}

/// Extract hostname from server URL (e.g., "http://localhost:9999" -> "localhost")
fn get_prompt_host(server_url: &str) -> String {
    // Remove protocol (http://, https://)
    let without_protocol = server_url
        .strip_prefix("https://")
        .or_else(|| server_url.strip_prefix("http://"))
        .unwrap_or(server_url);

    // Take part before ':' (port) or '/' (path)
    without_protocol
        .split(':')
        .next()
        .unwrap_or("localhost")
        .to_string()
}

/// Get current time as HH:MM:SS
fn get_prompt_time() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let total_secs = duration.as_secs();
            let secs_today = total_secs % 86400;
            let hours = secs_today / 3600;
            let minutes = (secs_today % 3600) / 60;
            let seconds = secs_today % 60;
            format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
        }
        Err(_) => "00:00:00".to_string(),
    }
}

/// Get current date as YYYY-MM-DD
fn get_prompt_date() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let total_secs = duration.as_secs();
            let days_since_epoch = total_secs / 86400;

            // Calculate year, month, day from days since epoch (1970-01-01)
            let mut year = 1970;
            let mut remaining_days = days_since_epoch as i32;

            loop {
                let days_in_year = if is_leap_year(year) { 366 } else { 365 };
                if remaining_days < days_in_year {
                    break;
                }
                remaining_days -= days_in_year;
                year += 1;
            }

            let days_in_months = if is_leap_year(year) {
                [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
            } else {
                [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
            };

            let mut month = 1;
            let mut day = remaining_days + 1;

            for &days_in_month in &days_in_months {
                if day <= days_in_month {
                    break;
                }
                day -= days_in_month;
                month += 1;
            }

            format!("{:04}-{:02}-{:02}", year, month, day)
        }
        Err(_) => "1970-01-01".to_string(),
    }
}

/// Check if a year is a leap year
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Get current namespace (always "default" for local shell)
fn get_prompt_namespace(_shell: &Shell) -> String {
    "default".to_string()
}

async fn run_repl(
    shell: &mut Shell,
    shell_config: &fs9_config::ShellConfig,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use completer::Sh9Helper;
    use rustyline::error::ReadlineError;
    use rustyline::{CompletionType, Config, Editor};

    let max_history = shell_config.history.max_entries;
    let rl_config = Config::builder()
        .completion_type(CompletionType::List)
        .max_history_size(max_history)?
        .history_ignore_dups(true)?
        .history_ignore_space(true)
        .build();

    let cwd = Arc::new(RwLock::new(shell.cwd.clone()));
    let env = Arc::new(RwLock::new(HashMap::new()));
    let aliases = Arc::new(RwLock::new(HashMap::new()));
    let functions = Arc::new(RwLock::new(HashSet::new()));

    let helper = Sh9Helper::new(
        shell.client.clone(),
        cwd.clone(),
        shell.namespace.clone(),
        env.clone(),
        aliases.clone(),
        functions.clone(),
    );

    let mut rl = Editor::with_config(rl_config)?;
    rl.set_helper(Some(helper));

    let history_file = shell_config.history.file.clone();
    let history_path = if let Some(stripped) = history_file.strip_prefix("~/") {
        dirs_home().join(stripped)
    } else {
        std::path::PathBuf::from(&history_file)
    };
    let _ = rl.load_history(&history_path);

    println!("sh9 - FS9 Shell v{}", env!("CARGO_PKG_VERSION"));
    println!("Type 'exit' to quit, 'help' for help.");
    if shell.token.is_some() {
        println!("(authenticated)");
    }
    println!();

    let mut last_exit_code = 0;

    loop {
        {
            let mut cwd_guard = cwd.write().unwrap();
            *cwd_guard = shell.cwd.clone();
        }

        {
            let mut env_guard = env.write().unwrap();
            *env_guard = shell.env.clone();
        }

        {
            let mut aliases_guard = aliases.write().unwrap();
            *aliases_guard = shell.aliases.clone();
        }

        {
            let mut functions_guard = functions.write().unwrap();
            *functions_guard = shell.functions.keys().cloned().collect();
        }

        let prompt = shell_config
            .prompt
            .replace("{cwd}", &shell.cwd)
            .replace("{user}", &get_prompt_user())
            .replace("{host}", &get_prompt_host(server_url))
            .replace("{time}", &get_prompt_time())
            .replace("{date}", &get_prompt_date())
            .replace("{status}", &last_exit_code.to_string())
            .replace("{ns}", &get_prompt_namespace(shell))
            .replace("{red}", "\x1b[31m")
            .replace("{green}", "\x1b[32m")
            .replace("{blue}", "\x1b[34m")
            .replace("{yellow}", "\x1b[33m")
            .replace("{cyan}", "\x1b[36m")
            .replace("{bold}", "\x1b[1m")
            .replace("{reset}", "\x1b[0m");

        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(line);

                if line == "exit" || line == "quit" {
                    break;
                }

                match shell.execute(line).await {
                    Ok(code) => {
                        last_exit_code = code;
                    }
                    Err(Sh9Error::Exit(code)) => {
                        std::process::exit(code);
                    }
                    Err(e) => {
                        eprintln!("sh9: {}", e);
                        last_exit_code = 1;
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("exit");
                break;
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);

    Ok(())
}

fn dirs_home() -> std::path::PathBuf {
    env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}
