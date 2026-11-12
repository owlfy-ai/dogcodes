//! Follows version checks and upgrades of a connected machine's agents. The shim decides
//! and does the work over there; this only asks, remembers the answer and reloads drivers.
use super::{TargetHost, agents};
use agent_core::Core;
use agent_domain::TargetId;
use agent_management::{
    operation::{self, Failure, Request, Upgraded},
    updates::{AgentUpdate, verified_version},
};
use agent_protocol::InstallationSummary;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const CACHE_LIFETIME: Duration = Duration::from_hours(6);
const FAILURE_LIFETIME: Duration = Duration::from_mins(1);

pub(super) struct Remembered {
    /// It changes with the version, which makes an older answer worthless.
    version: Option<String>,
    at: Instant,
    value: AgentUpdate,
}

fn blank(i: &InstallationSummary) -> AgentUpdate {
    AgentUpdate {
        installation_id: i.installation_id.to_string(),
        status: "checking".into(),
        current_version: i.version.clone(),
        desktop_version: None,
        latest_version: None,
        source: String::new(),
        message: None,
        can_upgrade: false,
        checked_at: 0,
        in_use: None,
    }
}

/// What to show of a failure on another machine. Its own words are in Chinese when written
/// for people, for the interface to translate; anything else isn't fit to show.
pub(super) fn message_of(failure: Failure) -> String {
    if failure.kind == "disconnected" {
        "backendText.targetOperationLost".into()
    } else if failure
        .message
        .strip_prefix("backendText.")
        .is_some_and(|key| !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric()))
        || failure
            .message
            .chars()
            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    {
        failure.message
    } else {
        "backendText.targetOperationFailed".into()
    }
}

impl TargetHost {
    fn remember(&self, i: &InstallationSummary, value: AgentUpdate) {
        self.agent_updates.lock().unwrap().insert(
            value.installation_id.clone(),
            Remembered {
                version: i.version.clone(),
                at: Instant::now(),
                value,
            },
        );
    }

    async fn installation_on(
        core: &Core,
        target: &TargetId,
        id: &str,
    ) -> Result<InstallationSummary, String> {
        core.bootstrap()
            .await
            .map_err(|e| e.to_string())?
            .installations
            .into_iter()
            .find(|i| i.target_id.as_ref() == Some(target) && i.installation_id.to_string() == id)
            .ok_or_else(|| "backendText.updateInstallationGone".to_owned())
    }

    pub(super) async fn agent_update(
        &self,
        core: &Core,
        target: &TargetId,
        id: &str,
        force: bool,
    ) -> Result<AgentUpdate, String> {
        let i = Self::installation_on(core, target, id).await?;
        if let Some(known) = self.agent_updates.lock().unwrap().get(id) {
            let lifetime = if known.value.status == "error" {
                FAILURE_LIFETIME
            } else {
                CACHE_LIFETIME
            };
            if known.value.status == "upgrading"
                || (!force && known.version == i.version && known.at.elapsed() < lifetime)
            {
                return Ok(known.value.clone());
            }
        }
        let machine = self.target(target)?;
        let request = Request::CheckUpdate {
            installation: Box::new(i.clone()),
        };
        let value = operation::call(&machine, &request)
            .await
            .unwrap_or_else(|failure: Failure| AgentUpdate {
                status: "error".into(),
                message: Some(message_of(failure)),
                ..blank(&i)
            });
        self.remember(&i, value.clone());
        Ok(value)
    }

