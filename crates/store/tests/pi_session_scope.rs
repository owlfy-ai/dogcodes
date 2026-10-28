//! Core/SQLite/Pi RPC integration; this executable is a private protocol fixture.
#![cfg(unix)]
#[path = "support/installations.rs"]
mod installations;
use agent_core::Core;
use agent_domain::{ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use driver_api::AgentDriver;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};

const FIXTURE: &str = r"#!/usr/bin/env python3
import sys,json,os,uuid
path=sys.argv[sys.argv.index('--session')+1] if '--session' in sys.argv else os.path.join(os.getcwd(),str(uuid.uuid4())+'.jsonl')
pending=None
def out(v):print(json.dumps(v),flush=True)
def reply(r,data=None):out({'type':'response','id':r['id'],'success':True,'data':data})
def finish():out({'type':'agent_settled'})
for line in sys.stdin:
 r=json.loads(line)
 with open('requests.jsonl','a') as f:f.write(json.dumps(r)+'\n')
 if r['type']=='get_state':reply(r,{'sessionFile':path,'model':{'provider':'fixture','id':'model'},'thinkingLevel':'medium','isStreaming':False})
 elif r['type']=='prompt':
  with open(path,'w') as f:f.write('{}')
  if r['message']=='question':
   pending=r
   out({'type':'extension_ui_request','id':'question-1','method':'select','title':'Choose','options':['Allow','Block']})
  else:reply(r);finish()
 elif r['type']=='extension_ui_response':reply(pending);pending=None;finish()
 elif r['type']=='abort':reply(r);finish()
 else:reply(r)
";

async fn idle(core: &Core, task: &TaskId) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if core
                .bootstrap()
                .await
                .unwrap()
                .tasks
                .iter()
                .find(|t| &t.task_id == task)
                .unwrap()
                .active_primary_run
                .is_none()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn pending(core: &Core, task: &TaskId) -> PendingApproval {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(question) = core
                .pending_approvals(task, 50)
                .await
                .unwrap()
                .into_iter()
                .next()
            {
                return question;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn submit(core: &Arc<Core>, task: &TaskId, message: &str) {
    let state = core.bootstrap().await.unwrap();
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
    request.expected_stream_version = Some(state.tasks[0].stream_version);
    core.execute(request).await.unwrap();
}
fn count(root: &Path, kind: &str) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|request| request["type"] == kind)
        .count()
}

#[tokio::test]
async fn hot_reused_pi_sessions_remain_answerable_and_cancellable_through_core() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("pi-fixture");
    std::fs::write(&executable, FIXTURE).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
    let executor = installations::probe(&core, "pi").await;
    let driver = Arc::new(
        pi_driver::PiDriver::new(
            executor.installation_id.as_deref().unwrap(),
            executable,
            None,
        )
        .unwrap(),
    );
    core.register_driver(driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "private Pi scope fixture".into(),
            path: root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "private task".into(),
            preferred_executor: executor,
        },
    )))
    .await
    .unwrap();
    submit(&core, &task, "first").await;
    idle(&core, &task).await;
    let snapshot = core.bootstrap().await.unwrap();
    let native_id = snapshot.session_maps[0].native_session_id.clone();
    let original = driver.current_session_handle(&native_id).await.unwrap();
    submit(&core, &task, "question").await;
    let question = pending(&core, &task).await;
    let second = driver.current_session_handle(&native_id).await.unwrap();
    assert!(!original.same_scope(&second));
    assert!(driver.interrupt(&original).await.is_err());
    let mut answer = CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
        task_id: task.clone(),
        turn_id: question.turn_id.clone(),
        requested_at_sequence: question.requested_at_sequence,
        approval_id: question.approval_id,
        option_id: "answer".into(),
        answer: Some(agent_domain::QuestionAnswer {
            cancelled: false,
            values: json!({"answer":"Allow"}),
        }),
    }));
    answer.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(answer).await.unwrap();
    idle(&core, &task).await;
    submit(&core, &task, "question").await;
    let cancelling = pending(&core, &task).await;
    let third = driver.current_session_handle(&native_id).await.unwrap();
    assert!(!second.same_scope(&third));
    assert!(driver.shutdown(second).await.is_err());
    let state = core.bootstrap().await.unwrap();
    let run = state.tasks[0].active_primary_run.as_ref().unwrap();
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: task.clone(),
        turn_id: run.turn_id.clone(),
        run_id: run.run_id.clone(),
    }));
    cancel.expected_stream_version = Some(state.tasks[0].stream_version);
    core.execute(cancel).await.unwrap();
    idle(&core, &task).await;
    let events = core.sync_events(0, 1000).await.unwrap();
    driver.shutdown(third).await.unwrap();
    assert_eq!(count(root.path(), "prompt"), 3);
    assert_eq!(count(root.path(), "abort"), 1);
    assert_eq!(core.bootstrap().await.unwrap().session_maps.len(), 1);
    assert!(
        events.iter().any(|e| e.event_type == "turn.completed"
            && e.payload["turnId"] == question.turn_id.to_string())
    );
    assert!(events.iter().any(|e| e.event_type == "turn.failed"
        && e.payload["failureKind"] == "interrupted"
        && e.payload["turnId"] == cancelling.turn_id.to_string()));
    assert!(core.pending_approvals(&task, 50).await.unwrap().is_empty());
}
