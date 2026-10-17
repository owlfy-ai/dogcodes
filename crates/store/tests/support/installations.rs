//! Explicit installation fixtures. Never included by production store code.
#![allow(dead_code)]
use agent_core::{AppendRequest, Core, CoreRepository};
use agent_domain::{CommandId, ExecutorRef, ProviderId};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, InstallationStatus, NewEvent, RecordInstallationProbe,
    StreamRef, StreamType,
};
use driver_api::AgentDriver;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Stable, valid ULID-shaped identity for deterministic driver/task fixtures.
pub fn executor(provider: &str) -> ExecutorRef {
    let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let hash = Sha256::digest(provider.as_bytes());
    let suffix: String = hash[..25]
        .iter()
        .map(|byte| char::from(alphabet[usize::from(byte & 31)]))
        .collect();
    ExecutorRef {
        provider_id: ProviderId::parse(provider).unwrap(),
        installation_id: Some(format!("ins_0{suffix}")),
    }
}

/// Seed a persisted probe event for a fixture whose explicit identity is fixed.
pub async fn seed(store: &dyn CoreRepository, executor: &ExecutorRef) {
    let id = executor
        .installation_id
        .as_ref()
        .expect("fixture requires installation identity");
    agent_domain::InstallationId::parse(id).unwrap();
    if store
        .bootstrap()
        .await
        .unwrap()
        .installations
        .iter()
        .any(|installation| installation.installation_id.as_str() == id)
    {
        return;
    }
    let event = NewEvent::hub(
        StreamRef {
            stream_type: StreamType::Installation,
            stream_id: id.clone(),
        },
        "agent.probe_completed",
        serde_json::json!({
            "installationId":id, "providerId":executor.provider_id, "executable":format!("fixture:{id}"),
            "fingerprint":"deterministic-test-fixture", "status":"ready", "version":null,
            "versionText":null, "diagnostic":null, "probeElapsedMillis":0, "probedAt":chrono::Utc::now()
        }),
    );
    store
        .append(AppendRequest {
            request_fingerprint: None,
            command_id: CommandId::new(),
            idempotency_key: event.event_id.to_string(),
            expected_stream_version: None,
            artifacts: vec![],
            events: vec![event],
        })
        .await
        .unwrap();
}

pub async fn register(core: &Core, store: &dyn CoreRepository, driver: Arc<dyn AgentDriver>) {
    seed(store, driver.executor()).await;
    core.register_driver(driver).await;
}

/// Use the real Core probe command when a fixture can take a generated identity.
pub async fn probe(core: &Arc<Core>, provider: &str) -> ExecutorRef {
    let executable = format!("fixture:{provider}");
    let mut command = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
        RecordInstallationProbe {
            provider_id: provider.into(),
            executable: executable.clone(),
            fingerprint: "deterministic-test-fixture".into(),
            status: InstallationStatus::Ready,
            version: None,
            version_text: None,
            diagnostic: None,
            elapsed_millis: 0,
        },
    ));
    command.actor = ActorRef::SystemRecovery;
    core.execute(command).await.unwrap();
    let installation = core
        .bootstrap()
        .await
        .unwrap()
        .installations
        .into_iter()
        .find(|i| i.provider_id == provider && i.executable == executable)
        .unwrap();
    ExecutorRef {
        provider_id: ProviderId::parse(provider).unwrap(),
        installation_id: Some(installation.installation_id.to_string()),
    }
}
