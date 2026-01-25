//! sh9 - Interactive shell for FS9
//!
//! Usage:
//!   sh9                     # Start interactive REPL
//!   sh9 -c "command"        # Execute a command
//!   sh9 script.sh9          # Execute a script file

use sh9::{Shell, Sh9Error};
use std::env;
use std::sync::Arc;
use tokio::sync::Mutex;

mod completer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    
    // Get server URL from environment or default
    let server_url = env::var("FS9_SERVER_URL")
        .unwrap_or_else(|_| "http://localhost:9999".to_string());
    
    let mut shell = Shell::new(&server_url);
    
    // Connect to FS9 server
    if let Err(e) = shell.connect().await {
        eprintln!("Warning: Could not connect to FS9 server: {}", e);
    }
    
    if args.len() == 1 {
        // Interactive mode
        run_repl(&mut shell).await?;
    } else if args.len() >= 3 && args[1] == "-c" {
        // Execute command from argument
        let command = args[2..].join(" ");
        match shell.execute(&command).await {
            Ok(code) => std::process::exit(code),
            Err(Sh9Error::Exit(code)) => std::process::exit(code),
            Err(e) => {
                eprintln!("sh9: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // Execute script file
        let script_path = &args[1];
        match std::fs::read_to_string(script_path) {
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
    }
    
    Ok(())
}

async fn run_repl(shell: &mut Shell) -> Result<(), Box<dyn std::error::Error>> {
    use rustyline::error::ReadlineError;
    use rustyline::{Config, Editor, CompletionType};
    use completer::Sh9Helper;

    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    
    let cwd = Arc::new(Mutex::new(shell.cwd.clone()));
    let helper = Sh9Helper::new(shell.client.clone(), cwd.clone());
    
    let mut rl = Editor::with_config(config)?;
    rl.set_helper(Some(helper));
    
    let history_path = dirs_home().join(".sh9_history");
    let _ = rl.load_history(&history_path);

    println!("sh9 - FS9 Shell v{}", env!("CARGO_PKG_VERSION"));
    println!("Type 'exit' to quit, 'help' for help.");
    println!();

    loop {
        {
            let mut cwd_guard = cwd.lock().await;
            *cwd_guard = shell.cwd.clone();
        }
        
        let prompt = format!("sh9:{}> ", shell.cwd);
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
