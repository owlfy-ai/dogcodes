//! Actual Core/SQLite/Claude protocol wiring, using only a private Python CLI.
#![cfg(unix)]
#[path = "support/installations.rs"]
mod installations;
use agent_core::Core;
use agent_domain::{ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use driver_api::{AgentDriver, SessionResumeCapability};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

const FIXTURE: &str = r"#!/usr/bin/env python3
import sys,json,threading,time,os
lock=threading.Lock()
def emit(v):
 with lock:print(json.dumps(v),flush=True)
def result():emit({'type':'result','subtype':'success'})
def background():
 seen=0
 while True:
  time.sleep(0.01)
  try:
   with open('continue') as f:n=int(f.read())
  except (OSError,ValueError):continue
  if n<=seen:continue
  seen=n
  emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'autonomous '+str(n)}}})
  emit({'type':'control_request','request_id':'approval-'+str(n),'request':{'subtype':'can_use_tool','tool_name':'Bash','input':{'command':'echo private fixture'}}})
threading.Thread(target=background,daemon=True).start()
for line in sys.stdin:
 r=json.loads(line)
 if r.get('type')=='user':result()
 elif r.get('type')=='control_request':
  request=r['request']
  data={'models':[]} if request['subtype']=='initialize' else {}
  emit({'type':'control_response','response':{'request_id':r['request_id'],'subtype':'success','response':data}})
  if request['subtype']=='interrupt':result()
 elif r.get('type')=='control_response':
  with open('replies.jsonl','a') as f:f.write(json.dumps(r)+'\n')
  if r['response']['request_id']=='approval-1':result()
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
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn approval(core: &Core, task: &TaskId) -> PendingApproval {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(approval) = core
                .pending_approvals(task, 50)
                .await
                .unwrap()
                .into_iter()
                .next()
            {
                return approval;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn hot_reused_claude_continuations_remain_approvable_and_cancellable_through_core() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("claude-fixture");
    std::fs::write(&executable, FIXTURE).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
    let executor = installations::probe(&core, "claude-code").await;
    let driver = Arc::new(
        claude_driver::ClaudeDriver::new(
            "claude-code",
            executor.installation_id.as_deref().unwrap(),
            executable,
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap(),
    );
    core.register_driver(driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "private Claude scope fixture".into(),
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
            title: "private session".into(),
            preferred_executor: executor,
        },
    )))
    .await
    .unwrap();
    let mut previous = None;
    for _ in 0..2 {
        let state = core.bootstrap().await.unwrap();
        let mut request = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            task_id: task.clone(),
            prompt: "private foreground".into(),
            attachments: vec![],
            model: None,
            thinking: None,
            permission_mode: None,
            executor_override: None,
            review_fix: None,
        }));
        request.expected_stream_version = Some(state.tasks[0].stream_version);
        core.execute(request).await.unwrap();
        idle(&core, &task).await;
        let state = core.bootstrap().await.unwrap();
        assert_eq!(state.session_maps.len(), 1);
        let current = driver
            .current_session_handle(&state.session_maps[0].native_session_id)
            .await
            .unwrap();
        if let Some(old) = previous {
            assert!(!current.same_scope(&old));
            assert!(driver.interrupt(&old).await.is_err());
        }
        previous = Some(current);
    }
    std::fs::write(root.path().join("continue"), "1").unwrap();
    let first = approval(&core, &task).await;
    let mut response = CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
        task_id: task.clone(),
        turn_id: first.turn_id.clone(),
        requested_at_sequence: first.requested_at_sequence,
        approval_id: first.approval_id,
        option_id: "allow".into(),
        answer: None,
    }));
    response.expected_stream_version =
        Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(response).await.unwrap();
    idle(&core, &task).await;
    assert!(
        std::fs::read_to_string(root.path().join("replies.jsonl"))
            .unwrap()
            .contains("approval-1")
    );
    std::fs::write(root.path().join("continue"), "2").unwrap();
    let second = approval(&core, &task).await;
    let state = core.bootstrap().await.unwrap();
    let active = state.tasks[0].active_primary_run.as_ref().unwrap();
    assert_eq!(active.turn_id, second.turn_id);
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: task.clone(),
        turn_id: active.turn_id.clone(),
        run_id: active.run_id.clone(),
    }));
    cancel.expected_stream_version = Some(state.tasks[0].stream_version);
    core.execute(cancel).await.unwrap();
    idle(&core, &task).await;
    let events = core.sync_events(0, 1000).await.unwrap();
    let handle = driver
        .current_session_handle(&previous.unwrap().native_session_id)
        .await
        .unwrap();
    driver.shutdown(handle).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "turn.autonomous.started")
            .count(),
        2
    );
    assert!(
        events.iter().any(|e| e.event_type == "turn.completed"
            && e.payload["turnId"] == first.turn_id.to_string())
    );
    assert!(events.iter().any(|e| e.event_type == "turn.failed"
        && e.payload["failureKind"] == "interrupted"
        && e.payload["turnId"] == second.turn_id.to_string()));
}
