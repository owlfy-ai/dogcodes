#![cfg(windows)]

#[tokio::test]
#[ignore = "reads installed Windows ZCode/Grok versions; no model calls"]
async fn local_zcode_and_grok_are_discovered_and_identified() {
    use agent_registry::{
        DiscoveryContext, InstallationStatus, ProviderManifest, discover_and_probe,
    };
    for source in [
        include_str!("../../../drivers/manifests/zcode.toml"),
        include_str!("../../../drivers/manifests/grok.toml"),
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
