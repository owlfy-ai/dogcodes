use agent_runtime::{ProcessSpec, install_process_diagnostics, run_bounded, run_scoped_bounded};
use std::sync::Mutex;

static EVENTS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
fn record(event: &str, detail: &str) {
    EVENTS.lock().unwrap().push((event.into(), detail.into()));
}

#[test]
fn diagnostic_fixture() {
    if std::env::var("DOGCODES_DIAGNOSTIC_FIXTURE").as_deref() == Ok("sleep") {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    if std::env::var_os("DOGCODES_DIAGNOSTIC_FIXTURE").is_some() {
        println!("CLI_OUTPUT_MUST_NOT_BE_LOGGED");
    }
}

#[tokio::test]
async fn records_direct_and_owned_process_metadata_without_cli_payloads() {
    install_process_diagnostics(record);
    let mut spec = ProcessSpec::probe(
        std::env::current_exe().unwrap(),
        vec![
            "--exact".into(),
            "diagnostic_fixture".into(),
            "--nocapture".into(),
        ],
    );
    spec.timeout = std::time::Duration::from_secs(15);
    spec.env_overrides.insert(
        "DOGCODES_DIAGNOSTIC_FIXTURE".into(),
        "PRIVATE_ENV_VALUE".into(),
    );
    for scoped in [false, true] {
        let output = if scoped {
            run_scoped_bounded(&spec).await
        } else {
            run_bounded(&spec).await
        }
        .unwrap();
        assert!(output.success, "{}", output.stderr);
        assert!(output.stdout.contains("CLI_OUTPUT_MUST_NOT_BE_LOGGED"));
    }
    spec.env_overrides
        .insert("DOGCODES_DIAGNOSTIC_FIXTURE".into(), "sleep".into());
    spec.timeout = std::time::Duration::from_millis(100);
    assert!(matches!(
        run_bounded(&spec).await,
        Err(agent_runtime::ProcessError::TimedOut { .. })
    ));
    // A failed spawn must also be observable with a numeric OS error.
    spec.executable = std::env::temp_dir().join("dogcodes-missing-diagnostic-fixture.exe");
    assert!(run_scoped_bounded(&spec).await.is_err());
    let events = EVENTS.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(event, _)| event == "process_spawned")
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|(event, _)| event == "process_exited")
            .count(),
        3
    );
    assert!(
        events
            .iter()
            .any(|(event, detail)| event == "process_failed" && detail.contains("os_error=Some("))
    );
    assert!(events.iter().any(|(event, detail)| event == "process_spawned" && detail.contains("child_pid=Some(")));
    let text = format!("{events:?}");
    assert!(text.contains("strategy=direct"));
    assert!(text.contains("process_timeout"));
    assert!(!text.contains("process_observer_dropped"));
    if cfg!(windows) {
        assert!(text.contains("creation_flags=0x08000004"));
        assert!(text.contains("direct creation_flags=0x08000000 CREATE_NO_WINDOW=true"));
    }
    assert!(!text.contains("PRIVATE_ENV_VALUE"));
    assert!(!text.contains("CLI_OUTPUT_MUST_NOT_BE_LOGGED"));
    assert!(!text.contains("diagnostic_fixture\""));
}
