//! Authentication / authorization helpers for fs9-meta.

use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::AppState;

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

fn unauthorized(message: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: message.to_string() })).into_response()
}

/// Middleware protecting `/api/v1/*` when an admin key is configured.
///
/// Accepts either:
/// - `Authorization: Bearer <key>`
/// - `x-fs9-meta-key: <key>`
pub async fn require_admin_key(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let Some(expected) = state.admin_key.as_deref() else {
        // No key configured: keep compatibility for local/dev deployments.
        return next.run(req).await;
    };

    let headers = req.headers();

    let presented = headers
        .get("x-fs9-meta-key")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers.get(header::AUTHORIZATION).and_then(|v| {
                let s = v.to_str().ok()?;
                s.strip_prefix("Bearer ")
            })
        });

    match presented {
        Some(key) if key == expected => next.run(req).await,
        _ => unauthorized("missing or invalid admin key"),
    }
}

