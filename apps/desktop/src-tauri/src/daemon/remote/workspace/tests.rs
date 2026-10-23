use super::*;
use crate::daemon::remote::{remote_call, response};
use agent_domain::{ExecutorRef, ProjectId, ProviderId, TaskId};
use agent_protocol::{ActorRef, CommandEnvelope, CoreCommand, CreateTask, RegisterProject};
use base64::Engine as _;
use std::{path::Path, time::Duration};

struct Fixture {
    core: Arc<Core>,
    control: Arc<ServiceControl>,
    temp: tempfile::TempDir,
    project: ProjectId,
    task: TaskId,
}

#[tokio::test]
async fn the_fence_protocols_are_gone_and_plain_stop_remains() {
    let fixture = Fixture::new().await;
    let caps = fixture
        .call("remote_capabilities", Value::Null)
        .await
        .unwrap();
    for gone in [
        "terminalStopRecovery",
        "cancelCommandRecovery",
        "powerOperationRecovery",
    ] {
        assert!(caps.get(gone).is_none(), "{gone}");
    }
    for method in [
        "terminal_stop_operation",
        "session_command_receipt",
        "retire_session_command",
        "power_preferences_operation",
    ] {
        assert!(fixture.call(method, json!([])).await.is_err(), "{method}");
    }
    // Stopping a terminal the PC does not know is an answer, not an unknown outcome.
    assert!(
        fixture
            .call("terminal_stop", json!(uuid::Uuid::new_v4().to_string()))
            .await
            .is_err()
    );
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("project")).unwrap();
        let core = Arc::new(Core::new(Arc::new(
            agent_store::SqliteStore::open_in_memory().unwrap(),
        )));
        let project = ProjectId::new();
        let task = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project.clone(),
                display_name: "Remote fixture".into(),
                path: temp
                    .path()
                    .join("project")
                    .canonicalize()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id: project.clone(),
                title: "Workspace".into(),
                preferred_executor: ExecutorRef {
                    provider_id: ProviderId::parse("fake").unwrap(),
                    installation_id: None,
                },
            },
        )))
        .await
        .unwrap();
        let control = Arc::new(ServiceControl::new(&temp.path().join("service")));
        Self {
            core,
            control,
            temp,
            project,
            task,
        }
    }
    fn root(&self) -> std::path::PathBuf {
        self.temp.path().join("project").canonicalize().unwrap()
    }
    async fn call(&self, method: &str, args: Value) -> Result<Value, String> {
        remote_call(
            &self.core,
            &self.control,
            method,
            args,
            ActorRef::PairedDevice(agent_domain::DeviceId::new()),
        )
        .await
    }
    async fn review(&self, operation: &str, arguments: Value) -> Result<Value, String> {
        self.call(
            "review",
            json!({"operation":operation,"arguments":arguments}),
        )
        .await
    }
    fn directory(&self, path: &str) -> Value {
        json!({"taskId":self.task,"root":0,"path":path})
    }
    fn terminal(&self) -> Value {
        json!({"operationId":uuid::Uuid::new_v4().to_string(),"projectId":self.project,
            "directoryIndex":0,"taskId":self.task,"commandId":null,"label":"Remote terminal",
            "cwd":self.root(),"command":null,"cols":80,"rows":24})
    }
    fn seed_run(&self) {
        let directory = self.temp.path().join("service/project-runs");
        std::fs::create_dir_all(&directory).unwrap();
        let catalog = json!({"projectId":self.project,"platform":std::env::consts::OS,"shell":"sh",
            "generation":"fixture-generation","executor":null,"directories":[{
                "path":self.root(),"status":"ready","error":null,"commands":[{
                    "id":"hello","label":"Hello","command":"echo remote-run","cwd":".","source":"fixture"}]}]});
        std::fs::write(
            directory.join(format!("{}-{}.json", self.project, std::env::consts::OS)),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn files_are_available_and_confined_to_registered_task_roots() {
    let f = Fixture::new().await;
    std::fs::write(f.root().join("hello.txt"), "remote files").unwrap();
    assert_eq!(
        f.call("workspace_roots", json!(f.task)).await.unwrap()[0],
        json!(f.root())
    );
    let listing = f
        .call("workspace_directory", f.directory(""))
        .await
        .unwrap();
    assert_eq!(listing[0]["name"], "hello.txt");
    assert_eq!(
        f.call("workspace_preview", f.directory("hello.txt"))
            .await
            .unwrap()["text"],
        "remote files"
    );
    assert_eq!(
        f.call("workspace_find_files", json!([f.task, "hello", false]))
            .await
            .unwrap()["files"][0]["path"],
        "hello.txt"
    );
    for path in ["../outside.txt", "/etc/passwd"] {
        assert!(
            f.call("workspace_preview", f.directory(path))
                .await
                .is_err()
        );
    }
    #[cfg(unix)]
    {
        std::fs::write(f.temp.path().join("outside.txt"), "private").unwrap();
        std::os::unix::fs::symlink(f.temp.path().join("outside.txt"), f.root().join("escape"))
            .unwrap();
        assert!(
            f.call("workspace_preview", f.directory("escape"))
                .await
                .is_err()
        );
    }
    assert!(
        f.call(
            "workspace_directory",
            json!({"taskId":TaskId::new(),"root":0,"path":""})
        )
        .await
        .is_err()
    );
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn diff_commit_and_receipt_reuse_the_shared_git_workflow() {
    let f = Fixture::new().await;
    git(&f.root(), &["init"]);
    for (key, value) in [
        ("user.name", "Gateway Fixture"),
        ("user.email", "gateway@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/nonexistent-gateway-fixture-hooks"),
    ] {
        git(&f.root(), &["config", key, value]);
    }
    std::fs::write(f.root().join("hello.txt"), "before\n").unwrap();
    git(&f.root(), &["add", "hello.txt"]);
    git(&f.root(), &["commit", "-m", "Initial fixture"]);
    std::fs::write(f.root().join("hello.txt"), "after\n").unwrap();
    let snapshot = f
        .review("query", json!({"taskId":f.task,"mode":"uncommitted"}))
        .await
        .unwrap();
    assert_eq!(snapshot["roots"][0]["files"][0]["path"], "hello.txt");
    let root = snapshot["roots"][0]["root"]["id"].clone();
    let file = f
        .review(
            "file",
            json!({"taskId":f.task,"snapshotId":snapshot["id"],"rootId":root,"path":"hello.txt"}),
        )
        .await
        .unwrap();
    assert!(!file.is_null());
    let id = uuid::Uuid::new_v4().to_string();
    let request = json!({"operationId":id,"taskId":f.task,"rootId":root,"action":"commit",
        "message":"Update remote fixture","stageAll":true,"snapshotId":snapshot["id"]});
    let first = f.review("git_action", request.clone()).await.unwrap();
    let head = git(&f.root(), &["rev-parse", "HEAD"]);
    assert_eq!(
        first,
        f.review("git_action", request.clone()).await.unwrap()
    );
    assert_eq!(head, git(&f.root(), &["rev-parse", "HEAD"]));
    let receipt = f
        .review("git_operation", json!({"taskId":f.task,"operationId":id}))
        .await
        .unwrap();
    assert_eq!(receipt["outcome"]["status"], "succeeded");
    let mut conflict = request;
    conflict["message"] = json!("Different intent");
    assert!(f.review("git_action", conflict).await.is_err());
}

#[tokio::test]
async fn only_workspace_allowlisted_methods_and_paired_actors_are_admitted() {
    let f = Fixture::new().await;
    for method in [
        "project_run_open",
        "scan_agent",
        "remote_control",
        "retire_if_idle",
        "workspace_lease",
        "attachment_save",
    ] {
        assert!(f.call(method, Value::Null).await.is_err(), "{method}");
    }
    for operation in ["reveal_path", "workspace_path"] {
        let args = if operation == "workspace_path" {
            json!({"taskId":f.task,"path":"hello.txt"})
        } else {
            json!({"taskId":f.task,"snapshotId":"snapshot","rootId":"root","path":"hello.txt"})
        };
        assert_eq!(
            f.review(operation, args).await.unwrap_err(),
            "remote review operation unavailable"
        );
    }
    assert!(
        remote_call(
            &f.core,
            &f.control,
            "workspace_roots",
            json!(f.task),
            ActorRef::LocalUser
        )
        .await
        .is_err()
    );
    let caps = f.call("remote_capabilities", Value::Null).await.unwrap();
    assert!(
        caps["workspaceMethods"]
            .as_array()
            .unwrap()
            .contains(&json!("project_run_start"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_reattaches_by_identity_and_supports_input_offsets_resize_and_stop() {
    let f = Fixture::new().await;
    let request = f.terminal();
    let started = f.call("terminal_start", request.clone()).await.unwrap();
    let id = started["id"].as_str().unwrap();
    assert_eq!(f.call("terminal_start", request).await.unwrap()["id"], id);
    assert_eq!(
        f.call("terminal_list", json!(f.project))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.call("terminal_get", json!(id)).await.unwrap()["id"], id);
    f.call("terminal_resize", json!([id, 100, 32]))
        .await
        .unwrap();
    assert_eq!(
        f.call("terminal_get", json!(id)).await.unwrap()["cols"],
        100
    );
    assert!(
        f.call("terminal_write", json!([id, b"not-sent".to_vec()]))
            .await
            .is_err()
    );
    let input_actor = ActorRef::PairedDevice(agent_domain::DeviceId::new());
    let stream = uuid::Uuid::new_v4().to_string();
    let opened=remote_call(&f.core,&f.control,"terminal_input",json!({"type":"open","terminalId":id,"expectedEpoch":started["epoch"],"expectedRevision":"0","streamId":stream}),input_actor.clone()).await.unwrap();
    remote_call(&f.core,&f.control,"terminal_input",json!({"type":"write","terminalId":id,"expectedEpoch":started["epoch"],"revision":opened["revision"],"streamId":stream,"sequence":"1","data":base64::engine::general_purpose::STANDARD.encode(b"echo gateway-pty-ok\n")}),input_actor).await.unwrap();
    let mut cursor = "0".to_string();
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let batch = f.call("terminal_read", json!([id, cursor])).await.unwrap();
            cursor = batch["next"].as_str().unwrap().into();
            let bytes: Vec<u8> = serde_json::from_value(batch["bytes"].clone()).unwrap();
            output.push_str(&String::from_utf8_lossy(&bytes));
            if output.contains("gateway-pty-ok") {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        f.call("terminal_read", json!([id, "18446744073709551616"]))
            .await
            .is_err()
    );
    f.call("terminal_stop", json!(id)).await.unwrap();
    let mut outside = f.terminal();
    outside["cwd"] = json!(f.temp.path());
    assert!(f.call("terminal_start", outside).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn project_run_uses_frozen_recipe_and_same_terminal_on_retry() {
    let f = Fixture::new().await;
    f.seed_run();
    let catalog = f.call("project_runs", json!(f.task)).await.unwrap();
    assert_eq!(catalog["generation"], "fixture-generation");
    let edit_id = uuid::Uuid::new_v4().to_string();
    let edit = json!({"operationId":edit_id,"taskId":f.task,"generation":catalog["generation"],
        "directoryIndex":0,"commandId":"hello","edit":{"action":"rename","label":"New label"}});
    let edited = f.call("project_run_edit", edit.clone()).await.unwrap();
    assert!(edited.get("Ok").is_some(), "{edited}");
    assert_eq!(edited, f.call("project_run_edit", edit).await.unwrap());
    assert_eq!(
        f.call("project_run_operation", json!([f.task, edit_id]))
            .await
            .unwrap()["status"],
        "succeeded"
    );
    let request = json!({"taskId":f.task,"generation":edited["Ok"]["generation"],
        "directoryIndex":0,"commandId":"hello","cols":80,"rows":24,"operationId":uuid::Uuid::new_v4().to_string()});
    let launched = f.call("project_run_start", request.clone()).await.unwrap();
    assert_eq!(
        launched["id"],
        f.call("project_run_start", request).await.unwrap()["id"]
    );
    let stale = json!({"taskId":f.task,"generation":"stale","directoryIndex":0,"commandId":"hello",
        "cols":80,"rows":24,"operationId":uuid::Uuid::new_v4().to_string()});
    assert!(f.call("project_run_start", stale).await.is_err());
    f.call("terminal_stop", launched["id"].clone())
        .await
        .unwrap();
}

#[tokio::test]
async fn gateway_streams_are_tagged_and_unsubscribe_releases_watchers() {
    let f = Fixture::new().await;
    let (tx, mut rx) = mpsc::channel(8);
    let core = f.core.clone();
    let control = f.control.clone();
    let args = f.directory("");
    let mut registry = Subscriptions::default();
    registry.prepare("files-1", "watch:files-1").await.unwrap();
    registry.insert(
        "files-1".into(),
        "watch:files-1".into(),
        tokio::spawn(async move {
            subscribe(&core, &control, "workspace_watch", args, &tx, "files-1")
                .await
                .unwrap();
        }),
    );
    let ack = tokio::time::timeout(Duration::from_secs(12), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ack["result"]["subscriptionId"], "files-1");
    assert_eq!(f.control.workspace_watchers.active_count(), 1);
    std::fs::write(f.root().join("new.txt"), "changed").unwrap();
    let event = tokio::time::timeout(Duration::from_secs(12), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event["type"], "notification");
    assert_eq!(event["subscriptionId"], "files-1");
    assert!(registry.remove("files-1").await);
    assert_eq!(f.control.workspace_watchers.active_count(), 0);
    assert!(!registry.remove("files-1").await);

    for method in ["review_watch", "project_runs_watch"] {
        let (tx, mut rx) = mpsc::channel(8);
        let core = f.core.clone();
        let control = f.control.clone();
        let args = json!(f.task);
        let task =
            tokio::spawn(
                async move { subscribe(&core, &control, method, args, &tx, method).await },
            );
        assert_eq!(rx.recv().await.unwrap()["result"]["subscribed"], true);
        f.control.review.changes.changed();
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["event"]["kind"], "invalidated");
        assert_eq!(event["method"], method);
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn subscription_limit_replacement_and_connection_drop_are_bounded() {
    struct OnDrop(Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let mut registry = Subscriptions::default();
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for index in 0..super::super::MAX_SUBSCRIPTIONS {
        let id = index.to_string();
        registry.prepare(&id, &id).await.unwrap();
        let guard = OnDrop(dropped.clone());
        registry.insert(
            id.clone(),
            id,
            tokio::spawn(async move {
                let _guard = guard;
                std::future::pending::<()>().await;
            }),
        );
    }
    assert!(registry.prepare("overflow", "overflow").await.is_err());
    registry.prepare("replacement", "0").await.unwrap();
    assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(registry);
    tokio::time::timeout(Duration::from_secs(5), async {
        while dropped.load(std::sync::atomic::Ordering::SeqCst) != super::super::MAX_SUBSCRIPTIONS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn oversized_result_is_a_request_error_and_does_not_close_the_connection() {
    let reply = response(
        "large",
        Ok(json!({"image":"x".repeat(super::super::MAX_FRAME_BYTES)})),
    );
    assert_eq!(reply["error"]["code"], "response_too_large");
    assert_eq!(response("next", Ok(json!("ok")))["result"], "ok");
}

/// Uses the production encrypted socket loop with an isolated local relay peer.
/// No external server, keychain, model account or user database is involved.
#[tokio::test]
async fn encrypted_socket_routes_workspace_calls_unsubscribes_and_checks_revocation() {
    use crate::daemon::remote::{
        Config, Ready, Secrets, Session, now, receive, send_json, take_json,
    };
    use tokio_tungstenite::tungstenite::Message;

    async fn read_json(
        socket: &mut crate::daemon::remote::Socket,
        session: &mut Session,
        buffer: &mut Vec<u8>,
    ) -> Value {
        loop {
            if let Some(value) = take_json(buffer).unwrap() {
                return value;
            }
            buffer.extend(
                session
                    .open(&receive(socket, Duration::from_secs(15)).await.unwrap())
                    .unwrap(),
            );
        }
    }
    async fn read_response(
        socket: &mut crate::daemon::remote::Socket,
        session: &mut Session,
        buffer: &mut Vec<u8>,
    ) -> Value {
        loop {
            let value = read_json(socket, session, buffer).await;
            if value["type"] == "response" {
                return value;
            }
            assert_eq!(value["type"], "notification");
        }
    }

    let f = Fixture::new().await;
    std::fs::write(f.root().join("hello.txt"), "through encrypted socket").unwrap();
    let host = dogcodes_endpoint::keypair().unwrap();
    let phone = dogcodes_endpoint::keypair().unwrap();
    let host_key: [u8; 32] = host.private.as_slice().try_into().unwrap();
    let phone_key: [u8; 32] = phone.private.as_slice().try_into().unwrap();
    let host_public: [u8; 32] = host.public.as_slice().try_into().unwrap();
    let mut book =
        dogcodes_endpoint::pairing::PairingBook::load(f.temp.path().join("bindings.json")).unwrap();
    let psk = [7; 32];
    let pairing_context = b"isolated-workspace-pairing";
    book.issue("fixture", psk, pairing_context.to_vec(), now() + 60, now())
        .unwrap();
    let mut attempt = book.begin("fixture", &host_key, now()).unwrap();
    let mut initial = Session::new(
        true,
        &phone_key,
        Some(host_public),
        Some(&psk),
        pairing_context,
    )
    .unwrap();
    attempt
        .session()
        .read_handshake(&initial.write_handshake().unwrap())
        .unwrap();
    initial
        .read_handshake(&attempt.session().write_handshake().unwrap())
        .unwrap();
    attempt
        .session()
        .read_handshake(&initial.write_handshake().unwrap())
        .unwrap();
    let grant = book.complete(&attempt, now()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.control.remote.state.lock().await.ready = Some(Ready {
        config: Config {
            service: format!("http://{}", listener.local_addr().unwrap()),
            host_id: "fixture-host".into(),
            public_key: hex::encode(&host.public),
        },
        secrets: Secrets {
            token: "fixture-token".into(),
            private_key: hex::encode(host_key),
        },
        book,
    });
    let (core, control, remote, device) = (
        f.core.clone(),
        f.control.clone(),
        f.control.remote.clone(),
        grant.device_id.clone(),
    );
    let host_task = tokio::spawn(async move {
        remote
            .connection(&core, &control, "workspace-test", "ticket", Some(&device))
            .await
    });
    let (socket, _) = listener.accept().await.unwrap();
    let socket = tokio_tungstenite::accept_async(tokio_tungstenite::MaybeTlsStream::Plain(socket))
        .await
        .unwrap();
    let mut socket = crate::daemon::remote::Socket::Relay(Box::new(socket));
    let context = serde_json::to_vec(&json!([
        "dogcodes-reconnect",
        1,
        "fixture-host",
        "workspace-test"
    ]))
    .unwrap();
    let mut session = Session::new(true, &phone_key, Some(host_public), None, &context).unwrap();
    socket
        .send(Message::Binary(session.write_handshake().unwrap().into()))
        .await
        .unwrap();
    session
        .read_handshake(&receive(&mut socket, Duration::from_secs(5)).await.unwrap())
        .unwrap();
    socket
        .send(Message::Binary(session.write_handshake().unwrap().into()))
        .await
        .unwrap();
    let mut buffer = Vec::new();
    assert_eq!(
        read_json(&mut socket, &mut session, &mut buffer).await["type"],
        "connected"
    );
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(agent_protocol::CancelTurn {
        task_id: f.task.clone(),
        turn_id: agent_domain::TurnId::new(),
        run_id: agent_domain::RunId::new(),
    }));
    cancel.expected_stream_version = Some(1);
    // Owned fixture PTY only; these input bytes contain no newline/command.
    let input_terminal = f
        .control
        .terminals
        .start(serde_json::from_value(f.terminal()).unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.control.terminals.get(&input_terminal.id).unwrap().status
            != crate::terminal_host::TerminalStatus::Running
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let input_stream = uuid::Uuid::new_v4().to_string();
    let input_open = json!({"type":"open","terminalId":input_terminal.id,"expectedEpoch":input_terminal.epoch,"expectedRevision":"0","streamId":input_stream});
    send_json(&mut socket,&mut session,&json!({"version":1,"hostId":"fixture-host","requestId":"input-open","method":"terminal_input","params":input_open})).await.unwrap();
    let opened = read_response(&mut socket, &mut session, &mut buffer).await;
    assert_eq!(opened["result"]["state"], "ready", "{opened}");
    let old_input = json!({"type":"write","terminalId":input_terminal.id,"expectedEpoch":input_terminal.epoch,"revision":"1","streamId":input_stream,"sequence":"1","data":base64::engine::general_purpose::STANDARD.encode(b"input-proof")});
    send_json(&mut socket,&mut session,&json!({"version":1,"hostId":"fixture-host","requestId":"input-write","method":"terminal_input","params":old_input})).await.unwrap();
    assert_eq!(
        read_response(&mut socket, &mut session, &mut buffer).await["result"]["state"],
        "applied"
    );
    for (id, method, params) in [
        ("read-file", "workspace_preview", f.directory("hello.txt")),
        ("watch", "workspace_watch", f.directory("")),
        ("cancel-watch", "unsubscribe", json!("watch")),
        ("runs-watch", "project_runs_watch", json!(f.task)),
        ("caps", "remote_capabilities", Value::Null),
        ("cancel-receipt", "command_receipt", json!([cancel])),
        (
            "power-set",
            "power_preferences_set",
            json!({"enabled": false}),
        ),
    ] {
        send_json(&mut socket,&mut session,&json!({"version":1,"hostId":"fixture-host","requestId":id,"method":method,"params":params})).await.unwrap();
        let reply = read_response(&mut socket, &mut session, &mut buffer).await;
        assert_eq!(reply["requestId"], id, "{reply}");
        assert!(reply.get("error").is_none(), "{reply}");
        match id {
            "read-file" => assert_eq!(reply["result"]["text"], "through encrypted socket"),
            "watch" => assert_eq!(reply["result"]["subscriptionId"], "watch"),
            "cancel-watch" => {
                assert_eq!(reply["result"]["unsubscribed"], true);
                assert_eq!(f.control.workspace_watchers.active_count(), 0);
            }
            "caps" => assert!(reply["result"]["powerMethods"].is_array()),
            "cancel-receipt" => assert!(reply["result"].is_null()),
            "power-set" => assert_eq!(reply["result"]["enabled"], false),
            _ => {}
        }
    }

    // Upgrade from the authenticated relay and use fresh pinned Noise state.
    f.control
        .remote
        .state
        .lock()
        .await
        .ready
        .as_mut()
        .unwrap()
        .config
        .service = "http://127.0.0.1:9".into();
    let offer = dogcodes_remote_transport::offer(vec![]).await.unwrap();
    send_json(&mut socket,&mut session,&json!({"version":1,"hostId":"fixture-host","requestId":"direct","method":"direct_open","params":{"offer":offer.description,"purpose":"business"}})).await.unwrap();
    let answer = read_response(&mut socket, &mut session, &mut buffer).await;
    assert!(answer.get("error").is_none(), "{answer}");
    let description = serde_json::from_value(answer["result"]["answer"].clone()).unwrap();
    let mut direct =
        crate::daemon::remote::Socket::Direct(offer.complete(description).await.unwrap());
    let context = serde_json::to_vec(&answer["result"]["noiseContext"]).unwrap();
    let mut direct_noise =
        Session::new(true, &phone_key, Some(host_public), None, &context).unwrap();
    direct
        .send(Message::Binary(
            direct_noise.write_handshake().unwrap().into(),
        ))
        .await
        .unwrap();
    direct_noise
        .read_handshake(&receive(&mut direct, Duration::from_secs(5)).await.unwrap())
        .unwrap();
    direct
        .send(Message::Binary(
            direct_noise.write_handshake().unwrap().into(),
        ))
        .await
        .unwrap();
    let mut direct_buffer = Vec::new();
    assert_eq!(
        read_json(&mut direct, &mut direct_noise, &mut direct_buffer).await["type"],
        "connected"
    );
    send_json(&mut direct,&mut direct_noise,&json!({"version":1,"hostId":"fixture-host","requestId":"direct-file","method":"workspace_preview","params":f.directory("hello.txt")})).await.unwrap();
    assert_eq!(
        read_response(&mut direct, &mut direct_noise, &mut direct_buffer).await["result"]["text"],
        "through encrypted socket"
    );

    let input_inspect = json!({"type":"inspect","terminalId":input_terminal.id,"expectedEpoch":input_terminal.epoch});
    send_json(&mut direct,&mut direct_noise,&json!({"version":1,"hostId":"fixture-host","requestId":"input-direct-inspect","method":"terminal_input","params":input_inspect})).await.unwrap();
    let read = read_response(&mut direct, &mut direct_noise, &mut direct_buffer).await;
    assert_eq!(read["result"]["owned"], true);
    assert_eq!(read["result"]["sequence"], "1");
    let fresh = uuid::Uuid::new_v4().to_string();
    send_json(&mut direct,&mut direct_noise,&json!({"version":1,"hostId":"fixture-host","requestId":"input-direct-open","method":"terminal_input","params":{"type":"open","terminalId":input_terminal.id,"expectedEpoch":input_terminal.epoch,"expectedRevision":"1","streamId":fresh}})).await.unwrap();
    assert_eq!(
        read_response(&mut direct, &mut direct_noise, &mut direct_buffer).await["result"]["state"],
        "ready"
    );
    let mut late = old_input.clone();
    late["sequence"] = json!("2");
    send_json(&mut socket,&mut session,&json!({"version":1,"hostId":"fixture-host","requestId":"input-stale-wss","method":"terminal_input","params":late})).await.unwrap();
    assert!(read_response(&mut socket, &mut session, &mut buffer).await["error"].is_object());
    let fresh_input = json!({"type":"write","terminalId":input_terminal.id,"expectedEpoch":input_terminal.epoch,"revision":"2","streamId":fresh,"sequence":"1","data":base64::engine::general_purpose::STANDARD.encode(b"fresh-proof")});
    send_json(&mut direct,&mut direct_noise,&json!({"version":1,"hostId":"fixture-host","requestId":"input-direct-write","method":"terminal_input","params":fresh_input})).await.unwrap();
    assert_eq!(
        read_response(&mut direct, &mut direct_noise, &mut direct_buffer).await["result"]["state"],
        "applied"
    );
    f.control.terminals.stop(&input_terminal.id).await.unwrap();
    send_json(&mut direct,&mut direct_noise,&json!({"version":1,"hostId":"fixture-host","requestId":"direct-power","method":"power_preferences_set","params":{"enabled":true}})).await.unwrap();
    let reply = read_response(&mut direct, &mut direct_noise, &mut direct_buffer).await;
    assert_eq!(reply["result"]["enabled"], true, "{reply}");
    // A failed upgrade does not consume or close the working relay session.
    send_json(&mut socket,&mut session,&json!({"version":1,"hostId":"fixture-host","requestId":"bad-direct","method":"direct_open","params":{"offer":{"type":"offer","sdp":"invalid"},"purpose":"business"}})).await.unwrap();
    assert!(
        read_response(&mut socket, &mut session, &mut buffer)
            .await
            .get("error")
            .is_some()
    );
    f.control.review.changes.changed();
    let event = read_json(&mut socket, &mut session, &mut buffer).await;
    assert_eq!(event["type"], "notification");
    assert_eq!(event["subscriptionId"], "runs-watch");

    // A bounded result error must leave subsequent commands usable.
    let mut image = vec![0_u8; 7 * 1024 * 1024];
    image[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    std::fs::write(f.root().join("large.png"), image).unwrap();
    send_json(&mut socket,&mut session,&json!({"version":1,"hostId":"fixture-host","requestId":"large","method":"workspace_preview","params":f.directory("large.png")})).await.unwrap();
    assert_eq!(
        read_response(&mut socket, &mut session, &mut buffer).await["error"]["code"],
        "response_too_large"
    );
    send_json(&mut socket,&mut session,&json!({"version":1,"hostId":"fixture-host","requestId":"still-connected","method":"workspace_roots","params":f.task})).await.unwrap();
    assert_eq!(
        read_response(&mut socket, &mut session, &mut buffer).await["result"][0],
        json!(f.root())
    );

    f.control
        .remote
        .state
        .lock()
        .await
        .ready
        .as_mut()
        .unwrap()
        .book
        .revoke(&grant.device_id)
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), host_task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap_err().contains("device revoked"));
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            receive(&mut direct, Duration::from_secs(5))
        )
        .await
        .unwrap()
        .is_err()
    );
}

#[tokio::test]
async fn project_attachment_freezes_content_and_has_a_durable_receipt() {
    let f = Fixture::new().await;
    std::fs::write(f.root().join("note.txt"), "original").unwrap();
    let upload = uuid::Uuid::new_v4().to_string();
    let mut request = json!({"source":f.directory("note.txt"),"uploadId":upload});
    let info = f
        .call("workspace_attachment_info", request.clone())
        .await
        .unwrap();
    request["request"] = info;
    std::fs::write(f.root().join("note.txt"), "changed").unwrap();
    assert!(
        f.call("workspace_attachment_import", request.clone())
            .await
            .is_err()
    );
    std::fs::write(f.root().join("note.txt"), "original").unwrap();
    let saved = f
        .call("workspace_attachment_import", request.clone())
        .await
        .unwrap();
    std::fs::remove_file(f.root().join("note.txt")).unwrap();
    assert_eq!(
        f.call("workspace_attachment_import", request)
            .await
            .unwrap(),
        saved
    );
}

#[tokio::test]
async fn command_execution_and_receipts_keep_the_same_authenticated_actor() {
    let f = Fixture::new().await;
    let actor = ActorRef::PairedDevice(agent_domain::DeviceId::new());
    for payload in [
        CoreCommand::RenameTask(agent_protocol::RenameTask {
            task_id: f.task.clone(),
            title: "Phone rename".into(),
        }),
        CoreCommand::DeleteTask(agent_protocol::TaskLifecycle {
            task_id: f.task.clone(),
        }),
    ] {
        let mut envelope = CommandEnvelope::local(payload);
        envelope.expected_stream_version = Some(
            f.core
                .bootstrap()
                .await
                .unwrap()
                .tasks
                .iter()
                .find(|t| t.task_id == f.task)
                .unwrap()
                .stream_version,
        );
        let args = json!([envelope]);
        let accepted = remote_call(&f.core, &f.control, "execute", args.clone(), actor.clone())
            .await
            .unwrap();
        let receipt = remote_call(
            &f.core,
            &f.control,
            "command_receipt",
            args.clone(),
            actor.clone(),
        )
        .await
        .unwrap();
        assert_eq!(accepted, receipt);
        assert_eq!(
            remote_call(&f.core, &f.control, "execute", args, actor.clone())
                .await
                .unwrap(),
            accepted
        );
    }
}

#[tokio::test]
async fn preview_uses_fixed_loopback_target_and_tunnels_websocket_without_replay() {
    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = Fixture::new().await;
    let previews = super::super::preview::Host::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let size = socket.read(&mut request).await.unwrap();
        assert!(String::from_utf8_lossy(&request[..size]).starts_with("GET /index?a=1 HTTP/1.1"));
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nContent-Type: text/html\r\nConnection: close\r\n\r\npreview").await.unwrap();
    });
    let lease = previews
        .call(
            &f.core,
            "preview_open",
            json!({"taskId":f.task,"port":port}),
        )
        .await
        .unwrap();
    let id = lease["previewId"].as_str().unwrap();
    assert!(
        previews
            .call(
                &f.core,
                "preview_http",
                json!({"previewId":id,"path":"//external.test"})
            )
            .await
            .is_err()
    );
    let response = previews
        .call(
            &f.core,
            "preview_http",
            json!({"previewId":id,"path":"/index?a=1"}),
        )
        .await
        .unwrap();
    assert_eq!(response["status"], 200);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(response["body"].as_str().unwrap())
            .unwrap(),
        b"preview"
    );
    server.await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        use futures_util::{SinkExt, StreamExt};
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_text().unwrap(),
            "hmr"
        );
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                "update".into(),
            ))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(200), socket.next())
                .await
                .is_err()
        );
    });
    let lease = previews
        .call(
            &f.core,
            "preview_open",
            json!({"taskId":f.task,"port":port}),
        )
        .await
        .unwrap();
    let id = lease["previewId"].as_str().unwrap();
    let opened = previews
        .call(
            &f.core,
            "preview_ws_open",
            json!({"previewId":id,"path":"/hmr"}),
        )
        .await
        .unwrap();
    let web = opened["socketId"].as_str().unwrap();
    let write = json!({"socketId":web,"sequence":"1","text":"hmr"});
    previews
        .call(&f.core, "preview_ws_write", write.clone())
        .await
        .unwrap();
    assert!(
        previews
            .call(&f.core, "preview_ws_write", write)
            .await
            .is_err()
    );
    assert_eq!(
        previews
            .call(&f.core, "preview_ws_read", json!(web))
            .await
            .unwrap()["text"],
        "update"
    );
    server.await.unwrap();
    previews
        .call(&f.core, "preview_close", json!(id))
        .await
        .unwrap();
    assert!(
        previews
            .call(&f.core, "preview_ws_read", json!(web))
            .await
            .is_err()
    );
    assert!(
        super::super::preview::Host::default()
            .call(&f.core, "preview_http", json!({"previewId":id,"path":"/"}))
            .await
            .is_err()
    );
}
