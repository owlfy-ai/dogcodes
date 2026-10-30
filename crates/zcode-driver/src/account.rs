//! Connection-scoped native account grants; no credential file is changed.
use super::{Connection, DriverError, ENV_ALLOWLIST, Value, ZcodeLaunch, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

const SCRIPT: &str = concat!(
    "const __dogcodesNative = (() => { const module = {exports:{}};\n",
    include_str!("credentials.cjs"),
    "\nreturn module.exports; })();\n",
    "const __dogcodesQuota = (() => { const module = {exports:{}};\n",
    include_str!("quota-reader.cjs"),
    "\nreturn module.exports; })();\n",
    "const __dogcodesCaptcha = (() => { const module = {exports:{}};\n",
    include_str!("account-captcha.cjs"),
    "\nreturn module.exports; })();\n",
    "const __dogcodesProject = (() => { const module = {exports:{}};\n",
    include_str!("project-token.cjs"),
    "\nreturn module.exports; })();\n",
    include_str!("account-reader.cjs"),
    "\naccountMain();"
);

const ACCOUNT_ENV: &[&str] = &[
    "ZCODE_CREDENTIAL_SECRET",
    "ZCODE_DATA_BASE_DIR",
    "ZCODE_DESKTOP_HOME_DIR",
    "ZCODE_BASE_URL",
    "ZCODE_ENV",
    "ZAI_BUSINESS_BASE_URL",
    "ZAI_TEST_BUSINESS_BASE_URL",
    "ZAI_PRODUCTION_BUSINESS_BASE_URL",
    "BIGMODEL_API_BASE_URL",
    "BIGMODEL_TEST_API_BASE_URL",
    "BIGMODEL_PRODUCTION_API_BASE_URL",
    "USERPROFILE",
    "SYSTEMROOT",
    "WINDIR",
];

#[derive(Clone)]
pub(super) struct Account {
    launch: ZcodeLaunch,
    builtin: PathBuf,
    grant: Value,
    checked: Option<tokio::time::Instant>,
    unsupported: bool,
}

impl Account {
    pub(super) async fn quota_read(
        launch: &ZcodeLaunch,
        identity_only: bool,
        admission: std::sync::Arc<tokio::sync::Mutex<()>>,
    ) -> Result<Value, DriverError> {
        let account = Self::new(launch)
            .ok_or_else(|| DriverError::CapabilityUnavailable("quota_unavailable".into()))?;
        // Keep the existing twelve-second query budget while waiting for the
        // previous helper's actual exit. Cancelled queued callers never spawn.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
        let permit = tokio::time::timeout_at(deadline, admission.lock_owned())
            .await
            .map_err(|_| DriverError::CapabilityUnavailable("quota_unavailable".into()))?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(DriverError::CapabilityUnavailable(
                "quota_unavailable".into(),
            ));
        }
        let value = account.run_owned(
            json!({"action":if identity_only {"quota_identity"} else {"quota"},"builtinFile":account.builtin}),
            remaining,
            None,
            Some(permit),
        ).await?;
        if let Some(reason) = value["unavailable"].as_str() {
            return Err(DriverError::CapabilityUnavailable(
                if reason == "quota_login_required" {
                    "quota_login_required"
                } else {
                    "quota_unavailable"
                }
                .into(),
            ));
        }
        Ok(value)
    }
    pub(super) fn new(launch: &ZcodeLaunch) -> Option<Self> {
        launch.cli_script.is_file().then_some(())?;
        Some(Self {
            launch: launch.clone(),
            builtin: launch.bundled_provider_config()?,
            grant: Value::Null,
            checked: None,
            unsupported: false,
        })
    }

    pub(super) async fn runtime(
        &self,
        request: &Value,
        owner: tokio::sync::watch::Receiver<bool>,
    ) -> Value {
        let original = owner.clone();
        let result = self
            .run_owned(
                json!({
                    "action":"runtime", "request":request,
                    "authorized":self.grant["authorized"], "checkedAt":self.grant["checkedAt"],
                    "version":self.grant["version"],
                }),
                Duration::from_secs(150),
                Some(owner),
                None,
            )
            .await;
        result
            .ok()
            .filter(|_| !*original.borrow() && original.has_changed().is_ok())
            .unwrap_or_else(|| {
                json!({"headersApplied":false,
            "errorMessage":"ZCode 账号验证未完成，请检查原生登录状态后重试。"})
            })
    }

    async fn run_owned(
        &self,
        input: Value,
        timeout: Duration,
        owner: Option<tokio::sync::watch::Receiver<bool>>,
        permit: Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Result<Value, DriverError> {
        let fail = || {
            DriverError::NotConfigured(
                "ZCode 账号信息暂时不可用，请检查原生登录状态后重试。".into(),
            )
        };
        let mut command = Command::new(
            agent_runtime::electron_node_executable(&self.launch.app_binary).map_err(|_| fail())?,
        );
        command
            .arg("-e")
            .arg(SCRIPT)
            .env_clear()
            .env("ELECTRON_RUN_AS_NODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        agent_runtime::apply_platform_environment(&mut command);
        for key in ENV_ALLOWLIST.iter().chain(ACCOUNT_ENV) {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        agent_runtime::apply_proxy_environment(&mut command);
        // Own stdin/output and physical cleanup even after the query observer
        // disappears. The caller never receives account data before scope exit.
        let (observer, mut abandoned) = tokio::sync::oneshot::channel::<()>();
        let result = tokio::spawn(async move {
            // Admission belongs to the original helper through physical exit,
            // even after the caller releases its cache guard by cancellation.
            let _permit = permit;
            if !matches!(
                abandoned.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ) || owner
                .as_ref()
                .is_some_and(|owner| *owner.borrow() || owner.has_changed().is_err())
            {
                return Err(fail());
            }
            let mut child = agent_runtime::OwnedChild::spawn(command).map_err(|_| fail())?;
            let retired = async move {
                if let Some(mut owner) = owner {
                    loop {
                        if *owner.borrow_and_update() || owner.changed().await.is_err() {
                            return;
                        }
                    }
                }
                std::future::pending::<()>().await;
            };
            let reading = tokio::time::timeout(timeout, async {
                let mut stdin = child.stdin.take().ok_or_else(fail)?;
                stdin
                    .write_all(input.to_string().as_bytes())
                    .await
                    .map_err(|_| fail())?;
                drop(stdin);
                let output = child.stdout.take().ok_or_else(fail)?;
                // Ordinary descendants can keep stdout open after the actual
                // account leader exits. Observe the leader concurrently so the
                // original scope is retired and EOF is no longer held hostage.
                let (status, bytes) =
                    tokio::try_join!(async { child.wait().await.map_err(|_| fail()) }, async {
                        let mut bytes = Vec::new();
                        output
                            .take(1_048_577)
                            .read_to_end(&mut bytes)
                            .await
                            .map_err(|_| fail())?;
                        if bytes.len() > 1_048_576 {
                            return Err(fail());
                        }
                        Ok(bytes)
                    },)?;
                if !status.success() {
                    return Err(fail());
                }
                let value: Value = serde_json::from_slice(&bytes).map_err(|_| fail())?;
                if value.get("error").is_some() {
                    return Err(fail());
                }
                Ok(value)
            });
            let result = tokio::select! {
                _ = &mut abandoned => Err(fail()),
                () = retired => Err(fail()),
                result = reading => result.map_err(|_| fail()),
            };
            let mut delay = 1;
            while child.terminate().await.is_err() {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(30);
            }
            result?
        })
        .await
        .map_err(|_| fail())?;
        drop(observer);
        result
    }
}

impl Connection {
    pub(super) async fn refresh_account(&mut self, force: bool) -> Result<(), DriverError> {
        let Some(account) = &self.account else {
            return Ok(());
        };
        if account.unsupported
            || (!force
                && account
                    .checked
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(30)))
        {
            return Ok(());
        }
        let worker = account.clone();
        // Cancellation or failure must not leave a previous account authorized.
        if let Some(account) = &mut self.account {
            account.grant = Value::Null;
            account.checked = None;
        }
        let grant = worker
            .run_owned(
                json!({"action":"inspect","builtinFile":worker.builtin}),
                Duration::from_secs(22),
                Some(self.native.closing()),
                None,
            )
            .await?;
        let result = self
            .request_inner(
                "provider/updateAccountConfig",
                grant["overlay"].clone(),
                None,
            )
            .await;
        match result {
            Ok(ack) if ack["receivedRevision"] == grant["overlay"]["revision"] => {
                if let Some(account) = &mut self.account {
                    account.grant = grant;
                    account.checked = Some(tokio::time::Instant::now());
                }
                Ok(())
            }
            Err(DriverError::CapabilityUnavailable(_)) => {
                if let Some(account) = &mut self.account {
                    account.unsupported = true;
                }
                Ok(())
            }
            Err(error) => Err(error),
            _ => Err(DriverError::Protocol(
                "ZCode 未确认账号配置，请重新检测 Agent。".into(),
            )),
        }
    }

    pub(super) async fn provider_auth(&self, request: &Value) -> Value {
        match &self.account {
            Some(account) => account.runtime(request, self.native.closing()).await,
            None => {
                json!({"headersApplied":false,"errorMessage":"请在 ZCode 中登录并重新检测 Agent。"})
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_api::{AgentDriver, SessionResumeCapability};

    #[cfg(unix)]
    fn fixture(ack: &str) -> (tempfile::TempDir, Connection) {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        // Account discovery is a separate helper invocation; the native process
        // must confirm the exact overlay before serving a metadata request.
        std::fs::write(&runtime, format!(r#"#!/bin/sh
if [ "$1" = "-e" ]; then
  read input
  echo '{{"overlay":{{"revision":"fixture","providers":{{}},"states":{{}}}},"authorized":{{}},"checkedAt":0,"version":"fixture"}}'
  exit 0
fi
read request
case "$request" in
  *provider/updateAccountConfig*) echo '{{"id":1,"result":{{"receivedRevision":"{ack}"}}}}';;
  *) echo '{{"id":1,"error":{{"code":-32000}}}}';;
esac
read request
echo '{{"id":2,"result":{{"settings":{{"model":{{"available":[{{"ref":{{"providerId":"fixture","modelId":"GLM-5.3"}},"label":"GLM-5.3"}}]}}}}}}}}'
while read request; do :; done
"#)).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        let launch = ZcodeLaunch {
            app_binary: runtime,
            cli_script: root.path().join("Resources/glm/zcode.cjs"),
        };
        let mut connection = Connection::spawn(&launch).unwrap();
        // Create synthetic bundle metadata after spawn, keeping fixture setup
        // independent of any real home directory or legacy credentials.
        std::fs::create_dir_all(launch.cli_script.parent().unwrap()).unwrap();
        std::fs::write(&launch.cli_script, "").unwrap();
        let builtin = root
            .path()
            .join("Resources/config/provider/zcode-builtin.json");
        std::fs::create_dir_all(builtin.parent().unwrap()).unwrap();
        std::fs::write(builtin, "{}").unwrap();
        connection.account = Account::new(&launch);
        (root, connection)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_account_helper_is_physically_retired_before_returning_failure() {
        use crate::cleanup_tests::present;
        use crate::metadata::tests::{fixture, ids};
        let (root, launch) = fixture("account-stuck");
        std::fs::create_dir_all(launch.cli_script.parent().unwrap()).unwrap();
        std::fs::write(&launch.cli_script, "").unwrap();
        let builtin = root
            .path()
            .join("Resources/config/provider/zcode-builtin.json");
        std::fs::create_dir_all(builtin.parent().unwrap()).unwrap();
        std::fs::write(&builtin, "{}").unwrap();
        let account = Account::new(&launch).unwrap();
        let result = account
            .run_owned(
                json!({"action":"inspect","builtinFile":builtin}),
                Duration::from_millis(300),
                None,
                None,
            )
            .await;
        assert!(result.is_err());
        let processes = ids(root.path());
        assert_eq!(processes.len(), 2, "private account helper did not start");
        assert!(
            processes.iter().all(|pid| !present(*pid)),
            "failure returned before original helper cleanup"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn account_overlay_is_confirmed_before_native_catalog_queries() {
        let (_root, mut connection) = fixture("fixture");
        let snapshot = connection
            .request("workspace/readState", json!({}), None)
            .await
            .unwrap();
        assert_eq!(crate::models::catalog(&snapshot).models[0].label, "GLM-5.3");
        assert!(connection.account.as_ref().unwrap().checked.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mismatched_overlay_receipt_does_not_authorize_or_query() {
        let (_root, mut connection) = fixture("other-account-revision");
        let result = connection
            .request("workspace/readState", json!({}), None)
            .await;
        assert!(matches!(result, Err(DriverError::Protocol(_))));
        let account = connection.account.as_ref().unwrap();
        assert!(account.grant.is_null());
        assert!(account.checked.is_none());
    }

    #[tokio::test]
    #[ignore = "queries local ZCode account entitlements, short-lived auth and model catalog; no model turn"]
    async fn local_account_model_catalog() {
        let driver = crate::ZcodeDriver::connect_ready(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            ZcodeLaunch {
                app_binary: "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
                cli_script: "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs".into(),
            },
            None,
            SessionResumeCapability::Unknown,
        )
        .await
        .unwrap();
        let catalog = driver.model_catalog().await.unwrap();
        assert!(!catalog.models.is_empty());
        let account_model = catalog
            .models
            .iter()
            .find_map(|m| {
                let reference: Value = serde_json::from_str(&m.id).ok()?;
                reference["providerId"]
                    .as_str()?
                    .contains("individual-coding-plan")
                    .then_some(reference)
            })
            .expect("an entitled personal Coding Plan model must be discoverable");
        let provider = account_model["providerId"].as_str().unwrap();
        let family = if provider.contains("bigmodel") {
            "bigmodel"
        } else {
            "zai"
        };
        let guard = driver.ensure_connection().await.unwrap();
        let auth = guard.as_ref().unwrap().provider_auth(&json!({
            "providerId":provider,"modelSelection":account_model,
            "accountAccess":{"type":"zhipu-account","mode":"individual-coding-plan","accountType":family}
        })).await;
        assert_eq!(
            auth["headersApplied"], true,
            "native account runtime callback must succeed"
        );
        assert!(
            auth["requestAuth"]["apiKey"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
        drop(guard);
        for model in catalog.models {
            println!("{} {}", model.id, model.label);
        }
    }
}

#[cfg(all(test, unix))]
mod runtime_tests;
