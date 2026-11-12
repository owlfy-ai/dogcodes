//! Installs agents on a connected machine and follows them until they can be found there.
//! The shim does the work; this keeps the page informed, however long it takes.
use super::TargetHost;
use agent_core::Core;
use agent_domain::TargetId;
use agent_management::detect::installation_failure;
use agent_management::operation::{self, Failure, Request};
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentInstall {
    provider_id: String,
    pub(super) status: &'static str,
    message: Option<String>,
}

/// One at a time per machine: installers share `~/.local/bin` and shell profiles.
#[derive(Default)]
pub(super) struct Installs {
    current: Vec<AgentInstall>,
    turn: Arc<tokio::sync::Mutex<()>>,
}

fn explained(failure: Failure) -> String {
    if failure.kind == "disconnected" {
        "backendText.installConnectionLost".into()
    } else {
        super::agent_updates::message_of(failure)
    }
}

impl TargetHost {
    fn current_installs(&self, target: &TargetId) -> Vec<AgentInstall> {
        self.agent_installs
            .lock()
            .unwrap()
            .get(target)
            .map(|installs| installs.current.clone())
            .unwrap_or_default()
    }

    pub(super) async fn agent_installs(&self, core: &Core, target: &TargetId) -> Vec<AgentInstall> {
        let mut current = self.current_installs(target);
        // Failed probes survive reconnects and app restarts in installation records.
        // Keep an ongoing install or its explicit failure ahead of the last scan.
        if let Ok(snapshot) = core.bootstrap().await {
            for installation in snapshot.installations {
                if installation.target_id.as_ref() != Some(target)
                    || current
                        .iter()
                        .any(|i| i.provider_id == installation.provider_id)
                {
                    continue;
                }
                if let Some(message) =
                    installation_failure(installation.status, installation.diagnostic.as_deref())
                {
                    current.push(AgentInstall {
                        provider_id: installation.provider_id,
                        status: "failed",
                        message: Some(message.into()),
                    });
                }
            }
        }
        current
    }

    fn settle_install(&self, target: &TargetId, provider: &str, failure: Option<String>) {
        let mut installs = self.agent_installs.lock().unwrap();
        let Some(installs) = installs.get_mut(target) else {
            return;
        };
        installs.current.retain(|i| i.provider_id != provider);
        if let Some(message) = failure {
            installs.current.push(AgentInstall {
                provider_id: provider.to_owned(),
                status: "failed",
                message: Some(message),
            });
        }
    }

    /// Answers once it has started. It's over when `agent_installs` no longer says
    /// "installing": gone if the agent was found on the machine, otherwise failed.
    pub(super) fn install_agent(
        self: &Arc<Self>,
        core: &Arc<Core>,
        target: &TargetId,
        provider: &str,
    ) -> Result<Vec<AgentInstall>, String> {
        let machine = self.target(target)?;
        let turn = {
            let mut installs = self.agent_installs.lock().unwrap();
            let installs = installs.entry(target.clone()).or_default();
            if installs
                .current
                .iter()
                .any(|i| i.provider_id == provider && i.status == "installing")
            {
                return Ok(installs.current.clone());
            }
            installs.current.retain(|i| i.provider_id != provider);
            installs.current.push(AgentInstall {
                provider_id: provider.to_owned(),
                status: "installing",
                message: None,
            });
            installs.turn.clone()
        };
        let (host, core, target_id, provider) = (
            self.clone(),
            core.clone(),
            target.clone(),
            provider.to_owned(),
        );
        tokio::spawn(async move {
            let _turn = turn.lock().await;
            let request = Request::Install {
                provider: provider.clone(),
            };
            let installed = operation::call::<()>(&machine, &request)
                .await
                .map_err(explained);
            let failure = match installed {
                Ok(()) => host
                    .find_installed(&core, &target_id, &provider)
                    .await
                    .err(),
                Err(message) => Some(message),
            };
            host.settle_install(&target_id, &provider, failure);
        });
        Ok(self.current_installs(target))
    }

    /// An installer saying it finished doesn't mean anything can run it yet.
    async fn find_installed(
        &self,
        core: &Arc<Core>,
        target: &TargetId,
        provider: &str,
    ) -> Result<(), String> {
        self.scan_now(core, target).await?;
        let found = self
            .connections
            .lock()
            .unwrap()
            .get(target)
            .and_then(|connection| {
                connection
                    .snapshot()
                    .agents
                    .iter()
                    .find(|agent| agent.provider_id == provider)
                    .map(|agent| installation_failure(agent.status, agent.diagnostic.as_deref()))
            });
        match found {
            Some(None) => Ok(()),
            Some(Some(message)) => Err(message.to_owned()),
            None => Err("backendText.installNotFound".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_protocol::{
        ActorRef, CommandEnvelope, CoreCommand, InstallationStatus, RecordInstallationProbe,
    };

    #[test]
    fn install_failures_keep_safe_localized_error_codes() {
        assert_eq!(
            explained(Failure::new(
                "other",
                "backendText.installRuntimeIncompatible"
            )),
            "backendText.installRuntimeIncompatible"
        );
        assert_eq!(
            explained(Failure::new("other", "private command output")),
            "backendText.targetOperationFailed"
        );
    }

    #[tokio::test]
    async fn stored_failed_probes_reappear_as_errors_but_never_override_an_ongoing_install() {
        let core = Arc::new(Core::new(Arc::new(
            agent_store::SqliteStore::open_in_memory().unwrap(),
        )));
        let target = TargetId::new();
        let other = TargetId::new();
        for (provider, machine, status) in [
            ("kimi", Some(target.clone()), InstallationStatus::Broken),
            (
                "cursor",
                Some(target.clone()),
                InstallationStatus::NotConfigured,
            ),
            ("pi", Some(other), InstallationStatus::Broken),
            ("codex", None, InstallationStatus::Broken),
        ] {
            let mut command = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
                RecordInstallationProbe {
                    target_id: machine,
                    provider_id: provider.into(),
                    executable: format!("/bin/{provider}"),
                    fingerprint: "fixture".into(),
                    status,
                    version: None,
                    version_text: None,
                    diagnostic: Some("agent_runtime_incompatible".into()),
                    elapsed_millis: 1,
                },
            ));
            command.actor = ActorRef::SystemRecovery;
            core.execute(command).await.unwrap();
        }
        let host = TargetHost::default();
        let errors = host.agent_installs(&core, &target).await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].provider_id, "kimi");
        assert_eq!(errors[0].status, "failed");
        assert_eq!(
            errors[0].message.as_deref(),
            Some("backendText.installRuntimeIncompatible")
        );
        host.agent_installs
            .lock()
            .unwrap()
            .entry(target.clone())
            .or_default()
            .current
            .push(AgentInstall {
                provider_id: "kimi".into(),
                status: "installing",
                message: None,
            });
        let errors = host.agent_installs(&core, &target).await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].status, "installing");
    }
}
