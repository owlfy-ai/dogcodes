//! Installs agents on a connected machine and follows them until they can be found there.
//! The shim does the work; this keeps the page informed, however long it takes.
use super::TargetHost;
use agent_core::Core;
use agent_domain::TargetId;
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
        "和这台主机的连接断开了，安装可能没有完成。请重新连接后再试".into()
    } else {
        super::agent_updates::message_of(failure)
    }
}

impl TargetHost {
    pub(super) fn agent_installs(&self, target: &TargetId) -> Vec<AgentInstall> {
        self.agent_installs
            .lock()
            .unwrap()
            .get(target)
            .map(|installs| installs.current.clone())
            .unwrap_or_default()
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
        Ok(self.agent_installs(target))
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
            .is_some_and(|connection| {
                connection
                    .snapshot()
                    .agents
                    .iter()
                    .any(|agent| agent.provider_id == provider)
            });
        found
            .then_some(())
            .ok_or_else(|| "安装程序已结束，但在这台主机上没有找到它。请重新检测".to_owned())
    }
}
