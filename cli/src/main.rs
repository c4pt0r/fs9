use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

mod config;
mod jwt;

use config::Config;

#[derive(Parser)]
#[command(name = "fs9-admin")]
#[command(about = "FS9 Multi-tenant Administration CLI", long_about = None)]
#[command(version)]
struct Cli {
    /// Server URL (overrides config)
    #[arg(short, long, global = true)]
    server: Option<String>,

    /// JWT secret (overrides config)
    #[arg(long, global = true)]
    secret: Option<String>,

    /// Admin namespace for management operations
    #[arg(long, global = true, default_value = "admin")]
    admin_ns: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize configuration
    Init {
        /// Server URL
        #[arg(short, long, default_value = "http://localhost:9999")]
        server: String,
        /// JWT secret
        #[arg(short = 'k', long)]
        secret: String,
    },

    /// Namespace management
    #[command(subcommand)]
    Ns(NsCommands),

    /// Token management
    #[command(subcommand)]
    Token(TokenCommands),

    /// Show current configuration
    Config,

    /// Check server health
    Health,
}

#[derive(Subcommand)]
enum NsCommands {
    /// Create a new namespace
    Create {
        /// Namespace name (lowercase, alphanumeric, hyphens, underscores)
        name: String,
    },
    /// List all namespaces
    List,
    /// Get namespace details
    Get {
        /// Namespace name
        name: String,
    },
    /// Delete a namespace (if supported)
    Delete {
        /// Namespace name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum TokenCommands {
    /// Generate a JWT token for a user
    Generate {
        /// User ID / subject
        #[arg(short, long)]
        user: String,
        /// Namespace
        #[arg(short, long)]
        namespace: String,
        /// Role (admin, operator, reader)
        #[arg(short, long, default_value = "reader")]
        role: String,
        /// Token TTL in seconds
        #[arg(short, long, default_value = "86400")]
        ttl: u64,
    },
    /// Decode and display a JWT token (without verification)
    Decode {
        /// JWT token
        token: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct NamespaceInfo {
    name: String,
    created_at: String,
    created_by: String,
    status: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateNamespaceRequest {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

fn main() {
    let cli = Cli::parse();

    // Load config
    let mut config = Config::load().unwrap_or_default();

    // Override with CLI args
    if let Some(server) = &cli.server {
        config.server = server.clone();
    }
    if let Some(secret) = &cli.secret {
        config.jwt_secret = secret.clone();
    }

    let result = match cli.command {
        Commands::Init { server, secret } => cmd_init(server, secret),
        Commands::Config => cmd_config(&config),
        Commands::Health => cmd_health(&config),
        Commands::Ns(ns_cmd) => match ns_cmd {
            NsCommands::Create { name } => cmd_ns_create(&config, &cli.admin_ns, &name),
            NsCommands::List => cmd_ns_list(&config, &cli.admin_ns),
            NsCommands::Get { name } => cmd_ns_get(&config, &cli.admin_ns, &name),
            NsCommands::Delete { name, force } => cmd_ns_delete(&config, &cli.admin_ns, &name, force),
        },
        Commands::Token(token_cmd) => match token_cmd {
            TokenCommands::Generate { user, namespace, role, ttl } => {
                cmd_token_generate(&config, &user, &namespace, &role, ttl)
            }
            TokenCommands::Decode { token } => cmd_token_decode(&token),
        },
    };

    if let Err(e) = result {
        eprintln!("{} {}", "Error:".red().bold(), e);
        std::process::exit(1);
    }
}

fn cmd_init(server: String, secret: String) -> Result<(), String> {
    let config = Config {
        server,
        jwt_secret: secret,
    };
    config.save()?;
    println!("{} Configuration saved to {}", "✓".green(), Config::path().display());
    Ok(())
}

fn cmd_config(config: &Config) -> Result<(), String> {
    println!("{}", "Current Configuration:".bold());
    println!("  Server:     {}", config.server.cyan());
    println!("  JWT Secret: {}", if config.jwt_secret.is_empty() { "(not set)".red().to_string() } else { "(set)".green().to_string() });
    println!("  Config:     {}", Config::path().display());
    Ok(())
}

fn cmd_health(config: &Config) -> Result<(), String> {
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/health", config.server);

    match client.get(&url).send() {
        Ok(resp) if resp.status().is_success() => {
            println!("{} Server is healthy", "✓".green());
            println!("  URL: {}", config.server.cyan());
            Ok(())
        }
        Ok(resp) => Err(format!("Server returned {}", resp.status())),
        Err(e) => Err(format!("Failed to connect: {}", e)),
    }
}

fn cmd_ns_create(config: &Config, admin_ns: &str, name: &str) -> Result<(), String> {
    let token = jwt::generate(&config.jwt_secret, "admin", admin_ns, "admin", 3600)?;
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/api/v1/namespaces", config.server);

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&CreateNamespaceRequest { name: name.to_string() })
        .send()
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = resp.status();
    let body = resp.text().unwrap_or_default();

    match status.as_u16() {
        201 => {
            let ns: NamespaceInfo = serde_json::from_str(&body)
                .map_err(|e| format!("Failed to parse response: {}", e))?;
            println!("{} Created namespace: {}", "✓".green(), ns.name.cyan());
            println!("  Created at: {}", ns.created_at);
            println!("  Created by: {}", ns.created_by);
            Ok(())
        }
        409 => Err(format!("Namespace '{}' already exists", name)),
        400 => {
            let err: ErrorResponse = serde_json::from_str(&body).unwrap_or(ErrorResponse {
                error: body,
                code: 400,
            });
            Err(format!("Invalid namespace name: {}", err.error))
        }
        403 => Err("Permission denied. Check your admin namespace and JWT secret.".to_string()),
        401 => Err("Authentication failed. Check your JWT secret.".to_string()),
        _ => Err(format!("Unexpected response ({}): {}", status, body)),
    }
}

fn cmd_ns_list(config: &Config, admin_ns: &str) -> Result<(), String> {
    let token = jwt::generate(&config.jwt_secret, "admin", admin_ns, "admin", 3600)?;
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/api/v1/namespaces", config.server);

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = resp.status();
    let body = resp.text().unwrap_or_default();

    if !status.is_success() {
        return Err(format!("Request failed ({}): {}", status, body));
    }

    let namespaces: Vec<NamespaceInfo> = serde_json::from_str(&body)
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    println!("{}", "Namespaces:".bold());
    if namespaces.is_empty() {
        println!("  (none)");
    } else {
        for ns in namespaces {
            let status_color = if ns.status == "active" {
                ns.status.green()
            } else {
                ns.status.yellow()
            };
            println!(
                "  {} {} ({})",
                "•".cyan(),
                ns.name.bold(),
                status_color
            );
            println!("      Created: {} by {}", ns.created_at, ns.created_by);
        }
    }
    Ok(())
}

fn cmd_ns_get(config: &Config, admin_ns: &str, name: &str) -> Result<(), String> {
    let token = jwt::generate(&config.jwt_secret, "admin", admin_ns, "admin", 3600)?;
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/api/v1/namespaces/{}", config.server, name);

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = resp.status();
    let body = resp.text().unwrap_or_default();

    match status.as_u16() {
        200 => {
            let ns: NamespaceInfo = serde_json::from_str(&body)
                .map_err(|e| format!("Failed to parse response: {}", e))?;
            println!("{}", "Namespace Details:".bold());
            println!("  Name:       {}", ns.name.cyan());
            println!("  Status:     {}", if ns.status == "active" { ns.status.green() } else { ns.status.yellow() });
            println!("  Created at: {}", ns.created_at);
            println!("  Created by: {}", ns.created_by);
            Ok(())
        }
        404 => Err(format!("Namespace '{}' not found", name)),
        _ => Err(format!("Request failed ({}): {}", status, body)),
    }
}

fn cmd_ns_delete(config: &Config, admin_ns: &str, name: &str, force: bool) -> Result<(), String> {
    if !force {
        println!("{} Delete namespace '{}'? This cannot be undone.", "Warning:".yellow().bold(), name);
        println!("Use --force to confirm deletion.");
        return Ok(());
    }

    let token = jwt::generate(&config.jwt_secret, "admin", admin_ns, "admin", 3600)?;
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/api/v1/namespaces/{}", config.server, name);

    let resp = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = resp.status();
    let body = resp.text().unwrap_or_default();

    match status.as_u16() {
        200 | 204 => {
            println!("{} Deleted namespace: {}", "✓".green(), name);
            Ok(())
        }
        404 => Err(format!("Namespace '{}' not found", name)),
        501 => Err("Namespace deletion not yet implemented on server".to_string()),
        _ => Err(format!("Request failed ({}): {}", status, body)),
    }
}

fn cmd_token_generate(config: &Config, user: &str, namespace: &str, role: &str, ttl: u64) -> Result<(), String> {
    let token = jwt::generate(&config.jwt_secret, user, namespace, role, ttl)?;

    println!("{}", "Generated Token:".bold());
    println!();
    println!("{}", token.cyan());
    println!();
    println!("{}", "Token Details:".bold());
    println!("  User:      {}", user);
    println!("  Namespace: {}", namespace);
    println!("  Role:      {}", role);
    println!("  TTL:       {} seconds", ttl);
    println!();
    println!("{}", "Usage:".bold());
    println!("  curl -H \"Authorization: Bearer {}\" {}/api/v1/stat?path=/", &token[..20], config.server);

    Ok(())
}

fn cmd_token_decode(token: &str) -> Result<(), String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("Invalid JWT format".to_string());
    }

    // Decode payload (second part)
    let payload = base64_decode(parts[1])?;
    let claims: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|e| format!("Failed to parse payload: {}", e))?;

    println!("{}", "Token Payload:".bold());
    println!("{}", serde_json::to_string_pretty(&claims).unwrap());

    // Show expiration
    if let Some(exp) = claims.get("exp").and_then(|v| v.as_i64()) {
        let exp_time = chrono::DateTime::from_timestamp(exp, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "invalid".to_string());
        let now = chrono::Utc::now().timestamp();
        if exp < now {
            println!("\n{} Token expired at {}", "⚠".yellow(), exp_time);
        } else {
            println!("\n{} Expires at {}", "✓".green(), exp_time);
        }
    }

    Ok(())
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|e| format!("Base64 decode failed: {}", e))
}
