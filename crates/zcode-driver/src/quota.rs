use super::{DriverError, ENV_ALLOWLIST, QuotaSnapshot, ZcodeLaunch};
use std::process::Stdio;
use tokio::io::AsyncReadExt;

#[derive(Default)]
pub(super) struct Cache {
    next_attempt_ms: i64,
    snapshot: Option<QuotaSnapshot>,
    pub(super) failure: Option<&'static str>,
}

impl Cache {
    pub(super) async fn read(
        &mut self,
        launch: &ZcodeLaunch,
        now: i64,
        force: bool,
    ) -> Option<QuotaSnapshot> {
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
    let fail = || {
        DriverError::NotConfigured(
            "ZCode quota unavailable; check the selected model and native login".into(),
        )
    };
    let mut command = tokio::process::Command::new(
        agent_runtime::electron_node_executable(&launch.app_binary).map_err(|_| {
            DriverError::NotConfigured("ZCode background runtime is unavailable".into())
        })?,
    );
    command
        .arg("-e")
        .arg(concat!(include_str!("quota-reader.cjs"), "\nmain();"))
        .env_clear()
        .env("ELECTRON_RUN_AS_NODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in ENV_ALLOWLIST
        .iter()
        .copied()
        .chain(["ZCODE_CREDENTIAL_SECRET"])
    {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    agent_runtime::apply_proxy_environment(&mut command);
    let mut child = command.spawn().map_err(|_| fail())?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(fail)?
        .take(65537)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| fail())?;
    if bytes.len() > 65536 {
        return Err(fail());
    }
    if !child.wait().await.map_err(|_| fail())?.success() {
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        return Err(DriverError::CapabilityUnavailable(
            if value["error"] == "login_required" {
                "quota_login_required"
            } else {
                "quota_unavailable"
            }
            .into(),
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| fail())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "reads authorized local ZCode credentials and official quota only"]
    async fn real_native_credentials_quota() {
        let launch = ZcodeLaunch {
            app_binary: "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
            cli_script: "unused".into(),
        };
        let snapshot = tokio::time::timeout(std::time::Duration::from_secs(18), pull(&launch))
            .await
            .unwrap()
            .unwrap();
        assert!(snapshot.windows.iter().any(|w| w.window_minutes == 300));
    }
}
