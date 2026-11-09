//! Two conversations on one ACP agent through Core and a private ACP fixture.
//! One native ACP process runs a single turn at a time, so a second
//! conversation must not be sent to the process the first one is busy on.
#![cfg(unix)]
#[path = "support/installations.rs"]
mod installations;
use agent_core::Core;
use agent_domain::{ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use serde_json::Value;
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};

const FIXTURE: &str = r"#!/usr/bin/env python3
import os,sys,json
os.chdir(os.path.dirname(os.path.abspath(__file__)))
count=0;pending=None
def out(v):print(json.dumps(v),flush=True)
def reply(r,v):out({'jsonrpc':'2.0','id':r['id'],'result':v})
for line in sys.stdin:
 r=json.loads(line)
 fd=os.open('requests.jsonl',os.O_WRONLY|os.O_CREAT|os.O_APPEND,0o600)
 os.write(fd,(json.dumps(dict(r,pid=os.getpid()))+'\n').encode());os.close(fd)
 m=r.get('method')
 if m=='session/cancel' and pending:reply(pending,{'stopReason':'cancelled'});pending=None
 if 'id' not in r:continue
 if m=='initialize':reply(r,{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'authMethods':[]})
 elif m=='session/new':count+=1;reply(r,{'sessionId':'s-'+str(os.getpid())+'-'+str(count)})
 elif m=='session/prompt':
  sid=r['params']['sessionId']
  out({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':sid,'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'ok'}}}})
  if r['params']['prompt'][-1]['text'].endswith('slow'):pending=r
  else:reply(r,{'stopReason':'end_turn'})
 else:reply(r,{})
";

async fn running(core: &Core, task: &TaskId) -> Option<TaskSummary> {
    core.bootstrap()
        .await
        .unwrap()
        .tasks
        .into_iter()
        .find(|t| &t.task_id == task)
        .filter(|t| t.active_primary_run.is_some())
}

async fn idle(core: &Core, task: &TaskId) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while running(core, task).await.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn submit(core: &Arc<Core>, task: &TaskId, message: &str) {
    let state = core.bootstrap().await.unwrap();
    let version = state
        .tasks
        .iter()
        .find(|t| &t.task_id == task)
        .unwrap()
        .stream_version;
    let mut request = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: message.into(),
        model: None,
        thinking: None,
        permission_mode: None,
        attachments: vec![],
        executor_override: None,
        review_fix: None,
    }));
    request.expected_stream_version = Some(version);
    core.execute(request).await.unwrap();
}

fn prompts(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|request| request["method"] == "session/prompt")
        .collect()
}

async fn failed(core: &Core, task: &TaskId) -> bool {
    core.timeline(task)
        .await
        .unwrap()
        .iter()
        .any(|item| item.text.contains("pending confirmation") || item.kind.contains("fail"))
}

#[tokio::test]
async fn a_second_conversation_runs_while_the_first_is_still_working() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("kimi-fixture");
    std::fs::write(&executable, FIXTURE).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
    let executor = installations::probe(&core, "kimi").await;
    let driver = Arc::new(
        acp_driver::AcpDriver::new(
            "kimi",
            executor.installation_id.as_deref().unwrap(),
            acp_driver::AcpLaunch {
                executable: executable.into(),
                args: vec![],
            },
            None,
            driver_api::SessionResumeCapability::Supported,
        )
        .unwrap(),
    );
    core.register_driver(driver).await;
    let project = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            target_id: None,
            project_id: project.clone(),
            display_name: "concurrent ACP fixture".into(),
            path: root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    let tasks: Vec<TaskId> = (0..3).map(|_| TaskId::new()).collect();
    for task in &tasks {
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id: project.clone(),
                title: "conversation".into(),
                preferred_executor: executor.clone(),
            },
        )))
        .await
        .unwrap();
    }

    // The first conversation keeps its turn open on the shared process.
    submit(&core, &tasks[0], "slow").await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while prompts(root.path()).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    // Both later conversations finish while the first is still running.
    for task in &tasks[1..] {
        submit(&core, task, "fast").await;
        idle(&core, task).await;
        assert!(!failed(&core, task).await, "conversation failed");
    }
    assert!(running(&core, &tasks[0]).await.is_some());
    // A follow-up turn stays on its conversation's own process and session.
    submit(&core, &tasks[1], "again").await;
    idle(&core, &tasks[1]).await;
    assert!(!failed(&core, &tasks[1]).await);

    let sent = prompts(root.path());
    assert_eq!(sent.len(), 4);
    let pid = |i: usize| sent[i]["pid"].as_u64().unwrap();
    let session = |i: usize| sent[i]["params"]["sessionId"].as_str().unwrap().to_owned();
    assert_ne!(pid(0), pid(1));
    assert_ne!(pid(1), pid(2));
    assert_ne!(pid(0), pid(2));
    assert_eq!(pid(1), pid(3));
    assert_eq!(session(1), session(3));
}
