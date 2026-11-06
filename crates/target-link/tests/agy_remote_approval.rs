//! An Antigravity approval on a server, the whole way: a stand-in `agy` runs on the far end
//! of a real SSH connection, its hook goes through the shim's relay to this program, and
//! the answer given here comes back to it. Opt-in, as it needs an OpenSSH server:
//!
//! ```sh
//! cargo build -p target-shim -p target-link --examples --bins
//! DOGCODES_SSHD_TESTS=1 cargo test -p target-link --test agy_remote_approval
//! ```
#[cfg(unix)]
mod support;

#[cfg(unix)]
mod unix {
    use agent_runtime::TargetExecutable;
    use antigravity_driver::AntigravityDriver;
    use driver_api::{
        AgentDriver, DriverEvent, PromptRequest, SessionRequest, SessionResumeCapability,
    };
    use futures::StreamExt;
    use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};
    use target_link::{
        AskpassPrompt, ConnectObserver, ConnectRequest, ConnectStep, connect, find_ssh,
    };

    use super::support::{Sshd, built, catalog};

    /// Lists its hook like the real one. In a turn it asks that hook about a tool first, as the
    /// real one does, and reports what it was told.
    const FAKE_AGY: &str = r#"#!/bin/sh
PATH="$PATH:/usr/bin:/bin"
root=""; previous=""
for argument in "$@"; do [ "$previous" = --add-dir ] && root="$argument"; previous="$argument"; done
hooks="$root/.agents/hooks.json"
command=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["dogcodes-approvals"]["PreToolUse"][0]["hooks"][0]["command"])' "$hooks")
case "$*" in *--print=/hooks*)
python3 -c 'import json,sys; print(json.dumps({"event":"command_result","command":{"name":"hooks","data":{"hooks":[{"name":"dogcodes-approvals","enabled":True,"source":sys.argv[1],"actions":[{"event":"PreToolUse","matcher":".*","command":sys.argv[2]}]}]}}}))' "$hooks" "$command"
printf '%s\n' '{"event":"result","result":{"status":"SUCCESS"}}'
exit 0;; esac
env > "$0.environment"
printf '%s\n' "$root" > "$0.root"
while IFS= read -r line; do
  case "$line" in *'"event":"user"'*)
    printf '%s\n' '{"event":"init","conversation_id":"conv-remote"}'
    printf '%s' '{"conversationId":"conv-remote","stepIdx":1,"toolCall":{"name":"run_command","args":{"command":"echo hi"}}}' | sh -c "$command" > "$0.decision" 2> "$0.failure"
    printf '%s\n' '{"event":"result","result":{"conversation_id":"conv-remote","status":"SUCCESS"}}'
    ;; esac
done
"#;

    struct Quiet;

    #[async_trait::async_trait]
    impl ConnectObserver for Quiet {
        fn step(&self, _: ConnectStep) {}
        fn log(&self, _: &str) {}
        async fn prompt(&self, _: AskpassPrompt) -> Option<String> {
            None
        }
    }

    async fn wait_for(what: &str, ready: impl Fn() -> bool) {
        for _ in 0..200 {
            if ready() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{what} did not happen");
    }

    async fn scenario() {
        let server = Sshd::start("", "");
        let (_shims, shims) = catalog();
        let link = connect(
            ConnectRequest {
                connection: server.connection(),
                program: find_ssh(&[]).await.unwrap(),
                askpass_program: built("examples/askpass_helper"),
                shims,
                config_file: Some(server.config.clone()),
            },
            Arc::new(Quiet),
        )
        .await
        .unwrap();
        let place = tempfile::tempdir().unwrap();
        let agy = place.path().join("agy");
        std::fs::write(&agy, FAKE_AGY).unwrap();
        std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).unwrap();
        let directory = place.path().to_string_lossy().into_owned();

        let driver = AntigravityDriver::new(
            "antigravity",
            "ins_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            TargetExecutable::new(link.target(), &agy),
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        assert!(!driver.runs_on_this_machine());
        let handle = driver
            .create_session(SessionRequest {
                app_mcps: vec![],
                permission_mode: Some("desktop-approvals".into()),
                additional_directories: vec![],
                working_directory: directory.clone(),
            })
            .await
            .unwrap();
        let mut events = driver
            .prompt(
                &handle,
                PromptRequest {
                    attachments: vec![],
                    permission_mode: Some("desktop-approvals".into()),
                    additional_directories: vec![],
                    prompt: "hello".into(),
                    working_directory: directory,
                    context: vec![],
                },
            )
            .await
            .unwrap();
        let mut asked = false;
        tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(event) = events.next().await {
                match event.unwrap() {
                    DriverEvent::ApprovalRequested {
                        native_request_id, ..
                    } => {
                        asked = true;
                        driver
                            .respond_to_interaction(&handle, &native_request_id, "allow_once")
                            .await
                            .unwrap();
                    }
                    DriverEvent::TurnCompleted { .. } => return,
                    DriverEvent::TurnFailed { reason } => panic!("the turn failed: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("the turn finishes");
        let decision = std::fs::read_to_string(agy.with_extension("decision")).unwrap();
        let failure = std::fs::read_to_string(agy.with_extension("failure")).unwrap();
        assert!(
            asked,
            "the approval must reach this side: {decision} {failure}"
        );
        assert!(decision.contains(r#""decision":"allow""#), "{decision}");
        // The hook says how to reach the desktop only through the relay.
        let environment = std::fs::read_to_string(agy.with_extension("environment")).unwrap();
        assert!(environment.contains("AGY_CLI_NONINTERACTIVE_HEADLESS=1"));
        assert!(!environment.contains("DOGCODES_APPROVAL"), "{environment}");

        let root = std::fs::read_to_string(agy.with_extension("root")).unwrap();
        let root = Path::new(root.trim()).to_owned();
        assert!(root.join(".agents/hooks.json").exists());
        driver.shutdown(handle).await.unwrap();
        wait_for("removing the hook's directory on the server", || {
            !root.exists()
        })
        .await;
    }

    pub fn main() {
        if antigravity_driver::approval_bridge::run_hook_if_requested() {
            return;
        }
        if std::env::var_os("DOGCODES_SSHD_TESTS").is_none() {
            println!("skipped: set DOGCODES_SSHD_TESTS=1, as it needs an OpenSSH server");
            return;
        }
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(scenario());
        println!("test result: ok");
    }
}

#[cfg(unix)]
fn main() {
    unix::main();
}

#[cfg(not(unix))]
fn main() {}
