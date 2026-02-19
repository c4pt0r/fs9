use axum::{body::Body, extract::Request, middleware::Next, response::Response};
use metrics::{counter, histogram};
use std::time::Instant;

use crate::auth::RequestContext;

pub fn init_metrics() -> metrics_exporter_prometheus::PrometheusHandle {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder")
}

pub async fn metrics_middleware(request: Request<Body>, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let path_label = normalize_path(&path);

    if path_label == "/metrics" {
        return next.run(request).await;
    }

    let ns = request
        .extensions()
        .get::<RequestContext>()
        .map(|ctx| ctx.ns.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let start = Instant::now();
    let response = next.run(request).await;
    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    let labels = [
        ("method", method.to_string()),
        ("path", path_label),
        ("status", status),
        ("namespace", ns),
    ];

    counter!("fs9_http_requests_total", &labels).increment(1);
    histogram!("fs9_http_request_duration_seconds", &labels[..3]).record(duration);

    response
}

fn normalize_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 5 && parts[1] == "api" && parts[2] == "v1" && parts[3] == "namespaces" {
        return format!(
            "/api/v1/namespaces/:ns{}",
            if parts.len() > 5 {
                format!("/{}", parts[5..].join("/"))
            } else {
                String::new()
            }
        );
    }
    path.to_string()
}

pub async fn metrics_handler(
    axum::extract::State(handle): axum::extract::State<
        metrics_exporter_prometheus::PrometheusHandle,
    >,
) -> String {
    handle.render()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_preserves_static() {
        assert_eq!(normalize_path("/api/v1/stat"), "/api/v1/stat");
        assert_eq!(normalize_path("/health"), "/health");
    }

    #[test]
    fn normalize_path_replaces_namespace() {
        assert_eq!(
            normalize_path("/api/v1/namespaces/myns"),
            "/api/v1/namespaces/:ns"
        );
    }
}
