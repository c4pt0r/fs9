use axum::{
    body::Body,
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use std::num::NonZeroU32;
use std::sync::Arc;

use crate::auth::RequestContext;

#[derive(Clone)]
pub struct RateLimitState {
    ns_limiter: Arc<DefaultKeyedRateLimiter<String>>,
    user_limiter: Arc<DefaultKeyedRateLimiter<String>>,
    enabled: bool,
}

impl RateLimitState {
    pub fn new(ns_qps: u32, user_qps: u32) -> Self {
        let ns_quota = NonZeroU32::new(ns_qps).unwrap_or(NonZeroU32::new(1000).unwrap());
        let user_quota = NonZeroU32::new(user_qps).unwrap_or(NonZeroU32::new(100).unwrap());

        Self {
            ns_limiter: Arc::new(RateLimiter::dashmap(Quota::per_second(ns_quota))),
            user_limiter: Arc::new(RateLimiter::dashmap(Quota::per_second(user_quota))),
            enabled: true,
        }
    }

    pub fn disabled() -> Self {
        Self {
            ns_limiter: Arc::new(RateLimiter::dashmap(Quota::per_second(
                NonZeroU32::new(1).unwrap(),
            ))),
            user_limiter: Arc::new(RateLimiter::dashmap(Quota::per_second(
                NonZeroU32::new(1).unwrap(),
            ))),
            enabled: false,
        }
    }
}

pub async fn rate_limit_middleware(
    axum::extract::State(state): axum::extract::State<RateLimitState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !state.enabled {
        return next.run(request).await;
    }

    let path = request.uri().path();
    if path == "/health" || path == "/metrics" {
        return next.run(request).await;
    }

    if let Some(ctx) = request.extensions().get::<RequestContext>() {
        if state.ns_limiter.check_key(&ctx.ns).is_err() {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "Namespace rate limit exceeded",
            )
                .into_response();
        }

        let user_key = format!("{}:{}", ctx.ns, ctx.user_id);
        if state.user_limiter.check_key(&user_key).is_err() {
            return (StatusCode::TOO_MANY_REQUESTS, "User rate limit exceeded").into_response();
        }
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_state_creation() {
        let state = RateLimitState::new(500, 50);
        assert!(state.enabled);
    }

    #[test]
    fn rate_limit_state_disabled() {
        let state = RateLimitState::disabled();
        assert!(!state.enabled);
    }
}
