use async_stream::stream;
use axum::{
    error_handling::HandleErrorLayer,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response, Sse},
    routing::{get, post},
    BoxError, Json, Router,
};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio::signal;
use tower::{
    limit::GlobalConcurrencyLimitLayer, load_shed::LoadShedLayer, timeout::TimeoutLayer,
    ServiceBuilder,
};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing::instrument;
use uuid::Uuid;

mod config;
mod provider;
mod quota;
mod redact;
mod telemetry;

use crate::config::AppConfig;
use crate::provider::openai::{
    ChatCompletionRequest, ChatMessage as OpenAIChatMessage, OpenAIChatCompletionResponse,
    OpenAIProvider, OpenAIStreamChunk,
};
use crate::quota::{QuotaError, QuotaManager};
use crate::redact::{redact_text, RedactionStats};
use crate::telemetry::{init_metrics, init_tracing, track_http_metrics};

#[derive(Clone)]
struct AppState {
    cfg: Arc<AppConfig>,
    openai: Arc<OpenAIProvider>,
    quota: Option<QuotaManager>,
}

#[derive(Debug, Deserialize, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    top_p: Option<f32>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    stream: Option<bool>,
    // pass-through for extra fields, ignored for MVP
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = AppConfig::from_env()?;
    init_tracing(&cfg);
    let handle = init_metrics()?;

    let quota = QuotaManager::maybe_new(&cfg).await?;
    let state = AppState {
        openai: Arc::new(OpenAIProvider::new(
            cfg.openai_api_key.clone(),
            cfg.openai_base_url.clone(),
        )?),
        cfg: Arc::new(cfg),
        quota,
    };
    let state = Arc::new(state);

    let governor = state.cfg.build_governor()?;

    let middleware = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(governor)
        .layer(HandleErrorLayer::new(handle_layer_error))
        .layer(LoadShedLayer::new())
        .option_layer(
            state
                .cfg
                .timeout_secs
                .filter(|v| *v > 0)
                .map(|secs| TimeoutLayer::new(Duration::from_secs(secs))),
        )
        .option_layer(
            state
                .cfg
                .max_concurrency
                .filter(|v| *v > 0)
                .map(GlobalConcurrencyLimitLayer::new),
        )
        .into_inner();

    // Define routes and handlers
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(move || async move { handle.render() }))
        .route("/v1/chat/completions", post(chat_handler))
        .with_state(state.clone())
        .layer(middleware);

    let addr: SocketAddr = state.cfg.listen_addr.parse()?;
    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    info!(%local_addr, "starting server");
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("failed to install signal handler");
        term.recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("signal received, starting graceful shutdown");
}

#[instrument(skip(state, headers, req), fields(tenant = %headers.get("x-api-key").and_then(|v| v.to_str().ok()).unwrap_or("anonymous"),
                                               model  = %req.model))]
