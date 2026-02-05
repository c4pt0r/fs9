use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Inline error response for auth failures (avoids coupling to api module).
#[derive(Debug, serde::Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: u64,
    pub iat: u64,
    /// Namespace binding — one JWT, one namespace.
    #[serde(default)]
    pub ns: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<String>,
}

/// Context extracted from JWT and carried through the entire request.
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub ns: String,
    pub user_id: String,
    pub roles: Vec<String>,
}

impl Claims {
    pub fn new(subject: &str, permissions: Vec<String>, mounts: Vec<String>, ttl_secs: u64) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            sub: subject.to_string(),
            exp: now + ttl_secs,
            iat: now,
            ns: None,
            roles: Vec::new(),
            permissions,
            mounts,
        }
    }

    pub fn with_namespace(subject: &str, ns: &str, roles: Vec<String>, ttl_secs: u64) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            sub: subject.to_string(),
            exp: now + ttl_secs,
            iat: now,
            ns: Some(ns.to_string()),
            roles,
            permissions: Vec::new(),
            mounts: Vec::new(),
        }
    }

    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.contains(&"admin".to_string())
            || self.permissions.contains(&permission.to_string())
    }

    pub fn can_access_mount(&self, path: &str) -> bool {
        if self.mounts.is_empty() || self.permissions.contains(&"admin".to_string()) {
            return true;
        }

        for mount in &self.mounts {
            if mount.ends_with('*') {
                let prefix = &mount[..mount.len() - 1];
                if path.starts_with(prefix) {
                    return true;
                }
            } else if path == mount || path.starts_with(&format!("{mount}/")) {
                return true;
            }
        }

        false
    }
}

#[derive(Clone)]
pub struct JwtConfig {
    pub secret: String,
    pub issuer: Option<String>,
    pub audience: Option<String>,
}

impl JwtConfig {
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            issuer: None,
            audience: None,
        }
    }

    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    pub fn encode(&self, claims: &Claims) -> Result<String, jsonwebtoken::errors::Error> {
        encode(
            &Header::default(),
            claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
    }

    pub fn decode(&self, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
        let mut validation = Validation::default();

        if let Some(issuer) = &self.issuer {
            validation.set_issuer(&[issuer]);
        }

        if let Some(audience) = &self.audience {
            validation.set_audience(&[audience]);
        }

        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &validation,
        )?;

        Ok(token_data.claims)
    }
}

#[derive(Clone)]
pub struct AuthState {
    pub config: JwtConfig,
}

impl AuthState {
    pub fn new(config: JwtConfig) -> Self {
        Self { config }
    }
}

pub async fn auth_middleware(
    axum::extract::State(auth): axum::extract::State<AuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path == "/health" || path.starts_with("/api/v1/auth") {
        // Always inject a default RequestContext even for health checks
        request.extensions_mut().insert(RequestContext {
            ns: crate::namespace::DEFAULT_NAMESPACE.to_string(),
            user_id: "anonymous".to_string(),
            roles: Vec::new(),
        });
        return next.run(request).await;
    }

    let auth_header = request.headers().get(header::AUTHORIZATION);

    let token = match auth_header {
        Some(value) => {
            let value = match value.to_str() {
                Ok(v) => v,
                Err(_) => return unauthorized("Invalid Authorization header"),
            };

            if !value.starts_with("Bearer ") {
                return unauthorized("Authorization header must use Bearer scheme");
            }

            &value[7..]
        }
        None => return unauthorized("Missing Authorization header"),
    };

    match auth.config.decode(token) {
        Ok(claims) => {
            let ns = match &claims.ns {
                Some(ns) => ns.clone(),
                None => return unauthorized("Token missing required 'ns' claim"),
            };
            let ctx = RequestContext {
                ns,
                user_id: claims.sub.clone(),
                roles: claims.roles.clone(),
            };
            request.extensions_mut().insert(ctx);
            request.extensions_mut().insert(claims);
            next.run(request).await
        }
        Err(e) => unauthorized(&format!("Invalid token: {e}")),
    }
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 401,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_verify_token() {
        let config = JwtConfig::new("test-secret-key-12345");
        let claims = Claims::new("user-123", vec!["read".into(), "write".into()], vec!["/data/*".into()], 3600);

        let token = config.encode(&claims).unwrap();
        let decoded = config.decode(&token).unwrap();

        assert_eq!(decoded.sub, "user-123");
        assert!(decoded.has_permission("read"));
        assert!(decoded.has_permission("write"));
        assert!(!decoded.has_permission("admin"));
    }

    #[test]
    fn admin_has_all_permissions() {
        let claims = Claims::new("admin", vec!["admin".into()], vec![], 3600);

        assert!(claims.has_permission("read"));
        assert!(claims.has_permission("write"));
        assert!(claims.has_permission("delete"));
        assert!(claims.has_permission("anything"));
    }

    #[test]
    fn mount_access_wildcard() {
        let claims = Claims::new("user", vec!["read".into()], vec!["/data/*".into()], 3600);

        assert!(claims.can_access_mount("/data/file.txt"));
        assert!(claims.can_access_mount("/data/subdir/file.txt"));
        assert!(!claims.can_access_mount("/other/file.txt"));
    }

    #[test]
    fn mount_access_exact() {
        let claims = Claims::new("user", vec!["read".into()], vec!["/data".into()], 3600);

        assert!(claims.can_access_mount("/data"));
        assert!(claims.can_access_mount("/data/file.txt"));
        assert!(!claims.can_access_mount("/datafile.txt"));
    }

    #[test]
    fn admin_can_access_all_mounts() {
        let claims = Claims::new("admin", vec!["admin".into()], vec![], 3600);

        assert!(claims.can_access_mount("/anything"));
        assert!(claims.can_access_mount("/data/file.txt"));
    }

    #[test]
    fn empty_mounts_allows_all() {
        let claims = Claims::new("user", vec!["read".into()], vec![], 3600);

        assert!(claims.can_access_mount("/anything"));
    }

    #[test]
    fn expired_token_rejected() {
        let config = JwtConfig::new("test-secret");
        let mut claims = Claims::new("user", vec![], vec![], 0);
        claims.exp = claims.iat - 100;

        let token = config.encode(&claims).unwrap();
        assert!(config.decode(&token).is_err());
    }
}
