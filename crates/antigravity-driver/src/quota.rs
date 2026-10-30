//! Read-only native /usage command; never submitted as a model prompt.
use super::{
    AntigravityDriver, Classified, DriverError, ENV_ALLOWLIST, ExecutorRef, FRAME_LIMIT, Stdio,
    Value, async_trait, auth_backoff_kind, classify, is_transport_error, record_auth_failure,
};
use driver_api::{QuotaProvider, QuotaSnapshot, QuotaWindow};
use tokio::io::AsyncReadExt;

// Native /usage includes silent login and quota refresh; observed cold starts exceed 20s.
const QUOTA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

fn resolve_quota(
    cache: &mut Option<QuotaSnapshot>,
    result: Result<QuotaSnapshot, DriverError>,
    force: bool,
) -> Result<QuotaSnapshot, DriverError> {
    match result {
        Ok(snapshot) => {
            *cache = Some(snapshot.clone());
            Ok(snapshot)
        }
        Err(error) if driver_api::failure::invalidates_account_quota(&error) => {
            *cache = None;
            Err(error)
        }
        Err(error) if force => Err(error),
        Err(error) => cache.clone().ok_or(error),
    }
}

fn parse_usage(value: &Value) -> Option<Vec<QuotaWindow>> {
    if value["event"] != "command_result" || value["command"]["name"] != "usage" {
        return None;
    }
    let groups = value["command"]["data"]["groups"].as_array()?;
    let mut windows = Vec::new();
    for group in groups {
        let scope = group["name"].as_str()?;
        for bucket in group["buckets"].as_array()? {
            let minutes = match bucket["window"].as_str()? {
                "5h" => 300,
                "weekly" => 10_080,
                _ => continue,
            };
            let fraction = bucket["remaining_fraction"].as_f64()?;
            if !(0.0..=1.0).contains(&fraction) {
                return None;
            }
            let reset =
                chrono::DateTime::parse_from_rfc3339(bucket["reset_time"].as_str()?).ok()?;
            windows.push(QuotaWindow {
                id: format!("{scope}:{}", bucket["id"].as_str()?),
                scope: Some(scope.to_owned()),
                window_minutes: minutes,
                used_percent: Some((1.0 - fraction) * 100.0),
                resets_at_unix: Some(reset.timestamp()),
            });
        }
    }
    windows.sort_by_key(|window| window.window_minutes);
    (!windows.is_empty()).then_some(windows)
}

