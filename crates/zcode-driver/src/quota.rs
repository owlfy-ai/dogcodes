use super::{DriverError, QuotaSnapshot, ZcodeLaunch};

#[derive(Default)]
pub(super) struct Cache {
    next_attempt_ms: i64,
    snapshot: Option<QuotaSnapshot>,
    identity: Option<String>,
    pub(super) failure: Option<&'static str>,
}

impl Cache {
    pub(super) async fn read(
        &mut self,
        launch: &ZcodeLaunch,
        now: i64,
        force: bool,
    ) -> Option<QuotaSnapshot> {
        let identity = match super::account::Account::quota_read(launch, true).await {
            Ok(value) => value["identity"].as_str().map(str::to_owned),
            Err(error) => {
                self.snapshot = None;
                self.identity = None;
                self.next_attempt_ms = 0;
                self.failure = Some(
                    if matches!(error, DriverError::CapabilityUnavailable(ref r) if r == "quota_login_required")
                    {
                        "quota_login_required"
                    } else {
                        "quota_unavailable"
                    },
                );
                return None;
            }
        };
        if self.identity != identity {
            self.snapshot = None;
            self.next_attempt_ms = 0;
            self.identity = identity;
        }
        if !force && now < self.next_attempt_ms {
            return self.snapshot.clone();
        }
        self.next_attempt_ms = now + 60_000;
        match tokio::time::timeout(std::time::Duration::from_secs(18), pull(launch)).await {
            Ok(Ok(snapshot)) => {
                self.snapshot = Some(snapshot);
                self.failure = None;
            }
            Ok(Err(DriverError::CapabilityUnavailable(reason)))
                if reason == "quota_login_required" =>
            {
                self.snapshot = None;
                self.failure = Some("quota_login_required");
            }
            _ => self.failure = Some("quota_unavailable"),
        }
        if force && self.failure.is_some() {
            return None;
        }
        self.snapshot.clone()
    }
}

async fn pull(launch: &ZcodeLaunch) -> Result<QuotaSnapshot, DriverError> {
    let value = super::account::Account::quota_read(launch, false).await?;
    serde_json::from_value(value["snapshot"].clone())
        .map_err(|_| DriverError::CapabilityUnavailable("quota_unavailable".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "reads authorized local ZCode credentials and official quota only"]
    async fn real_native_credentials_quota() {
        let launch = ZcodeLaunch {
            app_binary: "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
            cli_script: "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs".into(),
        };
        let snapshot = tokio::time::timeout(std::time::Duration::from_secs(18), pull(&launch))
            .await
            .unwrap()
            .unwrap();
        assert!(snapshot.windows.iter().any(|w| w.window_minutes == 300));
    }
}
