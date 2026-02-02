use fs9_config::Fs9Config;
use sh9::{Shell, Sh9Error};
use std::env;
use std::sync::{Arc, RwLock};

mod completer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    let config = fs9_config::load().unwrap_or_else(|_| Fs9Config::default());
    let server_url = if config.shell.server.is_empty() {
        "http://localhost:9999".to_string()
    } else {
        config.shell.server.clone()
    };

    let mut shell = Shell::new(&server_url);
    
    // Connect to FS9 server
    if let Err(e) = shell.connect().await {
        eprintln!("Warning: Could not connect to FS9 server: {}", e);
    }
    
    if args.len() == 1 {
        run_repl(&mut shell, &config.shell.prompt).await?;
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
