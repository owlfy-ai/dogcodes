//! Isolated native harness identity, not a replacement for desktop discovery.
use agent_core::Core;
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, InstallationStatus, RecordInstallationProbe,
};
use driver_api::AgentDriver;
use std::sync::Arc;
type Error = Box<dyn std::error::Error>;

pub async fn identity(core: &Arc<Core>, provider: &str) -> Result<String, Error> {
    probe(core, provider, InstallationStatus::Installed, None).await?;
    Ok(core
        .bootstrap()
        .await?
        .installations
        .into_iter()
        .find(|i| i.provider_id == provider && i.executable == format!("native-fixture:{provider}"))
        .ok_or("isolated harness installation missing")?
        .installation_id
        .to_string())
}

async fn probe(
    core: &Arc<Core>,
    provider: &str,
    status: InstallationStatus,
    version: Option<String>,
) -> Result<(), Error> {
    let mut request = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
        RecordInstallationProbe {
            provider_id: provider.into(),
            executable: format!("native-fixture:{provider}"),
            fingerprint: "isolated-native-harness".into(),
            status,
            version,
            version_text: None,
            diagnostic: Some("Isolated native acceptance harness; not production discovery".into()),
            elapsed_millis: 0,
        },
    ));
    request.actor = ActorRef::SystemRecovery;
    core.execute(request).await?;
    Ok(())
}

pub async fn register(core: &Arc<Core>, driver: Arc<dyn AgentDriver>) -> Result<(), Error> {
    let executor = driver.executor();
    let snapshot = core.bootstrap().await?;
    if !snapshot.installations.iter().any(|i| {
        i.provider_id == executor.provider_id.as_str()
            && executor.installation_id.as_deref() == Some(i.installation_id.as_str())
            && i.executable == format!("native-fixture:{}", executor.provider_id)
    }) {
        return Err("native harness driver does not match its installation".into());
    }
    probe(
        core,
        executor.provider_id.as_str(),
        InstallationStatus::Ready,
        driver.native_agent_version().map(str::to_owned),
    )
    .await?;
    core.register_driver(driver).await;
    Ok(())
}
