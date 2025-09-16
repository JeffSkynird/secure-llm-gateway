use std::{collections::HashMap, sync::Arc, time::Duration};

use governor::middleware::NoOpMiddleware;
use http::Request;
use serde::Deserialize;
use tower_governor::{
    errors::GovernorError, governor::GovernorConfigBuilder, key_extractor::KeyExtractor,
    GovernorLayer,
};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub listen_addr: String,
    pub openai_api_key: String,
    #[serde(default)]
    pub openai_base_url: Option<String>,

    // rate limit
    #[serde(default = "default_rps")]
    pub rps: u32,
    #[serde(default = "default_burst")]
    pub burst: u32,

    // redis + quotas
    #[serde(default)]
    pub redis_url: Option<String>,
    #[serde(default = "default_quota")]
    pub default_quota: u32,
    #[serde(default = "default_quota_window_secs")]
    pub quota_window_secs: u64,
    #[serde(default)]
    pub tenant_quotas: HashMap<String, u32>,

    // circuit-breaker lite
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    // telemetry
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

fn default_rps() -> u32 {
    5
}
fn default_burst() -> u32 {
    10
}

fn default_quota() -> u32 {
    120
}

fn default_quota_window_secs() -> u64 {
    60
}

fn default_service_name() -> String {
    "secure-llm-gateway".to_string()
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen_addr =
            std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let openai_api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY not set"))?;
        let openai_base_url = std::env::var("OPENAI_BASE_URL").ok();
        let rps = std::env::var("RPS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default_rps());
        let burst = std::env::var("BURST")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default_burst());
        let redis_url = std::env::var("REDIS_URL").ok();
        let default_quota = std::env::var("DEFAULT_QUOTA")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(default_quota);
        let quota_window_secs = std::env::var("QUOTA_WINDOW_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(default_quota_window_secs);
        let tenant_quotas = std::env::var("TENANT_QUOTAS")
            .ok()
            .map(parse_tenant_quotas)
            .unwrap_or_default();
        let timeout_secs = std::env::var("TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok());
        let max_concurrency = std::env::var("MAX_CONCURRENCY")
            .ok()
            .and_then(|s| s.parse().ok());
        let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        let service_name =
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| default_service_name());
        Ok(Self {
            listen_addr,
            openai_api_key,
            openai_base_url,
            rps,
            burst,
            redis_url,
            default_quota,
            quota_window_secs,
            tenant_quotas,
            timeout_secs,
            max_concurrency,
            otlp_endpoint,
            service_name,
        })
    }

    pub fn build_governor(&self) -> anyhow::Result<GovernorLayer<ApiKeyExtractor, NoOpMiddleware>> {
        if self.rps == 0 {
            anyhow::bail!("RPS must be greater than zero");
        }
        if self.burst == 0 {
            anyhow::bail!("BURST must be greater than zero");
        }

        let mut builder = GovernorConfigBuilder::default();
        let mut builder = builder.key_extractor(ApiKeyExtractor);
        builder.period(Duration::from_secs(1) / self.rps);
        builder.burst_size(self.burst);

        let config = builder
            .finish()
            .ok_or_else(|| anyhow::anyhow!("invalid governor configuration"))?;

        Ok(GovernorLayer {
            config: Arc::new(config),
        })
    }
}

fn parse_tenant_quotas(s: String) -> HashMap<String, u32> {
    s.split(',')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let tenant = parts.next()?.trim();
            let quota = parts.next()?.trim().parse().ok()?;
            if tenant.is_empty() {
                return None;
            }
            Some((tenant.to_string(), quota))
        })
        .collect()
}

#[derive(Clone, Copy)]
pub struct ApiKeyExtractor;

impl KeyExtractor for ApiKeyExtractor {
    type Key = String;

    fn extract<B>(&self, req: &Request<B>) -> Result<Self::Key, GovernorError> {
        // Use X-Api-Key header if present, otherwise fall back to client IP+path
        if let Some(k) = req.headers().get("x-api-key") {
            if let Ok(s) = k.to_str() {
                if !s.is_empty() {
                    return Ok(format!("key:{s}"));
                }
            }
        }
        let ip = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");
        let path = req.uri().path();
        Ok(format!("ip:{ip}:{path}"))
    }
}
