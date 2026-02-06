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

    /// Mount management
    #[command(subcommand)]
    Mount(MountCommands),

    /// Show current configuration
    Config,

    /// Check server health
    Health,
}

#[derive(Subcommand)]
enum MountCommands {
    /// Mount a provider to a path in a namespace
    Add {
        /// Provider name (pagefs, memfs, localfs, etc.)
        provider: String,
        /// Target namespace
        #[arg(short, long)]
        namespace: String,
        /// Mount path
        #[arg(short, long, default_value = "/")]
        path: String,
        /// Provider configuration as JSON (e.g., '{"uid": 1000}')
        #[arg(short, long)]
        config: Option<String>,
        /// Set config values (can be repeated). Format: key=value or key.subkey=value
        /// Examples: --set uid=1000 --set backend.type=s3 --set backend.bucket=mybucket
        #[arg(long = "set", value_name = "KEY=VALUE")]
        sets: Vec<String>,
    },
    /// List mounts in a namespace
    List {
        /// Target namespace
        #[arg(short, long)]
        namespace: String,
    },
}

#[derive(Subcommand)]
enum NsCommands {
    /// Create a new namespace
    Create {
        /// Namespace name (lowercase, alphanumeric, hyphens, underscores)
        name: String,
        /// Mount provider after creation (format: "provider" or "provider:/path")
        #[arg(short, long)]
        mount: Option<String>,
        /// Configuration JSON for the mount (e.g., '{"uid": 1000}')
        #[arg(long)]
        mount_config: Option<String>,
        /// Set mount config values (can be repeated). Format: key=value or key.subkey=value
        #[arg(long = "set", value_name = "KEY=VALUE")]
        sets: Vec<String>,
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
        /// Roles: read-only, read-write, admin (can specify multiple times)
        #[arg(short, long = "role", default_value = "read-write")]
        roles: Vec<String>,
        /// Token TTL in seconds
        #[arg(short = 'T', long, default_value = "86400")]
        ttl: u64,
        /// Only output the raw token (for scripting)
        #[arg(short, long)]
        quiet: bool,
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
        Commands::Mount(mount_cmd) => match mount_cmd {
            MountCommands::Add { provider, namespace, path, config: cfg, sets } => {
                cmd_mount_add(&config, &namespace, &path, &provider, cfg, sets)
            }
            MountCommands::List { namespace } => cmd_mount_list(&config, &namespace),
        },
        Commands::Ns(ns_cmd) => match ns_cmd {
            NsCommands::Create { name, mount, mount_config, sets } => {
                cmd_ns_create(&config, &cli.admin_ns, &name, mount, mount_config, sets)
            }
            NsCommands::List => cmd_ns_list(&config, &cli.admin_ns),
            NsCommands::Get { name } => cmd_ns_get(&config, &cli.admin_ns, &name),
            NsCommands::Delete { name, force } => cmd_ns_delete(&config, &cli.admin_ns, &name, force),
        },
        Commands::Token(token_cmd) => match token_cmd {
            TokenCommands::Generate { user, namespace, roles, ttl, quiet } => {
                cmd_token_generate(&config, &user, &namespace, roles, ttl, quiet)
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

fn cmd_ns_create(
    config: &Config,
    admin_ns: &str,
    name: &str,
    mount: Option<String>,
    mount_config: Option<String>,
    sets: Vec<String>,
) -> Result<(), String> {
    let token = jwt::generate(&config.jwt_secret, "admin", admin_ns, &["admin".to_string()], 3600)?;
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

            // If --mount was provided, mount the provider
            if let Some(mount_spec) = mount {
                let (provider, path) = parse_mount_spec(&mount_spec);
                println!();
                match do_mount(config, name, path, provider, mount_config, &sets) {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("{} Mount failed: {}", "⚠".yellow(), e);
                    }
                }
            }

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

/// Parse mount spec: "pagefs" -> ("pagefs", "/") or "pagefs:/data" -> ("pagefs", "/data")
fn parse_mount_spec(spec: &str) -> (&str, &str) {
    match spec.split_once(':') {
        Some((provider, path)) => (provider, path),
        None => (spec, "/"),
    }
}

/// Parse --set key=value arguments into a JSON object.
/// Supports nested keys with dot notation: "backend.type=s3" -> {"backend": {"type": "s3"}}
fn parse_sets_to_json(sets: &[String]) -> Result<serde_json::Value, String> {
    let mut root = serde_json::Map::new();

    for set in sets {
        let (key, value) = set
            .split_once('=')
            .ok_or_else(|| format!("Invalid --set format '{}'. Expected key=value", set))?;

        let value = parse_value(value);
        set_nested_value(&mut root, key, value)?;
    }

    Ok(serde_json::Value::Object(root))
}

/// Parse a string value into appropriate JSON type
fn parse_value(s: &str) -> serde_json::Value {
    // Try to parse as number
    if let Ok(n) = s.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(n) = s.parse::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(n) {
            return serde_json::Value::Number(num);
        }
    }
    // Try to parse as boolean
    match s.to_lowercase().as_str() {
        "true" => return serde_json::Value::Bool(true),
        "false" => return serde_json::Value::Bool(false),
        _ => {}
    }
    // Default to string
    serde_json::Value::String(s.to_string())
}

/// Set a nested value in a JSON object using dot notation
fn set_nested_value(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let parts: Vec<&str> = key.split('.').collect();

    if parts.len() == 1 {
        obj.insert(key.to_string(), value);
        return Ok(());
    }

    let first = parts[0];
    let rest = parts[1..].join(".");

    let nested = obj
        .entry(first.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

    match nested {
        serde_json::Value::Object(ref mut map) => set_nested_value(map, &rest, value),
        _ => Err(format!("Cannot set nested key '{}': parent is not an object", key)),
    }
}

/// Merge --set values into existing config (--set takes precedence)
fn merge_config(
    config_json: Option<String>,
    sets: &[String],
) -> Result<serde_json::Value, String> {
    let mut base: serde_json::Value = config_json
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| format!("Invalid JSON: {}", e))?
        .unwrap_or(serde_json::json!({}));

    if !sets.is_empty() {
        let sets_value = parse_sets_to_json(sets)?;
        merge_json(&mut base, sets_value);
    }

    Ok(base)
}

/// Deep merge two JSON values (source overwrites target)
fn merge_json(target: &mut serde_json::Value, source: serde_json::Value) {
    match (target, source) {
        (serde_json::Value::Object(ref mut t), serde_json::Value::Object(s)) => {
            for (k, v) in s {
                merge_json(t.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (t, s) => *t = s,
    }
}

/// Shared mount logic used by both `mount add` and `ns create --mount`
fn do_mount(
    config: &Config,
    namespace: &str,
    path: &str,
    provider: &str,
    config_json: Option<String>,
    sets: &[String],
) -> Result<(), String> {
    let token = jwt::generate(
        &config.jwt_secret,
        "admin",
        namespace,
        &["operator".to_string()],
        3600,
    )?;
    let client = reqwest::blocking::Client::new();

    let mount_config = merge_config(config_json, sets)?;

    let resp = client
        .post(&format!("{}/api/v1/mount", config.server))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "path": path,
            "provider": provider,
            "config": mount_config
        }))
        .send()
        .map_err(|e| format!("Request failed: {}", e))?;

    match resp.status().as_u16() {
        200 | 201 => {
            println!(
                "{} Mounted {} at {} (namespace: {})",
                "✓".green(),
                provider.cyan(),
                path.cyan(),
                namespace
            );
            Ok(())
        }
        404 => Err(format!("Provider '{}' not found", provider)),
        403 => Err("Permission denied".to_string()),
        _ => Err(format!("Failed: {}", resp.text().unwrap_or_default())),
    }
}

fn cmd_mount_add(
    config: &Config,
    namespace: &str,
    path: &str,
    provider: &str,
    config_json: Option<String>,
    sets: Vec<String>,
) -> Result<(), String> {
    do_mount(config, namespace, path, provider, config_json, &sets)
}

fn cmd_mount_list(config: &Config, namespace: &str) -> Result<(), String> {
    let token = jwt::generate(
        &config.jwt_secret,
        "admin",
        namespace,
        &["read-only".to_string()],
        3600,
    )?;
    let client = reqwest::blocking::Client::new();

    let resp = client
        .get(&format!("{}/api/v1/mounts", config.server))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Request failed: {}", resp.status()));
    }

    let mounts: Vec<serde_json::Value> = resp.json().unwrap_or_default();

    println!("{} in namespace '{}':", "Mounts".bold(), namespace.cyan());
    if mounts.is_empty() {
        println!("  (none)");
    } else {
        for m in mounts {
            println!(
                "  {} → {}",
                m["path"].as_str().unwrap_or("?").bold(),
                m["provider_name"].as_str().unwrap_or("?").green()
            );
        }
    }
    Ok(())
}

fn cmd_ns_list(config: &Config, admin_ns: &str) -> Result<(), String> {
    let token = jwt::generate(&config.jwt_secret, "admin", admin_ns, &["admin".to_string()], 3600)?;
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
    let token = jwt::generate(&config.jwt_secret, "admin", admin_ns, &["admin".to_string()], 3600)?;
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

    let token = jwt::generate(&config.jwt_secret, "admin", admin_ns, &["admin".to_string()], 3600)?;
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

fn cmd_token_generate(config: &Config, user: &str, namespace: &str, roles: Vec<String>, ttl: u64, quiet: bool) -> Result<(), String> {
    // Validate roles
    let valid_roles = ["read-only", "read-write", "admin", "operator"];
    for role in &roles {
        if !valid_roles.contains(&role.as_str()) {
            return Err(format!(
                "Invalid role '{}'. Valid roles: {}",
                role,
                valid_roles.join(", ")
            ));
        }
    }

    let token = jwt::generate(&config.jwt_secret, user, namespace, &roles, ttl)?;

    if quiet {
        println!("{}", token);
    } else {
        println!("{}", "Generated Token:".bold());
        println!();
        println!("{}", token.cyan());
        println!();
        println!("{}", "Token Details:".bold());
        println!("  User:      {}", user);
        println!("  Namespace: {}", namespace);
        println!("  Roles:     {}", roles.join(", "));
        println!("  TTL:       {} seconds", ttl);
        println!();
        println!("{}", "Usage:".bold());
        println!("  curl -H \"Authorization: Bearer {}\" {}/api/v1/stat?path=/", &token[..20], config.server);
    }

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
