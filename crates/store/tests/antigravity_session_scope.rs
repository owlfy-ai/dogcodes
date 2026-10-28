//! Core/SQLite/Antigravity integration; private CLI and loopback Hook fixtures only.
#![cfg(unix)]
#[path = "support/installations.rs"]
mod installations;
use agent_core::Core;
use agent_domain::{ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use driver_api::AgentDriver;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

const FIXTURE: &str = r"#!/usr/bin/env python3
import sys,json,os,socket,pathlib
if '--print=/hooks' in sys.argv:
 roots=[sys.argv[i+1] for i,a in enumerate(sys.argv[:-1]) if a=='--add-dir']
 source=pathlib.Path(roots[-1])/'.agents/hooks.json'
 action=json.loads(source.read_text())['dogcodes-approvals']['PreToolUse'][0]
 print(json.dumps({'event':'command_result','command':{'name':'hooks','data':{'hooks':[{'name':'dogcodes-approvals','enabled':True,'source':str(source),'actions':[{'event':'PreToolUse','matcher':action['matcher'],'command':action['hooks'][0]['command']}]}]}}}),flush=True)
 sys.exit(0)
def out(v):print(json.dumps(v),flush=True)
step=0
for line in sys.stdin:
 r=json.loads(line)
 if r.get('event')!='user':continue
 text=[b.get('text','') for b in r['message']['content']]
 message='question' if 'question' in text else 'approval' if 'approval' in text else 'first'
 step+=1
 with open('requests.jsonl','a') as f:f.write(json.dumps({'message':message})+'\n')
 if step==1:out({'event':'init','conversation_id':'core-agy-fixture','init':{}})
 if message in ['question','approval']:
  tool={'name':'ask_question','args':{'questions':[{'id':'answer','question':'Choose'}]}} if message=='question' else {'name':'run_command','args':{'CommandLine':'echo private'}}
  host,port=os.environ['DOGCODES_APPROVAL_ADDR'].split(':')
  with socket.create_connection((host,int(port)),timeout=10) as s:
   s.sendall((json.dumps({'token':os.environ['DOGCODES_APPROVAL_TOKEN'],'payload':{'conversationId':'core-agy-fixture','stepIdx':step,'toolCall':tool}})+'\n').encode())
   response=b''
   while True:
    chunk=s.recv(8192)
    if not chunk:break
    response+=chunk
  if response:
   with open('replies.jsonl','a') as f:f.write(response.decode()+'\n')
 out({'event':'result','result':{'conversation_id':'core-agy-fixture','status':'SUCCESS','response':'private fixture complete','num_turns':step}})
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
#[tokio::test]
async fn hot_antigravity_alias_handoff_keeps_core_approvals_questions_and_cancel_scoped() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("agy-fixture");
    std::fs::write(&executable, FIXTURE).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
    let executor = installations::probe(&core, "antigravity").await;
    let driver = Arc::new(
        antigravity_driver::AntigravityDriver::new(
            "antigravity",
            executor.installation_id.as_deref().unwrap(),
            executable,
            None,
            driver_api::SessionResumeCapability::Supported,
        )
        .unwrap(),
    );
    core.register_driver(driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "private Antigravity scope fixture".into(),
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
    submit(&core, &task, "approval").await;
    let approval = pending(&core, &task).await;
    let current = driver.current_session_handle(&native_id).await.unwrap();
    assert!(!original.same_scope(&current));
    assert_eq!(current.native_session_id, "core-agy-fixture");
    assert!(driver.interrupt(&original).await.is_err());
    let mut approve = CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
        task_id: task.clone(),
        turn_id: approval.turn_id.clone(),
        requested_at_sequence: approval.requested_at_sequence,
        approval_id: approval.approval_id,
        option_id: "allow_once".into(),
        answer: None,
    }));
    approve.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(approve).await.unwrap();
    idle(&core, &task).await;
    submit(&core, &task, "question").await;
    let question = pending(&core, &task).await;
    let second = driver.current_session_handle(&native_id).await.unwrap();
    assert!(!current.same_scope(&second));
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
    submit(&core, &task, "approval").await;
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
    let replies = std::fs::read_to_string(root.path().join("replies.jsonl")).unwrap();
    let replies: Vec<Value> = replies
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert!(replies.len() >= 2);
    assert!(
        replies
            .iter()
            .skip(2)
            .all(|reply| reply["decision"] == "deny")
    );
    assert_eq!(replies[0]["decision"], "allow");
    assert!(replies[1]["reason"].as_str().unwrap().contains("Allow"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("requests.jsonl"))
            .unwrap()
            .lines()
            .count(),
        4
    );
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
