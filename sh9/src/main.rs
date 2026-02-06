use clap::Parser;
use fs9_config::Fs9Config;
use sh9::{Shell, Sh9Error};
use std::env;
use std::sync::{Arc, RwLock};

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
    let server_url = args.server
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
        // Interactive REPL
        run_repl(&mut shell, &config.shell.prompt).await?;
    }
    
    Ok(())
}

async fn run_repl(shell: &mut Shell, prompt_template: &str) -> Result<(), Box<dyn std::error::Error>> {
    use rustyline::error::ReadlineError;
    use rustyline::{Config, Editor, CompletionType};
    use completer::Sh9Helper;

    let rl_config = Config::builder()
        .completion_type(CompletionType::List)
        .build();

    let cwd = Arc::new(RwLock::new(shell.cwd.clone()));
    let helper = Sh9Helper::new(shell.client.clone(), cwd.clone());

    let mut rl = Editor::with_config(rl_config)?;
    rl.set_helper(Some(helper));

    let history_path = dirs_home().join(".sh9_history");
    let _ = rl.load_history(&history_path);

    println!("sh9 - FS9 Shell v{}", env!("CARGO_PKG_VERSION"));
    println!("Type 'exit' to quit, 'help' for help.");
    if shell.token.is_some() {
        println!("(authenticated)");
    }
    println!();

    loop {
        {
            let mut cwd_guard = cwd.write().unwrap();
            *cwd_guard = shell.cwd.clone();
        }

        let prompt = prompt_template.replace("{cwd}", &shell.cwd);
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
                    Ok(_) => {}
                    Err(Sh9Error::Exit(code)) => {
                        std::process::exit(code);
                    }
                    Err(e) => {
                        eprintln!("sh9: {}", e);
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
