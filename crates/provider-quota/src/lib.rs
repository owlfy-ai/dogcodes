//! Read-only account monitoring, independent of an agent's model/session readiness.
//! Credentials stay in this crate; only validated quota readings leave it.
mod credentials;
mod http;
mod projection;
#[cfg(test)]
mod tests;

use agent_domain::ExecutorRef;
use async_trait::async_trait;
use driver_api::{DriverError, QuotaProvider, QuotaSnapshot};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone, Copy)]
enum Kind {
    Go,
    DeepSeek,
    DeepSeekPlatform,
    Sub2Api,
}

// Deliberately no Debug/Serialize: a request contains native credentials.
#[derive(Clone)]
struct Request {
    kind: Kind,
    label: String,
    endpoint: String,
    key: String,
}

struct Cached {
    fingerprint: [u8; 32],
    until: Instant,
    result: Result<QuotaSnapshot, DriverError>,
}

pub struct AccountMonitor {
    executor: ExecutorRef,
    session: Option<Arc<dyn QuotaProvider>>,
    cache: Mutex<Option<Cached>>,
    #[cfg(test)]
    environment: Option<HashMap<String, String>>,
    #[cfg(test)]
    endpoints: Option<(String, String)>,
}

impl AccountMonitor {
    /// Wrap the native session provider when ready; monitor accounts on their own otherwise.
    #[must_use]
    pub fn new(executor: ExecutorRef, session: Option<Arc<dyn QuotaProvider>>) -> Option<Self> {
        if !matches!(
            executor.provider_id.as_str(),
            "opencode" | "pi" | "deepseek-harness"
        ) {
            return None;
        }
        Some(Self {
            executor,
            session,
            cache: Mutex::new(None),
            #[cfg(test)]
            environment: None,
            #[cfg(test)]
            endpoints: None,
        })
    }

    async fn requests(&self) -> Result<Vec<Request>, DriverError> {
        let environment: HashMap<String, String> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        #[cfg(test)]
        let environment = self.environment.clone().unwrap_or(environment);
        let provider = self.executor.provider_id.to_string();
        let requests =
            tokio::task::spawn_blocking(move || credentials::discover(&provider, &environment))
                .await
                .map_err(|_| unavailable("provider credentials unavailable"))??;
        #[cfg(test)]
        let mut requests = requests;
        #[cfg(test)]
        if let Some((go, deepseek)) = &self.endpoints {
            for request in &mut requests {
                request.endpoint = match request.kind {
                    Kind::Go => go.clone(),
                    Kind::DeepSeek | Kind::DeepSeekPlatform => deepseek.clone(),
                    Kind::Sub2Api => request.endpoint.clone(),
                };
            }
        }
        Ok(requests)
    }

    async fn accounts(&self, force: bool) -> Result<QuotaSnapshot, DriverError> {
        let mut cache = self.cache.lock().await;
        // Discover after taking the gate: a queued reader must not restore an older account.
        let requests = self.requests().await?;
        let fingerprint = identity(&requests);
        if !force
            && let Some(cached) = cache.as_ref()
            && cached.fingerprint == fingerprint
            && Instant::now() < cached.until
        {
            return cached.result.clone();
        }
        // Never reuse another account's successful snapshot after a failed refresh.
        *cache = None;
        let result = fetch(requests).await;
        if identity(&self.requests().await?) != fingerprint {
            return Err(unavailable("provider credentials changed during refresh"));
        }
        let ttl = if result.is_ok() { 60 } else { 15 };
        *cache = Some(Cached {
            fingerprint,
            until: Instant::now() + Duration::from_secs(ttl),
            result: result.clone(),
        });
        result
    }
}

fn identity(requests: &[Request]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for request in requests {
        for part in [&request.label, &request.endpoint, &request.key] {
            digest.update(part.len().to_le_bytes());
            digest.update(part.as_bytes());
        }
    }
    digest.finalize().into()
}

async fn fetch(requests: Vec<Request>) -> Result<QuotaSnapshot, DriverError> {
    let client = http::client()?;
    let mut windows = Vec::new();
    let mut balances = Vec::new();
    let mut last_error = None;
    let mut go = false;
    let results = futures::future::join_all(requests.into_iter().map(|r| {
        let client = &client;
        async move {
            let payload = http::get(client, &r).await?;
            match r.kind {
                Kind::Go => Ok((projection::go(&payload)?, Vec::new())),
                Kind::DeepSeek => Ok((Vec::new(), projection::deepseek(&payload)?)),
                Kind::DeepSeekPlatform => {
                    Ok((Vec::new(), projection::deepseek_platform(&payload)?))
                }
                Kind::Sub2Api => Ok((Vec::new(), projection::sub2api(&payload, &r.label)?)),
            }
        }
    }))
    .await;
    for result in results {
        match result {
            Ok((w, b)) => {
                go |= !w.is_empty();
                windows.extend(w);
                balances.extend(b);
            }
            Err(error) => last_error = Some(error),
        }
    }
    if windows.is_empty() && balances.is_empty() {
        return Err(last_error.unwrap_or_else(|| unavailable("no supported provider balance")));
    }
    let now = chrono::Utc::now().timestamp_millis();
    Ok(QuotaSnapshot {
        windows,
        balances,
        collected_at_unix_ms: now,
        stale_after_unix_ms: now + 60_000,
        source: "provider_api".into(),
        confidence: "provider_reported".into(),
        parser_version: if go {
            "opencode-go-usage-v1"
        } else {
            "provider-balance-v1"
        }
        .into(),
        ..Default::default()
    })
}

fn unavailable(message: &str) -> DriverError {
    DriverError::CapabilityUnavailable(message.into())
}
fn invalid() -> DriverError {
    DriverError::Protocol("provider quota response invalid".into())
}

#[async_trait]
impl QuotaProvider for AccountMonitor {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    async fn read_quota(&self, id: Option<&str>) -> Result<QuotaSnapshot, DriverError> {
        self.read_quota_with_refresh(id, false).await
    }
    async fn read_quota_with_refresh(
        &self,
        id: Option<&str>,
        force: bool,
    ) -> Result<QuotaSnapshot, DriverError> {
        let mut quota = self.accounts(force).await?;
        if let Some(session) = &self.session
            && id.is_some()
            && let Ok(native) = session.read_quota(id).await
        {
            quota.context = native.context;
        }
        Ok(quota)
    }
    async fn read_session_usage(
        &self,
        id: &str,
    ) -> Option<driver_api::usage::SessionUsageSnapshot> {
        self.session.as_ref()?.read_session_usage(id).await
    }
}
