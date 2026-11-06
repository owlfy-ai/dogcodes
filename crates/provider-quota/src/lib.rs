//! Read-only account monitoring, independent of an agent's model/session readiness.
//! The accounts are read where the agent is installed, by `agent_management`; here their
//! answers become validated quota readings.
mod http;
mod projection;
#[cfg(test)]
mod tests;

use agent_domain::ExecutorRef;
use agent_management::{
    operation::{self, Failure},
    provider_account::{self, Agent, Credential, Kind, Reading, Usage},
};
use agent_runtime::{SharedTarget, TargetExecutable};
use async_trait::async_trait;
use driver_api::{DriverError, QuotaProvider, QuotaSnapshot};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const CACHE_FOR: Duration = Duration::from_mins(1);
const FAILURE_CACHE_FOR: Duration = Duration::from_secs(15);

struct Cached {
    fingerprint: String,
    until: Instant,
    result: Result<QuotaSnapshot, DriverError>,
}

pub struct AccountMonitor {
    executor: ExecutorRef,
    agent: Agent,
    target: SharedTarget,
    session: Option<Arc<dyn QuotaProvider>>,
    cache: Mutex<Option<Cached>>,
    #[cfg(test)]
    environment: Option<provider_account::Environment>,
    #[cfg(test)]
    endpoints: Option<(String, String)>,
}

impl AccountMonitor {
    /// Wrap the native session provider when ready; monitor accounts on their own otherwise.
    #[must_use]
    pub fn new(
        executor: ExecutorRef,
        executable: &TargetExecutable,
        version: Option<String>,
        session: Option<Arc<dyn QuotaProvider>>,
    ) -> Option<Self> {
        if !matches!(
            executor.provider_id.as_str(),
            "opencode" | "pi" | "deepseek-harness"
        ) {
            return None;
        }
        Some(Self {
            agent: Agent {
                provider: executor.provider_id.to_string(),
                executable: executable.path.to_string_lossy().into_owned(),
                version,
            },
            target: executable.target.clone(),
            executor,
            session,
            cache: Mutex::new(None),
            #[cfg(test)]
            environment: None,
            #[cfg(test)]
            endpoints: None,
        })
    }

    /// Checking which account is signed in takes a process when `OpenCode` or another
    /// machine must answer, and a running conversation reads quota every few seconds. Those
    /// keep a reading until it expires or is refreshed by hand; the rest just read a file.
    fn keeps_readings(&self) -> bool {
        !self.target.is_local() || self.agent.provider == "opencode"
    }

    async fn read(&self, known: Option<String>) -> Result<Usage, Failure> {
        if !self.target.is_local() {
            let request = operation::Request::ProviderAccount {
                agent: self.agent.clone(),
                known,
            };
            return operation::call(&self.target, &request).await;
        }
        let environment: provider_account::Environment = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        #[cfg(test)]
        let environment = self.environment.clone().unwrap_or(environment);
        provider_account::read(
            &self.agent,
            &environment,
            known.as_deref(),
            |credential: Credential| {
                #[cfg(test)]
                let credential = self.redirected(credential);
                async move { http::get(&credential).await }
            },
        )
        .await
    }

    #[cfg(test)]
    fn redirected(&self, mut credential: Credential) -> Credential {
        if let Some((go, deepseek)) = &self.endpoints {
            credential.endpoint = match credential.kind {
                Kind::Go => go.clone(),
                Kind::DeepSeek | Kind::DeepSeekPlatform => deepseek.clone(),
                Kind::Sub2Api => credential.endpoint,
            };
        }
        credential
    }

    async fn accounts(&self, force: bool) -> Result<QuotaSnapshot, DriverError> {
        // Reading under the gate: a queued reader must not restore an older account.
        let mut cache = self.cache.lock().await;
        let current = cache
            .as_ref()
            .filter(|cached| !force && Instant::now() < cached.until);
        if let Some(cached) = current
            && self.keeps_readings()
        {
            return cached.result.clone();
        }
        let known = current.map(|cached| cached.fingerprint.clone());
        match self.read(known).await {
            Ok(Usage::Unchanged) => cache.as_ref().map_or_else(
                || Err(unavailable("quota_unavailable")),
                |c| c.result.clone(),
            ),
            Ok(Usage::Read {
                fingerprint,
                readings,
            }) => {
                // Never reuse another account's successful snapshot after a failed refresh.
                let result = snapshot(readings);
                let ttl = if result.is_ok() {
                    CACHE_FOR
                } else {
                    FAILURE_CACHE_FOR
                };
                *cache = Some(Cached {
                    fingerprint,
                    until: Instant::now() + ttl,
                    result: result.clone(),
                });
                result
            }
            Err(failure) => {
                let error = driver_error(failure);
                *cache = self.keeps_readings().then(|| Cached {
                    fingerprint: String::new(),
                    until: Instant::now() + FAILURE_CACHE_FOR,
                    result: Err(error.clone()),
                });
                Err(error)
            }
        }
    }
}

fn snapshot(readings: Vec<Reading>) -> Result<QuotaSnapshot, DriverError> {
    let mut windows = Vec::new();
    let mut balances = Vec::new();
    let mut last_error = None;
    let mut go = false;
    for reading in readings {
        let projected =
            reading
                .outcome
                .map_err(driver_error)
                .and_then(|payload| match reading.kind {
                    Kind::Go => Ok((projection::go(&payload)?, Vec::new())),
                    Kind::DeepSeek => Ok((Vec::new(), projection::deepseek(&payload)?)),
                    Kind::DeepSeekPlatform => {
                        Ok((Vec::new(), projection::deepseek_platform(&payload)?))
                    }
                    Kind::Sub2Api => {
                        Ok((Vec::new(), projection::sub2api(&payload, &reading.label)?))
                    }
                });
        match projected {
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

fn driver_error(failure: Failure) -> DriverError {
    match failure.kind.as_str() {
        "login" => DriverError::NotConfigured(failure.message),
        "capability" => DriverError::CapabilityUnavailable(failure.message),
        "protocol" => DriverError::Protocol(failure.message),
        _ => DriverError::Disconnected(failure.message),
    }
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
