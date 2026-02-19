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
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db9_client::Db9AuthError;
use crate::state::AppState;

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
    pub fn new(
        subject: &str,
        permissions: Vec<String>,
        mounts: Vec<String>,
        ttl_secs: u64,
    ) -> Self {
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

    /// Decode a token while ignoring `exp` validation (signature still verified).
    pub fn decode_ignore_exp(&self, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
        let mut validation = Validation::default();
        validation.validate_exp = false;

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

    /// Decode a token, allowing expired tokens (for refresh endpoint).
    /// Still validates the signature, just ignores expiration.
    pub fn decode_allow_expired(&self, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
        let mut validation = Validation::default();
        validation.validate_exp = false; // Allow expired tokens

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

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let grace_period = 4 * 60 * 60; // 4 hours
        if token_data.claims.exp + grace_period < now {
            return Err(jsonwebtoken::errors::Error::from(
                jsonwebtoken::errors::ErrorKind::ExpiredSignature,
            ));
        }

        Ok(token_data.claims)
    }
}

#[derive(Clone)]
pub struct AuthState {
    pub enabled: bool,
    pub config: JwtConfig,
}

impl AuthState {
    pub fn new(enabled: bool, config: JwtConfig) -> Self {
        Self { enabled, config }
    }
}

/// Combined auth state for middleware that includes both JWT config and app state.
#[derive(Clone)]
pub struct AuthMiddlewareState {
    pub auth: AuthState,
    pub app_state: Arc<AppState>,
}

impl AuthMiddlewareState {
    pub fn new(auth: AuthState, app_state: Arc<AppState>) -> Self {
        Self { auth, app_state }
    }
}

/// Extract tenant_id from URL paths like `/{tenant_id}/api/v1/...`.
fn extract_tenant_id(path: &str) -> Option<&str> {
    let path = path.strip_prefix('/')?;
    let (tenant_id, rest) = path.split_once('/')?;
    if rest.starts_with("api/v1/") && !tenant_id.is_empty() {
        Some(tenant_id)
    } else {
        None
    }
}

pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AuthMiddlewareState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();

    // Health endpoint needs no auth
    if path == "/health" {
        request.extensions_mut().insert(RequestContext {
            ns: crate::namespace::DEFAULT_NAMESPACE.to_string(),
            user_id: "anonymous".to_string(),
            roles: Vec::new(),
        });
        return next.run(request).await;
    }

    // Auth is disabled: allow all requests and run as anonymous in the default namespace.
    if !state.auth.enabled {
        request.extensions_mut().insert(RequestContext {
            ns: crate::namespace::DEFAULT_NAMESPACE.to_string(),
            user_id: "anonymous".to_string(),
            roles: vec!["admin".to_string()],
        });
        return next.run(request).await;
    }

    // Refresh endpoint handles its own token parsing/validation so it can accept expired tokens
    // without being blocked by meta validation.
    if path == "/api/v1/auth/refresh" {
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

    // Try db9 token auth if tenant_id is in the URL and db9_client is configured
    let tenant_id = extract_tenant_id(&path);
    if let (Some(tenant_id), Some(db9_client)) = (tenant_id, &state.app_state.db9_client) {
        match db9_client.validate_token(token, tenant_id).await {
            Ok(customer_id) => {
                let ctx = RequestContext {
                    ns: tenant_id.to_string(),
                    user_id: customer_id,
                    roles: vec!["admin".to_string()],
                };
                request.extensions_mut().insert(ctx);
                return next.run(request).await;
            }
            Err(Db9AuthError::TenantNotAuthorized(_)) => {
                return forbidden("Tenant not authorized for this token");
            }
            Err(Db9AuthError::CachedRejection) => {
                // Token was recently rejected — fall through to JWT quickly
                tracing::debug!("db9 token rejected (cached), falling back to JWT");
            }
            Err(Db9AuthError::RateLimited) => {
                return too_many_requests("Too many authentication requests, please retry later");
            }
            Err(Db9AuthError::Backend(401, _)) => {
                // Token rejected by db9 — fall through to JWT validation
                tracing::debug!("db9 token rejected (401), falling back to JWT");
            }
            Err(e) => {
                tracing::warn!(error = %e, "db9 token validation failed, falling back to JWT");
            }
        }
    }

    // Check if token has been revoked
    if state.app_state.revocation_set.is_revoked(token).await {
        return unauthorized("Token has been revoked");
    }

    if let Some(cached) = state.app_state.token_cache.get(token).await {
        let ctx = RequestContext {
            ns: cached.namespace.clone(),
            user_id: cached.user_id.clone(),
            roles: cached.roles.clone(),
        };
        request.extensions_mut().insert(ctx);
        // Insert a Claims-like view without extending the token lifetime.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = Claims {
            sub: cached.user_id.clone(),
            exp: cached.expires_at,
            iat: now,
            ns: Some(cached.namespace.clone()),
            roles: cached.roles.clone(),
            permissions: Vec::new(),
            mounts: Vec::new(),
        };
        request.extensions_mut().insert(claims);
        return next.run(request).await;
    }

    // 2. If meta_client is configured, use it for validation (with circuit breaker + retry)
    if let Some(meta_client) = &state.app_state.meta_client {
        match meta_client
            .validate_token_with_circuit_breaker(token, &state.app_state.circuit_breaker, 3, 100)
            .await
        {
            Ok(resp) if resp.valid => {
                let namespace = resp
                    .namespace
                    .clone()
                    .unwrap_or_else(|| crate::namespace::DEFAULT_NAMESPACE.to_string());
                let user_id = resp
                    .user_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                let roles = resp.roles.clone();

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let token_claims = state.auth.config.decode_ignore_exp(token).ok();

                // Cache the validated token, but never beyond the JWT's own `exp`.
                if let Some(claims) = &token_claims {
                    if now >= claims.exp {
                        return unauthorized("Invalid token: expired");
                    }
                    state
                        .app_state
                        .token_cache
                        .set(
                            token,
                            user_id.clone(),
                            namespace.clone(),
                            roles.clone(),
                            claims.exp,
                        )
                        .await;
                }

                let ctx = RequestContext {
                    ns: namespace.clone(),
                    user_id: user_id.clone(),
                    roles: roles.clone(),
                };
                request.extensions_mut().insert(ctx);
                // Insert a Claims-like view without extending the token lifetime.
                let exp = token_claims.as_ref().map_or(now, |c| c.exp);
                let claims = Claims {
                    sub: user_id,
                    exp,
                    iat: now,
                    ns: Some(namespace),
                    roles,
                    permissions: Vec::new(),
                    mounts: Vec::new(),
                };
                request.extensions_mut().insert(claims);
                return next.run(request).await;
            }
            Ok(resp) => {
                // Meta service says token is invalid
                return unauthorized(&format!(
                    "Invalid token: {}",
                    resp.error
                        .unwrap_or_else(|| "validation failed".to_string())
                ));
            }
            Err(e) => {
                // Meta service unavailable - fall through to local JWT validation
                tracing::warn!(
                    error = %e,
                    "Meta service unavailable, falling back to local JWT validation"
                );
            }
        }
    }

    // 3. Fallback: local JWT decode (when meta unavailable or not configured)
    let decode_result = state.auth.config.decode(token);

    match decode_result {
        Ok(claims) => {
            let ns = match &claims.ns {
                Some(ns) => ns.clone(),
                None => return unauthorized("Token missing required 'ns' claim"),
            };

            // Cache the locally validated token. (Refresh requests are handled earlier.)
            state
                .app_state
                .token_cache
                .set(
                    token,
                    claims.sub.clone(),
                    ns.clone(),
                    claims.roles.clone(),
                    claims.exp,
                )
                .await;

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

fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 403,
        }),
    )
        .into_response()
}

fn too_many_requests(message: &str) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 429,
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
        let claims = Claims::new(
            "user-123",
            vec!["read".into(), "write".into()],
            vec!["/data/*".into()],
            3600,
        );

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
