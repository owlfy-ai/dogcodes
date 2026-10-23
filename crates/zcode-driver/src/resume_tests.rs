//! Installed CLI acceptance with an isolated home and a loopback model; no paid API calls.
#![cfg(target_os = "macos")]
use super::*;
use std::io::{BufRead, BufReader as StdReader};

const SERVER: &str = r"
import json,sys
from http.server import BaseHTTPRequestHandler,HTTPServer
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args): pass
 def do_POST(self):
  body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
  with open(sys.argv[1], 'a') as f: f.write(json.dumps(body)+'\n')
  text='DOGCODES_NATIVE_MEMORY' if 'fixture-second' not in json.dumps(body.get('messages',[])[-1:]) else 'DOGCODES_COLD_OK'
  usage={'input_tokens':21,'output_tokens':5}
  self.send_response(200)
  if not body.get('stream'):
   self.send_header('Content-Type','application/json');self.end_headers()
   self.wfile.write(json.dumps({'id':'fixture','type':'message','role':'assistant','model':'fixture-model','content':[{'type':'text','text':text}],'stop_reason':'end_turn','usage':usage}).encode());return
  self.send_header('Content-Type','text/event-stream');self.end_headers()
  events=[('message_start',{'message':{'id':'fixture','type':'message','role':'assistant','model':'fixture-model','content':[],'stop_reason':None,'usage':usage}}),('content_block_start',{'index':0,'content_block':{'type':'text','text':''}}),('content_block_delta',{'index':0,'delta':{'type':'text_delta','text':text}}),('content_block_stop',{'index':0}),('message_delta',{'delta':{'stop_reason':'end_turn','stop_sequence':None},'usage':usage}),('message_stop',{})]
  for event,data in events:
   data['type']=event;self.wfile.write(('event: '+event+'\ndata: '+json.dumps(data)+'\n\n').encode());self.wfile.flush()
server=HTTPServer(('127.0.0.1',0),Handler)
print(server.server_address[1],flush=True)
server.serve_forever()
";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires installed ZCode; isolated local model, never uses user accounts"]
async fn installed_native_cold_resume() {
    const CHILD: &str = "DOGCODES_ZCODE_RESUME_FIXTURE";
    let app = PathBuf::from("/Applications/ZCode.app/Contents/MacOS/ZCode");
    let version =
        std::process::Command::new(agent_runtime::electron_node_executable(&app).unwrap())
            .args([
                "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs",
                "--version",
            ])
            .env("ELECTRON_RUN_AS_NODE", "1")
            .output()
            .unwrap();
    assert!(version.status.success());
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), "0.16.9");
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let recorded = root.path().join("requests.jsonl");
        let mut server = std::process::Command::new("/usr/bin/python3")
            .args(["-u", "-c", SERVER])
            .arg(&recorded)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut port = String::new();
        StdReader::new(server.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let native = root.path().join(".zcode/v2");
        std::fs::create_dir_all(&native).unwrap();
        let config = json!({"schemaVersion":1,"config":{
            "providerConfigRules":{"providerRules":[{"providerId":"fixture","providerName":"Fixture","enabled":true,"config":{
                "group":"standard-personal","access":{"type":"api-key","apiKey":"test-only"},
                "api":{"type":"anthropic-messages","baseUrl":format!("http://127.0.0.1:{}",port.trim())},"personalModelIds":["fixture-model"]}}]},
            "modelConfigRules":{"providerModelRules":[{"providerId":"fixture","modelId":"fixture-model","config":{"optionSpecs":{"reasoningLevel":{"values":["disabled"],"map":"{}"}}}}],"manualProviderModelRules":[]},
            "defaultModelSelection":{"providerId":"fixture","modelId":"fixture-model"}
        }});
        std::fs::write(native.join("provider_config.json"), config.to_string()).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "resume_tests::installed_native_cold_resume",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HOME", root.path())
            .env("ZCODE_DATA_BASE_DIR", root.path())
            .env("ZCODE_DESKTOP_HOME_DIR", root.path())
            .env("ZCODE_CREDENTIAL_SECRET", "fixture-only")
            .env("DOGCODES_TEST_WORKSPACE", root.path())
            .output();
        let _ = server.kill();
        let _ = server.wait();
        let result = result.unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let requests = std::fs::read_to_string(recorded).unwrap();
        let next: Value = requests
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|r| {
                r["messages"]
                    .as_array()
                    .is_some_and(|m| m.last().unwrap().to_string().contains("fixture-second"))
            })
            .expect("second native request");
        assert!(
            next["messages"]
                .to_string()
                .contains("DOGCODES_NATIVE_MEMORY"),
            "native history was not restored"
        );
        assert!(next["messages"].to_string().contains("fixture-first"));
        println!(
            "ZCode native cold resume: same session, local provider and historical messages verified"
        );
        return;
    }
    let cwd = std::env::var("DOGCODES_TEST_WORKSPACE").unwrap();
    let launch = ZcodeLaunch {
        app_binary: "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
        cli_script: "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs".into(),
    };
    let driver = || {
        ZcodeDriver::new(
            "zcode",
            &InstallationId::new().to_string(),
            launch.clone(),
            Some("0.16.9".into()),
            verified_resume_capability(Some("0.16.9")),
        )
        .unwrap()
    };
    let first = driver();
    let session = first
        .create_session(SessionRequest {
            working_directory: cwd.clone(),
            additional_directories: vec![],
            history_mcp: None,
            permission_mode: None,
        })
        .await
        .unwrap();
    let id = session.native_session_id.clone();
    run(
        &first,
        &session,
        &cwd,
        "fixture-first: remember the fixture word",
    )
    .await;
    first.shutdown(session).await.unwrap();
    drop(first);
    let second = driver();
    let (a, b) = tokio::join!(
        second.resume_session(&id, &cwd),
        second.resume_session(&id, &cwd)
    );
    let session = a.unwrap();
    assert_eq!(b.unwrap().native_session_id, id);
    run(
        &second,
        &session,
        &cwd,
        "fixture-second: repeat the remembered word",
    )
    .await;
    second.shutdown(session).await.unwrap();
    assert!(matches!(
        second
            .resume_session("00000000-0000-4000-8000-000000000000", &cwd)
            .await,
        Err(DriverError::SessionMissing(_))
    ));
}

async fn run(driver: &ZcodeDriver, session: &SessionHandle, cwd: &str, prompt: &str) {
    let stream = driver
        .prompt(
            session,
            HubPromptRequest {
                prompt: prompt.into(),
                working_directory: cwd.into(),
                attachments: vec![],
                permission_mode: None,
                additional_directories: vec![],
                context: vec![],
            },
        )
        .await
        .unwrap();
    let events: Vec<_> = tokio::time::timeout(
        std::time::Duration::from_mins(1),
        stream.collect::<Vec<_>>(),
    )
    .await
    .expect("native fixture timeout");
    let events: Vec<_> = events.into_iter().collect::<Result<_, _>>().unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DriverEvent::TurnCompleted { .. })),
        "{events:?}"
    );
    let text = events
        .iter()
        .filter_map(|e| {
            if let DriverEvent::TextDelta { text, .. } = e {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<String>();
    assert!(
        text.contains(if prompt.starts_with("fixture-first") {
            "DOGCODES_NATIVE_MEMORY"
        } else {
            "DOGCODES_COLD_OK"
        }),
        "{text}"
    );
}