#[async_trait]
impl QuotaProvider for AntigravityDriver {
    async fn read_session_usage(
        &self,
        id: &str,
    ) -> Option<driver_api::usage::SessionUsageSnapshot> {
        let process = {
            let processes = self.processes.lock().await;
            processes.get(id).cloned().or_else(|| {
                processes
                    .values()
                    .find(|p| p.native_id.lock().unwrap().as_deref() == Some(id))
                    .cloned()
            })
        }?;
        let mut usage = process
            .usage
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(driver_api::usage::SessionUsageSnapshot::new);
        if let Some(context) = super::session_usage::read(&process).await {
            usage.context_used_tokens = context.context_used_tokens;
            usage.context_window_tokens = context.context_window_tokens;
            usage.observed_at_unix_ms = context.observed_at_unix_ms;
        }
        usage.has_data().then_some(usage)
    }

    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }

    async fn read_quota(
        &self,
        native_session_id: Option<&str>,
    ) -> Result<QuotaSnapshot, DriverError> {
        self.read_quota_with_refresh(native_session_id, false).await
    }

    async fn read_quota_with_refresh(
        &self,
        _: Option<&str>,
        force: bool,
    ) -> Result<QuotaSnapshot, DriverError> {
        let mut cache = self.quota_cache.lock().await;
        // A failure from a live turn overrides even a still-fresh account reading.
        if let Some(kind) = auth_backoff_kind().await {
            return resolve_quota(
                &mut cache,
                Err(DriverError::NotConfigured(kind.into())),
                force,
            );
        }
        let now = chrono::Utc::now().timestamp_millis();
        if let Some(snapshot) = cache
            .as_ref()
            .filter(|s| !force && now < s.stale_after_unix_ms)
        {
            return Ok(snapshot.clone());
        }
        let result = tokio::time::timeout(QUOTA_TIMEOUT, async {
            if let Some(kind) = auth_backoff_kind().await {
                return Err(DriverError::NotConfigured(kind.into()));
            }
            // Never race native credential refresh with an active session.
            let slot = self.spawn.clone().try_lock_owned().map_err(|_| {
                DriverError::Disconnected("agy session is active; quota refresh deferred".into())
            })?;
            let mut command = tokio::process::Command::new(&self.executable);
            command
                .args(["--print=/usage", "--output-format", "stream-json"])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            for variable in ENV_ALLOWLIST {
                if let Some(value) = agent_runtime::inherited_variable(variable) {
                    command.env(variable, value);
                }
            }
            agent_runtime::apply_proxy_environment(&mut command);
            let (bytes, status) = query(command, super::SpawnSlot { _guard: slot }).await?;
            let mut windows = None;
            let mut success = false;
            for line in bytes.split(|byte| *byte == b'\n') {
                let Ok(value) = serde_json::from_slice::<Value>(line) else {
                    continue;
                };
                if value["event"] == "result"
                    && value["result"]["status"] == "ERROR"
                    && value["result"]["error"]
                        .as_str()
                        .is_some_and(is_transport_error)
                {
                    return Err(DriverError::CapabilityUnavailable(
                        "quota_network_error".into(),
                    ));
                }
                if let Some(Classified::AuthFailure(kind, _)) =
                    classify(&String::from_utf8_lossy(line))
                {
                    record_auth_failure(kind).await;
                    return Err(DriverError::NotConfigured(kind.into()));
                }
                if let Some(parsed) = parse_usage(&value) {
                    windows = Some(parsed);
                }
                if value["event"] == "result" && value["result"]["status"] == "SUCCESS" {
                    success = true;
                }
            }
            if !status.success() || !success {
                return Err(DriverError::Protocol(
                    "agy quota command did not succeed".into(),
                ));
            }
            let now = chrono::Utc::now().timestamp_millis();
            Ok(QuotaSnapshot {
                session_usage: None,
                windows: windows
                    .ok_or_else(|| DriverError::Protocol("agy quota payload unavailable".into()))?,
                collected_at_unix_ms: now,
                stale_after_unix_ms: now + 60_000,
                source: "native_pull".into(),
                confidence: "provider_reported".into(),
                parser_version: "agy-usage-v1".into(),
                ..QuotaSnapshot::default()
            })
        })
        .await;
        resolve_quota(
            &mut cache,
            result.unwrap_or_else(|_| {
                Err(DriverError::CapabilityUnavailable("quota_timeout".into()))
            }),
            force,
        )
    }
}

