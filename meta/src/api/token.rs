//! Token API handlers.

use axum::{extract::State, Json};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::db::models::{
    GenerateTokenRequest, GenerateTokenResponse, ValidateTokenRequest, ValidateTokenResponse,
};
use crate::error::MetaError;
use crate::AppState;

const REFRESH_GRACE_SECS: i64 = 7 * 24 * 60 * 60;

/// JWT claims structure.
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    /// Subject (user ID)
    sub: String,
    /// Namespace
    ns: String,
    /// Roles
    roles: Vec<String>,
    /// Expiration time (Unix timestamp)
    exp: i64,
    /// Issued at (Unix timestamp)
    iat: i64,
}

/// Generate a new JWT token.
pub async fn generate(
    State(state): State<AppState>,
    Json(req): Json<GenerateTokenRequest>,
) -> Result<Json<GenerateTokenResponse>, MetaError> {
    // Validate namespace exists
    state
        .store
        .get_namespace(&req.namespace)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{}' not found", req.namespace)))?;

    let ttl = req.ttl_seconds.unwrap_or(86400); // Default 24 hours
    let now = Utc::now();
    #[allow(clippy::cast_possible_wrap)]
    let expires_at = now + Duration::seconds(ttl as i64);

    let claims = Claims {
        sub: req.user_id.clone(),
        ns: req.namespace.clone(),
        roles: req.roles.clone(),
        exp: expires_at.timestamp(),
        iat: now.timestamp(),
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )?;

    Ok(Json(GenerateTokenResponse { token, expires_at }))
}

/// Validate a JWT token.
pub async fn validate(
    State(state): State<AppState>,
    Json(req): Json<ValidateTokenRequest>,
) -> Result<Json<ValidateTokenResponse>, MetaError> {
    let mut validation = Validation::default();
    validation.validate_exp = true;

    match decode::<Claims>(
        &req.token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &validation,
    ) {
        Ok(token_data) => {
            let claims = token_data.claims;
            Ok(Json(ValidateTokenResponse {
                valid: true,
                user_id: Some(claims.sub),
                namespace: Some(claims.ns),
                roles: claims.roles,
                expires_at: Some(
                    chrono::DateTime::from_timestamp(claims.exp, 0).unwrap_or_else(Utc::now),
                ),
                error: None,
            }))
        }
        Err(e) => Ok(Json(ValidateTokenResponse {
            valid: false,
            user_id: None,
            namespace: None,
            roles: vec![],
            expires_at: None,
            error: Some(e.to_string()),
        })),
    }
}

/// Refresh request.
#[derive(Debug, Deserialize)]
pub struct RefreshTokenRequest {
    pub token: String,
    pub ttl_seconds: Option<u64>,
}

/// Refresh a JWT token (extend expiration).
pub async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<RefreshTokenRequest>,
) -> Result<Json<GenerateTokenResponse>, MetaError> {
    let mut validation = Validation::default();
    validation.validate_exp = false; // Refresh accepts expired tokens (within grace), but still verifies signature.

    let token_data = decode::<Claims>(
        &req.token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &validation,
    )?;

    let claims = token_data.claims;
    let now_ts = Utc::now().timestamp();
    if claims.exp + REFRESH_GRACE_SECS < now_ts {
        return Err(jsonwebtoken::errors::Error::from(
            jsonwebtoken::errors::ErrorKind::ExpiredSignature,
        )
        .into());
    }
    let ttl = req.ttl_seconds.unwrap_or(86400);
    let now = Utc::now();
    #[allow(clippy::cast_possible_wrap)]
    let expires_at = now + Duration::seconds(ttl as i64);

    let new_claims = Claims {
        sub: claims.sub,
        ns: claims.ns,
        roles: claims.roles,
        exp: expires_at.timestamp(),
        iat: now.timestamp(),
    };

    let token = encode(
        &Header::default(),
        &new_claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )?;

    Ok(Json(GenerateTokenResponse { token, expires_at }))
}
