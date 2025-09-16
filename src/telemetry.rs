use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace as sdktrace, Resource};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init_tracing(cfg: &crate::config::AppConfig) {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,axum::rejection=trace"));
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    if let Some(layer) = build_otel_layer(cfg) {
        tracing_subscriber::registry()
            .with(layer)
            .with(fmt::layer().with_target(false).json())
            .with(env_filter.clone())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt::layer().with_target(false).json())
            .with(env_filter)
            .init();
    }
}

pub fn init_metrics() -> anyhow::Result<PrometheusHandle> {
    let builder = PrometheusBuilder::new();
    let handle = builder.install_recorder()?;
    Ok(handle)
}

pub fn track_http_metrics(route: &str, model: &str, request_id: &str) {
    metrics::counter!(
        "http_requests_total",
        "route" => route.to_string(),
        "model" => model.to_string()
    )
    .increment(1);
    metrics::gauge!("inflight_requests").increment(1.0);
    // Drop gauge when request completes would require middleware; MVP keeps it simple.
    let _ = request_id; // suppress unused
}

fn build_otel_layer(
    cfg: &crate::config::AppConfig,
) -> Option<OpenTelemetryLayer<tracing_subscriber::Registry, sdktrace::Tracer>> {
    let endpoint = cfg.otlp_endpoint.as_deref()?;
    let mut exporter_builder = opentelemetry_otlp::SpanExporter::builder().with_http();
    exporter_builder = exporter_builder.with_endpoint(endpoint.to_string());
    let exporter = match exporter_builder.build() {
        Ok(exporter) => exporter,
        Err(err) => {
            eprintln!("failed to build otlp exporter: {err}");
            return None;
        }
    };

    tracing::info!("otlp http exporter enabled -> {}", endpoint);

    let resource = Resource::builder()
        .with_service_name(cfg.service_name.clone())
        .build();

    let provider = sdktrace::SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();

    let tracer = provider.tracer(cfg.service_name.clone());
    opentelemetry::global::set_tracer_provider(provider);

    Some(tracing_opentelemetry::layer().with_tracer(tracer))
}