// The UI observer may time out or disappear. The owned worker keeps the
// original helper and its shared auth slot through positive OS retirement.
async fn query(
    command: tokio::process::Command,
    slot: super::SpawnSlot,
) -> Result<(Vec<u8>, std::process::ExitStatus), DriverError> {
    let (_observer, mut cancelled) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let abandoned = || DriverError::Disconnected("agy quota caller left".into());
        if cancelled.try_recv() == Err(tokio::sync::oneshot::error::TryRecvError::Closed) {
            return Err(abandoned());
        }
        let mut child = agent_runtime::OwnedChild::spawn(command)
            .map_err(|_| DriverError::Disconnected("agy quota spawn failed".into()))?;
        let result = tokio::select! {
            biased;
            _ = &mut cancelled => Err(abandoned()),
            result = tokio::time::timeout(QUOTA_TIMEOUT, async {
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| DriverError::Disconnected("agy quota stdout missing".into()))?;
                let mut bytes = Vec::new();
                stdout
                    .take((FRAME_LIMIT + 1) as u64)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| DriverError::Disconnected("agy quota read failed".into()))?;
                if bytes.len() > FRAME_LIMIT {
                    return Err(DriverError::Protocol(
                        "agy quota output exceeds limit".into(),
                    ));
                }
                let status = child
                    .wait()
                    .await
                    .map_err(|_| DriverError::Disconnected("agy quota wait failed".into()))?;
                Ok((bytes, status))
            }) => result.map_err(|_| DriverError::CapabilityUnavailable("quota_timeout".into())),
        };
        let mut delay = 1;
        while child.terminate().await.is_err() {
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        }
        drop(slot);
        result?
    })
    .await
    .map_err(|_| DriverError::Disconnected("agy quota worker ended without a result".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_api::SessionResumeCapability;
    use serde_json::json;

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_quota_helper_retires_its_tree_and_releases_auth_promptly() {
        let (root, executable) = crate::metadata_tests::fixture();
        let lock = crate::SpawnLock::default();
        let slot = crate::SpawnSlot {
            _guard: lock.clone().lock_owned().await,
        };
        let mut command = tokio::process::Command::new(executable);
        command
            .arg("--print=/usage")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let work = tokio::spawn(async move { query(command, slot).await });
        crate::metadata_tests::cancelled(root.path(), work, lock).await;
    }

    #[test]
    fn account_rejections_clear_cached_quota_even_on_manual_refresh() {
        for kind in [
            "authentication_failed",
            "subscription_required",
            "access_denied",
            "driver_not_configured",
        ] {
            for force in [false, true] {
                for stale_after in [1, i64::MAX] {
                    let mut cache = Some(QuotaSnapshot {
                        stale_after_unix_ms: stale_after,
                        ..Default::default()
                    });
                    let error = resolve_quota(
                        &mut cache,
                        Err(DriverError::NotConfigured(kind.into())),
                        force,
                    )
                    .unwrap_err();
                    assert_eq!(driver_api::failure::kind(&error), kind);
                    assert!(cache.is_none());
                    assert!(
                        resolve_quota(
                            &mut cache,
                            Err(DriverError::CapabilityUnavailable("quota_timeout".into())),
                            false
                        )
                        .is_err()
                    );
                }
            }
        }
    }

    #[test]
    fn transient_quota_errors_keep_cache_without_hiding_manual_refresh_errors() {
        let snapshot = QuotaSnapshot {
            stale_after_unix_ms: 1,
            ..Default::default()
        };
        let mut cache = Some(snapshot.clone());
        let error = || DriverError::CapabilityUnavailable("quota_network_error".into());
        assert_eq!(
            resolve_quota(&mut cache, Err(error()), false).unwrap(),
            snapshot
        );
        assert!(resolve_quota(&mut cache, Err(error()), true).is_err());
        assert_eq!(cache, Some(snapshot));
    }

    #[tokio::test]
    async fn active_session_returns_stale_cache_without_waiting_or_spawning() {
        let mut driver = AntigravityDriver::new(
            "antigravity",
            "ins_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "/nonexistent/agy".into(),
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        driver.spawn = super::super::SpawnLock::default();
        let _active = super::super::acquire_spawn_slot(&driver.spawn).await;
        let stale = QuotaSnapshot {
            session_usage: None,
            stale_after_unix_ms: 1,
            source: "native_pull".into(),
            ..QuotaSnapshot::default()
        };
        *driver.quota_cache.lock().await = Some(stale.clone());
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            driver.read_quota(None),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, stale);
        let fresh = QuotaSnapshot {
            session_usage: None,
            stale_after_unix_ms: i64::MAX,
            ..stale
        };
        *driver.quota_cache.lock().await = Some(fresh.clone());
        assert_eq!(driver.read_quota(None).await.unwrap(), fresh);
        // Manual refresh must bypass fresh cache but respect active-session guards.
        assert!(driver.read_quota_with_refresh(None, true).await.is_err());
        assert_eq!(*driver.quota_cache.lock().await, Some(fresh));
    }
    #[tokio::test]
    #[ignore = "requires installed authenticated agy; read-only, no model turn"]
    async fn real_readonly_quota() {
        let executable = std::env::var_os("AGY_QUOTA_EXECUTABLE").expect("AGY_QUOTA_EXECUTABLE");
        let driver = AntigravityDriver::new(
            "antigravity",
            "ins_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            executable.into(),
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let snapshot = driver.read_quota(None).await.unwrap();
        assert!(!snapshot.windows.is_empty());
        assert!(snapshot.windows.iter().all(|window| window.scope.is_some()));
        assert_eq!(driver.read_quota(None).await.unwrap(), snapshot);
    }
    #[test]
    fn parses_scoped_native_usage_and_rejects_invalid_fraction() {
        let mut value = json!({"event":"command_result","command":{"name":"usage","data":{"groups":[{"name":"Gemini Models","buckets":[{"id":"gemini-5h","window":"5h","remaining_fraction":0.75,"reset_time":"2026-09-08T09:53:00Z"}]}]}}});
        let windows = parse_usage(&value).unwrap();
        assert_eq!(windows[0].used_percent, Some(25.0));
        assert_eq!(windows[0].scope.as_deref(), Some("Gemini Models"));
        value["command"]["data"]["groups"][0]["buckets"][0]["remaining_fraction"] = json!(2);
        assert!(parse_usage(&value).is_none());
        assert!(parse_usage(&json!({"event":"result"})).is_none());
    }
}
