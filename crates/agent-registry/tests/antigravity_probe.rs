#![cfg(any(unix, windows))]

use agent_registry::{InstallationStatus, ProviderManifest, probe_installation};

const MANIFEST: &str = include_str!("../../../drivers/manifests/antigravity.toml");

#[tokio::test]
async fn antigravity_identity_and_version_probes_disable_interactive_auth() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let (name, source) = (
        "Antigravity probe.cmd",
        "@echo off\r\n\
         if not \"%AGY_CLI_NONINTERACTIVE_HEADLESS%\"==\"1\" (\r\n\
           >\"%~dp0interactive-auth.txt\" echo %1\r\n\
           exit /b 73\r\n\
         )\r\n\
         if \"%~1\"==\"models\" (\r\n\
           >>\"%~dp0probes.txt\" echo models\r\n\
           echo Error: Please sign in to view available models. 1>&2\r\n\
           exit /b 1\r\n\
         )\r\n\
         if \"%~1\"==\"--version\" (\r\n\
           >>\"%~dp0probes.txt\" echo version\r\n\
           echo 1.2.3\r\n\
           exit /b 0\r\n\
         )\r\n\
         exit /b 74\r\n",
    );
    #[cfg(unix)]
    let (name, source) = (
        "Antigravity probe",
        r#"#!/bin/sh
root=${0%/*}
if [ "$AGY_CLI_NONINTERACTIVE_HEADLESS" != 1 ]; then
    printf '%s\n' "$1" > "$root/interactive-auth.txt"
    exit 73
fi
case "$1" in
  models)
    printf '%s\n' models >> "$root/probes.txt"
    printf '%s\n' 'Error: Please sign in to view available models.' >&2
    exit 1
    ;;
  --version)
    printf '%s\n' version >> "$root/probes.txt"
    printf '%s\n' '1.2.3'
    ;;
  *) exit 74 ;;
esac
"#,
    );
    let executable = directory.path().join(name);
    std::fs::write(&executable, source).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let manifest = ProviderManifest::parse_toml(MANIFEST).unwrap();
    let report = probe_installation(&manifest, executable.canonicalize().unwrap())
        .await
        .unwrap();

    assert!(
        !directory.path().join("interactive-auth.txt").exists(),
        "installation discovery must not enter interactive authentication: {report:?}"
    );
    // A signed-out CLI is still installed. Authentication belongs to readiness,
    // and neither the identity nor the version command should start login UI.
    assert_eq!(report.status, InstallationStatus::Installed, "{report:?}");
    assert_eq!(report.version.unwrap().to_string(), "1.2.3");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("probes.txt"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["models", "version"]
    );
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "reads the installed Antigravity CLI; no model prompts or credential writes"]
async fn installed_windows_antigravity_probe_is_noninteractive() {
    use std::os::windows::process::CommandExt;
    const HOST: &str = "DOGCODES_TEST_AGY_CONSOLELESS_PROBE";
    if std::env::var_os(HOST).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "installed_windows_antigravity_probe_is_noninteractive",
                "--ignored",
                "--nocapture",
            ])
            .env(HOST, "1")
            .creation_flags(0x0000_0008) // DETACHED_PROCESS, like the installed GUI host.
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let executable =
        std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("agy/bin/agy.exe");
    let manifest = ProviderManifest::parse_toml(MANIFEST).unwrap();
    let report = probe_installation(&manifest, executable).await.unwrap();
    eprintln!(
        "Antigravity discovery: {:?}, version {:?}, diagnostic {:?}",
        report.status, report.version, report.diagnostic
    );
    assert_eq!(report.status, InstallationStatus::Installed, "{report:?}");
    assert!(report.version.is_some(), "{report:?}");
}
