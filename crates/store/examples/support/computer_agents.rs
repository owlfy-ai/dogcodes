//! Native launch configuration shared by discovery and live CU acceptance.
use agent_registry::{DiscoveryContext, ProviderManifest};
use driver_api::{AgentDriver, SessionResumeCapability};
use std::{path::PathBuf, sync::Arc};

type Error = Box<dyn std::error::Error>;
pub fn manifests() -> Result<Vec<ProviderManifest>, Error> {
    [
        "codex",
        "claude-code",
        "deepseek-harness",
        "cursor",
        "kimi",
        "grok",
        "codebuddy",
        "kiro",
        "opencode",
        "antigravity",
        "zcode",
    ]
    .into_iter()
    .map(|name| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../drivers/manifests")
            .join(format!("{name}.toml"));
        Ok(ProviderManifest::parse_toml(&std::fs::read_to_string(
            path,
        )?)?)
    })
    .collect()
}

pub async fn discover() -> Result<(), Error> {
    let context = DiscoveryContext::from_current_process();
    let mut reports = vec![];
    for manifest in manifests()? {
        let probes = agent_registry::discover_and_probe(&manifest, &context).await?;
        reports.push(serde_json::json!({"provider":manifest.id,"installations":probes.iter().map(|probe|
            serde_json::json!({"executable":probe.executable,"status":probe.status,"version":probe.version.as_ref().map(ToString::to_string)})
        ).collect::<Vec<_>>()}));
    }
    println!("{}", serde_json::to_string_pretty(&reports)?);
    Ok(())
}

pub async fn connect(
    provider: &str,
    identity: &str,
    executable: PathBuf,
) -> Result<Arc<dyn AgentDriver>, Error> {
    let manifest = manifests()?
        .into_iter()
        .find(|manifest| manifest.id.as_str() == provider)
        .ok_or("unsupported acceptance provider")?;
    let probe = agent_registry::probe_installation(&manifest, executable.clone()).await?;
    let version = probe.version.as_ref().map(ToString::to_string);
    Ok(match provider {
        "codex" => Arc::new(codex_driver::CodexDriver::new(
            &executable,
            identity.into(),
            version.ok_or("Codex version unavailable")?,
        )?),
        "claude-code" => Arc::new(claude_driver::ClaudeDriver::new(
            provider,
            identity,
            executable,
            version,
            SessionResumeCapability::Supported,
        )?),
        "antigravity" => Arc::new(antigravity_driver::AntigravityDriver::new(
            provider,
            identity,
            executable,
            version,
            SessionResumeCapability::Supported,
        )?),
        "zcode" => {
            let resources = agent_runtime::electron_resources_directory(&executable)
                .ok_or("ZCode resources unavailable")?;
            let resume = zcode_driver::verified_resume_capability(version.as_deref());
            Arc::new(
                zcode_driver::ZcodeDriver::connect_ready(
                    provider,
                    identity,
                    zcode_driver::ZcodeLaunch {
                        app_binary: executable,
                        cli_script: resources.join("glm/zcode.cjs"),
                    },
                    version,
                    resume,
                )
                .await?,
            )
        }
        _ => Arc::new(
            acp_driver::AcpDriver::connect_ready(
                provider,
                identity,
                acp_driver::AcpLaunch {
                    executable,
                    args: manifest.discovery.launch_args,
                },
                version,
            )
            .await?,
        ),
    })
}
