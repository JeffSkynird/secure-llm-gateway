use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Context;
use redis::aio::ConnectionManager;
use tokio::sync::Mutex;

use crate::config::AppConfig;

#[derive(Clone)]
pub struct QuotaManager {
    conn: Arc<Mutex<ConnectionManager>>,
    default_quota: u32,
    window: Duration,
    overrides: HashMap<String, u32>,
}

impl QuotaManager {
    pub async fn maybe_new(cfg: &AppConfig) -> anyhow::Result<Option<Self>> {
        let Some(url) = cfg.redis_url.as_ref() else {
            return Ok(None);
        };

        let client = redis::Client::open(url.clone())
            .with_context(|| format!("failed to create redis client for {url}"))?;
        let conn = ConnectionManager::new(client)
            .await
            .context("failed to connect to redis")?;
        Ok(Some(Self {
            conn: Arc::new(Mutex::new(conn)),
            default_quota: cfg.default_quota,
            window: Duration::from_secs(cfg.quota_window_secs),
            overrides: cfg.tenant_quotas.clone(),
        }))
    }

    fn limit_for(&self, tenant: &str) -> u32 {
        self.overrides
            .get(tenant)
            .copied()
            .unwrap_or(self.default_quota)
    }

    async fn increment(&self, key: &str) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.lock().await;
        let count: i64 = redis::cmd("INCR").arg(key).query_async(&mut *conn).await?;
        if count == 1 {
            let ttl_secs = self.window.as_secs() as usize;
            let _: () = redis::cmd("EXPIRE")
                .arg(key)
                .arg(ttl_secs)
                .query_async(&mut *conn)
                .await?;
        }
        Ok(count)
    }

    pub async fn check_and_increment(&self, tenant: &str) -> Result<(), QuotaError> {
        let limit = self.limit_for(tenant);
        if limit == 0 {
            return Err(QuotaError::Exceeded {
                limit,
                current: limit,
            });
        }
        let key = format!("quota:{tenant}");
        let count = self
            .increment(&key)
            .await
            .map_err(|e| QuotaError::Backend(e.into()))?;
        if count as u32 > limit {
            return Err(QuotaError::Exceeded {
                limit,
                current: count as u32,
            });
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QuotaError {
    #[error("tenant quota exceeded (limit {limit}, current {current})")]
    Exceeded { limit: u32, current: u32 },
    #[error("quota backend error: {0}")]
    Backend(#[from] anyhow::Error),
}
