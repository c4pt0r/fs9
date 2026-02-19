//! Client for communicating with fs9-meta service.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use crate::circuit_breaker::CircuitBreaker;

/// Client for fs9-meta service API.
#[derive(Clone)]
pub struct MetaClient {
    client: Client,
    base_url: String,
    admin_key: Option<String>,
}

/// Request to validate a token.
#[derive(Serialize)]
struct ValidateRequest {
    token: String,
}

/// Response from token validation.
#[derive(Debug, Deserialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub user_id: Option<String>,
    pub namespace: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    pub expires_at: Option<String>,
    pub error: Option<String>,
}

/// Request to refresh a token.
#[derive(Serialize)]
struct RefreshRequest {
    token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl_seconds: Option<u64>,
}

/// Response from token refresh.
#[derive(Debug, Deserialize)]
pub struct RefreshResponse {
    pub token: String,
    pub expires_at: String,
}

/// Mount info returned by fs9-meta.
#[derive(Debug, Deserialize)]
pub struct MountInfo {
    pub path: String,
    pub provider: String,
    pub config: serde_json::Value,
}

/// Namespace info returned by fs9-meta.
#[derive(Debug, Deserialize)]
pub struct NamespaceInfo {
    pub name: String,
    /// fs9-meta does not return a status field; default to "active"
    #[serde(default = "default_status_active")]
    pub status: String,
}

fn default_status_active() -> String {
    "active".to_string()
}

/// Error type for meta client operations.
#[derive(Debug, thiserror::Error)]
pub enum MetaClientError {
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Meta service returned error: {0}")]
    ServiceError(String),
}

impl MetaClient {
    /// Create a new meta client.
    ///
    /// # Arguments
    /// * `base_url` - Base URL of the fs9-meta service (e.g., "http://localhost:9998")
    #[must_use]
    pub fn new(base_url: &str, admin_key: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            admin_key,
        }
    }

    /// Validate a JWT token with the meta service.
    ///
    /// Returns `ValidateResponse` which indicates if the token is valid
    /// and contains the decoded claims if valid.
    pub async fn validate_token(&self, token: &str) -> Result<ValidateResponse, MetaClientError> {
        let url = format!("{}/api/v1/tokens/validate", self.base_url);

        let mut req = self.client.post(&url).json(&ValidateRequest {
            token: token.to_string(),
        });
        if let Some(key) = &self.admin_key {
            req = req.header("x-fs9-meta-key", key);
        }
        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(MetaClientError::ServiceError(format!(
                "HTTP {status}: {body}"
            )));
        }

        Ok(response.json().await?)
    }

    pub async fn validate_token_with_circuit_breaker(
        &self,
        token: &str,
        circuit_breaker: &Arc<CircuitBreaker>,
        max_retries: u32,
        base_delay_ms: u64,
    ) -> Result<ValidateResponse, MetaClientError> {
        if !circuit_breaker.allow_request().await {
            return Err(MetaClientError::ServiceError(
                "Circuit breaker is open — meta service unavailable".to_string(),
            ));
        }

        let mut last_err = None;
        for attempt in 0..max_retries.max(1) {
            if attempt > 0 {
                let delay = Duration::from_millis(base_delay_ms * 2u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
            }

            match self.validate_token(token).await {
                Ok(resp) => {
                    circuit_breaker.record_success().await;
                    return Ok(resp);
                }
                Err(e) => {
                    tracing::warn!(attempt = attempt + 1, error = %e, "Meta validate_token failed");
                    last_err = Some(e);
                }
            }
        }

        circuit_breaker.record_failure().await;
        Err(last_err.unwrap_or_else(|| {
            MetaClientError::ServiceError("All retry attempts exhausted".to_string())
        }))
    }

    /// Fetch a namespace's info from meta service.
    pub async fn get_namespace(&self, name: &str) -> Result<NamespaceInfo, MetaClientError> {
        let url = format!("{}/api/v1/admin/namespaces/{}", self.base_url, name);
        let mut req = self.client.get(&url);
        if let Some(key) = &self.admin_key {
            req = req.header("x-fs9-meta-key", key);
        }
        let response = req.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(MetaClientError::ServiceError(format!(
                "HTTP {status}: {body}"
            )));
        }
        Ok(response.json().await?)
    }

    /// Fetch mounts for a namespace from meta service.
    pub async fn get_namespace_mounts(
        &self,
        namespace: &str,
    ) -> Result<Vec<MountInfo>, MetaClientError> {
        let url = format!("{}/api/v1/namespaces/{}/mounts", self.base_url, namespace);
        let mut req = self.client.get(&url);
        if let Some(key) = &self.admin_key {
            req = req.header("x-fs9-meta-key", key);
        }
        let response = req.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(MetaClientError::ServiceError(format!(
                "HTTP {status}: {body}"
            )));
        }
        Ok(response.json().await?)
    }

    /// Create a namespace in the meta service.
    pub async fn create_namespace(&self, name: &str) -> Result<NamespaceInfo, MetaClientError> {
        let url = format!("{}/api/v1/admin/namespaces", self.base_url);
        let body = serde_json::json!({ "name": name });
        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.admin_key {
            req = req.header("x-fs9-meta-key", key);
        }
        let response = req.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(MetaClientError::ServiceError(format!(
                "HTTP {status}: {body}"
            )));
        }
        Ok(response.json().await?)
    }

    /// Create a mount for a namespace in the meta service.
    pub async fn create_mount(
        &self,
        namespace: &str,
        path: &str,
        provider: &str,
        config: &serde_json::Value,
    ) -> Result<MountInfo, MetaClientError> {
        let url = format!("{}/api/v1/namespaces/{}/mounts", self.base_url, namespace);
        let body = serde_json::json!({
            "path": path,
            "provider": provider,
            "config": config,
        });
        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.admin_key {
            req = req.header("x-fs9-meta-key", key);
        }
        let response = req.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(MetaClientError::ServiceError(format!(
                "HTTP {status}: {body}"
            )));
        }
        Ok(response.json().await?)
    }

    /// Refresh a JWT token with the meta service.
    ///
    /// Returns a new token with extended expiration.
    pub async fn refresh_token(
        &self,
        token: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<RefreshResponse, MetaClientError> {
        let url = format!("{}/api/v1/tokens/refresh", self.base_url);

        let mut req = self.client.post(&url).json(&RefreshRequest {
            token: token.to_string(),
            ttl_seconds,
        });
        if let Some(key) = &self.admin_key {
            req = req.header("x-fs9-meta-key", key);
        }
        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(MetaClientError::ServiceError(format!(
                "HTTP {status}: {body}"
            )));
        }

        Ok(response.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_client_url_normalization() {
        let client = MetaClient::new("http://localhost:9998/", None);
        assert_eq!(client.base_url, "http://localhost:9998");

        let client2 = MetaClient::new("http://localhost:9998", None);
        assert_eq!(client2.base_url, "http://localhost:9998");
    }
}
