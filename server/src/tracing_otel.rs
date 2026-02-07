#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider as _;
#[cfg(feature = "otel")]
use opentelemetry_sdk::trace::TracerProvider;

#[cfg(feature = "otel")]
pub fn init_tracer(
    service_name: &str,
) -> Result<TracerProvider, Box<dyn std::error::Error + Send + Sync>> {
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::SpanExporter;
    use opentelemetry_sdk::{runtime, Resource};

    let exporter = SpanExporter::builder().with_tonic().build()?;

    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, runtime::Tokio)
        .with_resource(Resource::new(vec![KeyValue::new(
            "service.name",
            service_name.to_string(),
        )]))
        .build();

    let _tracer = provider.tracer("fs9-server");

    Ok(provider)
}

#[cfg(feature = "otel")]
pub fn otel_tracer(provider: &TracerProvider) -> opentelemetry_sdk::trace::Tracer {
    provider.tracer("fs9-server")
}

#[cfg(feature = "otel")]
pub async fn shutdown_tracer(provider: TracerProvider) {
    if let Err(e) = provider.shutdown() {
        tracing::warn!(error = %e, "Failed to shut down OpenTelemetry tracer");
    }
}
