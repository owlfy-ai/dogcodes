#![cfg(windows)]

#[tokio::test]
#[ignore = "uses the registered Kimi PATH and a child with stale launch environment; no model calls"]
async fn registered_path_is_used_even_when_the_running_host_inherited_an_old_path() {
    if std::env::var_os("DOGCODES_TEST_STALE_LAUNCH_PATH").is_some() {
        use agent_registry::{
            DiscoveryContext, InstallationStatus, ProviderManifest, discover_and_probe,
        };
        let mut manifest =
            ProviderManifest::parse_toml(include_str!("../../../drivers/manifests/kimi.toml"))
                .unwrap();
        // Force discovery through the newly registered PATH, not our known
        // home-directory fallback for Kimi's default installer.
        manifest.discovery.user_paths.clear();
        manifest.discovery.platform_paths.clear();
        let reports = discover_and_probe(&manifest, &DiscoveryContext::from_current_process())
            .await
            .unwrap();
        assert!(
            reports
                .iter()
                .any(|report| report.status == InstallationStatus::Installed)
        );
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "registered_path_is_used_even_when_the_running_host_inherited_an_old_path",
            "--ignored",
            "--nocapture",
        ])
        .env("DOGCODES_TEST_STALE_LAUNCH_PATH", "1")
        .env(
            "PATH",
            std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires the DeepSeek Harness desktop app; no model calls"]
async fn deepseek_harness_desktop_cli_is_discovered_and_identified() {
    use agent_registry::{
        DiscoveryContext, InstallationStatus, ProviderManifest, discover_and_probe,
    };
    let manifest = ProviderManifest::parse_toml(include_str!(
        "../../../drivers/manifests/deepseek-harness.toml"
    ))
    .unwrap();
    let reports = discover_and_probe(&manifest, &DiscoveryContext::from_current_process())
        .await
        .unwrap();
    for report in &reports {
        eprintln!("{report:?}");
    }
    assert!(reports.iter().any(|report| {
        report.status == InstallationStatus::Installed
            && format!("{report:?}").contains("DeepSeek Harness")
    }));
}

#[tokio::test]
#[ignore = "reads installed Windows Agent CLI versions; no model calls"]
async fn local_windows_agents_are_discovered_and_identified() {
    use agent_registry::{
        DiscoveryContext, InstallationStatus, ProviderManifest, discover_and_probe,
    };
    for source in [
        include_str!("../../../drivers/manifests/zcode.toml"),
        include_str!("../../../drivers/manifests/grok.toml"),
        include_str!("../../../drivers/manifests/cursor.toml"),
        include_str!("../../../drivers/manifests/kimi.toml"),
    ] {
        let manifest = ProviderManifest::parse_toml(source).unwrap();
        let reports = discover_and_probe(&manifest, &DiscoveryContext::from_current_process())
            .await
            .unwrap();
        assert!(
            reports
                .iter()
                .any(|report| report.status == InstallationStatus::Installed),
            "{} installation probe failed: {reports:?}",
            manifest.id
        );
        for report in reports {
            eprintln!(
                "{}: {:?}, version {:?}",
                manifest.id, report.status, report.version
            );
        }
    }
}