    /// Answers once the upgrade has started; the page follows it with `agent_update`.
    pub(super) async fn upgrade_agent(
        self: &Arc<Self>,
        core: &Arc<Core>,
        target: &TargetId,
        id: &str,
    ) -> Result<AgentUpdate, String> {
        let i = Self::installation_on(core, target, id).await?;
        let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
        // A conversation on another of this host's agents isn't touched by this one's files.
        if snapshot.tasks.iter().any(|task| {
            task.active_primary_run
                .as_ref()
                .is_some_and(|run| run.executor.installation_id.as_deref() == Some(id))
        }) {
            return Err("backendText.updateWaitForTask".into());
        }
        let machine = self.target(target)?;
        let started = AgentUpdate {
            status: "upgrading".into(),
            message: Some("backendText.updateDownloading".into()),
            ..self
                .agent_updates
                .lock()
                .unwrap()
                .get(id)
                .map_or_else(|| blank(&i), |known| known.value.clone())
        };
        {
            let mut known = self.agent_updates.lock().unwrap();
            if known
                .get(id)
                .is_some_and(|known| known.value.status == "upgrading")
            {
                return Err("backendText.updateBusy".into());
            }
            known.insert(
                id.to_owned(),
                Remembered {
                    version: i.version.clone(),
                    at: Instant::now(),
                    value: started.clone(),
                },
            );
        }
        let (host, core, target, value) =
            (self.clone(), core.clone(), target.clone(), started.clone());
        tokio::spawn(async move {
            let request = Request::Upgrade {
                installation: Box::new(i.clone()),
            };
            let upgraded = operation::call::<Upgraded>(&machine, &request)
                .await
                .map_err(message_of);
            let finished = match upgraded {
                Ok(upgraded) => {
                    host.finish_upgrade(&core, &target, &i, upgraded, value.clone())
                        .await
                }
                Err(message) => Err(message),
            };
            match finished {
                Ok((fresh, update)) => {
                    if fresh.installation_id != i.installation_id {
                        host.remember(
                            &i,
                            AgentUpdate {
                                installation_id: i.installation_id.to_string(),
                                ..update.clone()
                            },
                        );
                    }
                    host.remember(&fresh, update);
                }
                Err(message) => host.remember(
                    &i,
                    AgentUpdate {
                        status: "error".into(),
                        can_upgrade: value.latest_version.is_some(),
                        message: Some(message),
                        ..value
                    },
                ),
            }
        });
        Ok(started)
    }

    /// The new binary means a new driver, and only then is it known to have taken effect.
    async fn finish_upgrade(
        &self,
        core: &Arc<Core>,
        target: &TargetId,
        i: &InstallationSummary,
        upgraded: Upgraded,
        mut value: AgentUpdate,
    ) -> Result<(InstallationSummary, AgentUpdate), String> {
        let Some(goal) = upgraded.goal else {
            return Ok((i.clone(), upgraded.update));
        };
        value.latest_version = Some(goal.clone());
        value.source.clone_from(&upgraded.update.source);
        value.message = Some("backendText.updateVerifying".into());
        self.remember(i, value.clone());
        self.scan_now(core, target).await?;
        let fresh = core
            .bootstrap()
            .await
            .map_err(|e| e.to_string())?
            .installations
            .into_iter()
            .find(|v| v.target_id.as_ref() == Some(target) && v.provider_id == i.provider_id)
            .ok_or("backendText.updateInstallationLost")?;
        let now = fresh
            .version
            .as_deref()
            .ok_or("backendText.updateVersionUnreadable")?;
        if !verified_version(&i.provider_id, now, &goal) {
            return Err("backendText.updateUnconfirmed".into());
        }
        value.installation_id = fresh.installation_id.to_string();
        value.current_version.clone_from(&fresh.version);
        value.status = "updated".into();
        value.message = Some("backendText.updateFinished".into());
        Ok((fresh, value))
    }

    /// A scan already running only overlaps with the same result.
    pub(super) async fn scan_now(&self, core: &Arc<Core>, target: &TargetId) -> Result<(), String> {
        let link = self.link(target)?;
        let connection = self
            .connections
            .lock()
            .unwrap()
            .get(target)
            .cloned()
            .ok_or("backendText.targetConnectionMissing")?;
        connection.update(|status| status.scanning = true);
        link.refresh_environment().await;
        let found = agents::scan(core, target, &link.target()).await;
        let scanned = found.is_some();
        connection.update(|status| {
            status.scanning = false;
            if let Some(found) = found {
                status.agents = found;
            }
        });
        scanned
            .then_some(())
            .ok_or_else(|| "backendText.targetOperationFailed".to_owned())
    }
}