async fn chat_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut req): Json<ChatRequest>,
) -> impl IntoResponse {
    let request_id = Uuid::new_v4().to_string();
    metrics::counter!("requests_total", "route" => "/v1/chat/completions").increment(1);

    let tenant = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .unwrap_or("anonymous");

    if let Some(quota) = state.quota.as_ref() {
        if let Err(err) = quota.check_and_increment(tenant).await {
            return handle_quota_error(err);
        }
    }

    // Redact request messages
    let mut redaction_stats = RedactionStats::default();
    for m in &mut req.messages {
        let (redacted, stats) = redact_text(&m.content);
        m.content = redacted;
        redaction_stats += stats;
    }
    metrics::counter!("redactions_total").increment(redaction_stats.matches as u64);

    let provider = state.openai.clone();
    let model = req.model.clone();
    let stream_requested = req.stream.unwrap_or(true);
    let openai_req = ChatCompletionRequest {
        model: req.model.clone(),
        messages: req
            .messages
            .iter()
            .cloned()
            .map(|m| OpenAIChatMessage {
                role: m.role,
                content: m.content,
            })
            .collect(),
        temperature: req.temperature,
        top_p: req.top_p,
        max_tokens: req.max_tokens,
        stream: Some(stream_requested),
    };

    if !stream_requested {
        let mut response = match provider.chat_completion(openai_req).await {
            Ok(resp) => resp,
            Err(e) => {
                track_http_metrics("/v1/chat/completions", &model, &request_id);
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        };

        redact_completion(&mut response);
        track_http_metrics("/v1/chat/completions", &model, &request_id);
        return Json(response).into_response();
    }

    // Stream from OpenAI and re-redact delta chunks before forwarding
    let mut upstream = match provider.chat_stream(openai_req).await {
        Ok(s) => s,
        Err(e) => {
            let err = format!(r#"{{"error":"{}"}}"#, e);
            let stream = stream! {
                yield Ok::<_, Infallible>(axum::response::sse::Event::default().data(err));
            };
            let sse = Sse::new(stream).keep_alive(
                axum::response::sse::KeepAlive::new().interval(Duration::from_secs(10)),
            );
            track_http_metrics("/v1/chat/completions", &model, &request_id);
            return sse.into_response();
        }
    };

    let first_item = upstream.next().await;

    let mut buffered_events = Vec::new();
    let mut stream_done = false;

    match first_item {
        Some(Ok(line)) => {
            if let Some((data, should_break)) = process_stream_line(&line) {
                buffered_events.push(axum::response::sse::Event::default().data(data));
                if should_break {
                    stream_done = true;
                }
            }
        }
        Some(Err(e)) => {
            let err = format!(r#"{{"error":"stream error: {}"}}"#, e);
            buffered_events.push(axum::response::sse::Event::default().data(err));
            stream_done = true;
        }
        None => {
            stream_done = true;
        }
    }

    let stream = stream! {
        for event in buffered_events {
            yield Ok::<_, Infallible>(event);
        }

        if stream_done {
            return;
        }

        while let Some(item) = upstream.next().await {
            match item {
                Ok(line) => {
                    if let Some((data, should_break)) = process_stream_line(&line) {
                        yield Ok(axum::response::sse::Event::default().data(data));
                        if should_break {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let err = format!(r#"{{"error":"stream error: {}"}}"#, e);
                    yield Ok(axum::response::sse::Event::default().data(err));
                    break;
                }
            }
        }
    };

    let sse = Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(10)));
    track_http_metrics("/v1/chat/completions", &model, &request_id);
    sse.into_response()
}

fn process_stream_line(line: &str) -> Option<(String, bool)> {
    if line.trim() == "data: [DONE]" {
        return Some(("data: [DONE]".to_string(), true));
    }

    if let Some(json_part) = line.strip_prefix("data: ") {
        if let Ok(mut chunk) = serde_json::from_str::<OpenAIStreamChunk>(json_part) {
            if let Some(choice) = chunk.choices.get_mut(0) {
                if let Some(delta) = choice.delta.as_mut() {
                    if let Some(content) = delta.content.as_mut() {
                        let (red, _) = redact_text(content);
                        *content = red;
                    }
                }
            }
            if let Ok(s) = serde_json::to_string(&chunk) {
                return Some((format!("data: {}", s), false));
            }
        }
        return Some((format!("data: {}", json_part), false));
    }

    None
}

fn redact_completion(resp: &mut OpenAIChatCompletionResponse) {
    for choice in &mut resp.choices {
        if let Some(message) = choice.message.as_mut() {
            let (redacted, _) = redact_text(&message.content);
            message.content = redacted;
        }
    }
}

fn handle_quota_error(err: QuotaError) -> Response {
    match err {
        QuotaError::Exceeded { limit, .. } => {
            metrics::counter!("quota_block_total", "reason" => "exceeded").increment(1);
            (
                StatusCode::TOO_MANY_REQUESTS,
                format!("quota exceeded (limit={limit})"),
            )
                .into_response()
        }
        QuotaError::Backend(e) => {
            tracing::error!(error = %e, "quota backend failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "quota backend failure").into_response()
        }
    }
}

async fn handle_layer_error(err: BoxError) -> impl IntoResponse {
    if err.is::<tower::timeout::error::Elapsed>() {
        tracing::warn!("request timed out");
        metrics::counter!("cb_events_total", "event" => "timeout").increment(1);
        return (StatusCode::GATEWAY_TIMEOUT, "upstream timed out");
    }
    if err.is::<tower::load_shed::error::Overloaded>() {
        tracing::warn!("shed request due to overload");
        metrics::counter!("cb_events_total", "event" => "load_shed").increment(1);
        return (StatusCode::SERVICE_UNAVAILABLE, "server overloaded");
    }
    tracing::error!(error = %err, "unhandled middleware error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}
