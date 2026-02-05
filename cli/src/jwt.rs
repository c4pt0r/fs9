use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    ns: String,
    roles: Vec<String>,
    iat: u64,
    exp: u64,
}

pub fn generate(secret: &str, user: &str, namespace: &str, roles: &[String], ttl_secs: u64) -> Result<String, String> {
    if secret.is_empty() {
        return Err("JWT secret not configured. Run 'fs9-admin init' or use --secret.".to_string());
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("System time error: {}", e))?
        .as_secs();

    let claims = Claims {
        sub: user.to_string(),
        ns: namespace.to_string(),
        roles: roles.to_vec(),
        iat: now,
        exp: now + ttl_secs,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| format!("Failed to generate token: {}", e))
}
