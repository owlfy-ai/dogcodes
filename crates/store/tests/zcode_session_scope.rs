//! Core/SQLite/ZCode integration; private shared CLI, no installed runtime or models.
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
import sys,json,os
os.chdir(os.path.dirname(os.path.abspath(__file__)))
sessions={};sequence=0;turn=0;pending=None;active=None
def out(v):print(json.dumps(v),flush=True)
def reply(r,v=None):out({'id':r['id'],'result':v if v is not None else {}})
def snapshot(id):return {'sessionId':id,'session':{'sessionId':id,'workspace':{'workspacePath':os.getcwd()},'status':'running' if active else 'idle'},'projection':{'currentTurnId':active},'settings':{'model':{'current':{'providerId':'private','modelId':'fixture'},'available':[{'ref':{'providerId':'private','modelId':'fixture'}}]}}}
def finish(id,result='success'):
 global active
 label=active;active=None
 out({'method':'turn.completed','params':{'sessionId':id,'turnId':label,'resultType':result}})
for line in sys.stdin:
 r=json.loads(line)
 with open('requests.jsonl','a') as f:f.write(json.dumps(r)+'\n')
 if 'method' not in r:
  with open('replies.jsonl','a') as f:f.write(json.dumps(r.get('result'))+'\n')
  if pending:finish(pending);pending=None
  continue
 method=r['method'];id=r.get('params',{}).get('sessionId')
 if method in ['session/create','session/resume']:
  if method=='session/create':sequence+=1;id='session-'+str(sequence)
  sessions[id]=True;reply(r,snapshot(id))
 elif method=='session/list':reply(r,{'sessions':[snapshot(id) for id in sessions]})
 elif method=='session/read':
  if id in sessions:reply(r,snapshot(id))
  else:out({'id':r['id'],'error':{'code':-32004,'message':'Session is not active: '+id}})
 elif method=='session/close':sessions.pop(id,None);reply(r,{'closed':True})
 elif method=='session/send':
  turn+=1;active='turn-'+str(turn);reply(r)
  out({'method':'turn.started','params':{'sessionId':id,'turnId':active}})
  text=r['params']['content']
  if text in ['approval','question']:
   pending=id
   if text=='question':out({'id':'reused-server-request','method':'interaction/requestUserInput','params':{'sessionId':id,'turnId':active,'questions':[{'question':'Choose','options':['Allow','Block']}]}})
   else:out({'id':'reused-server-request','method':'interaction/requestPermission','params':{'sessionId':id,'turnId':active,'requestId':'reused-permission-id','toolName':'bash','input':'echo private','options':[{'kind':'allow_once','name':'Allow once','optionId':'allow_once','response':{'decision':'allow'}},{'kind':'deny','name':'Deny','optionId':'deny','response':{'decision':'deny'}}]}})
  else:finish(id)
 elif method=='session/stop':reply(r);pending=None;finish(id,'cancelled')
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
#[tokio::test]
async fn shared_zcode_worker_keeps_hot_turn_approvals_questions_and_cancellation_scoped() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("zcode-fixture");
    std::fs::write(&executable, FIXTURE).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
    let executor = installations::probe(&core, "zcode").await;
    let driver = Arc::new(
        zcode_driver::ZcodeDriver::new(
            "zcode",
            executor.installation_id.as_deref().unwrap(),
            zcode_driver::ZcodeLaunch {
                app_binary: executable,
                cli_script: root.path().join("unused.cjs"),
            },
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
            display_name: "private ZCode scope fixture".into(),
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
    assert_eq!(current.native_session_id, "session-1");
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
            values: json!({"answer_0":"Allow"}),
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
    assert_eq!(replies[1]["action"], "accept");
    assert_eq!(replies[1]["content"]["answer_0"], "Allow");
    let requests = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
    let requests: Vec<Value> = requests
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "session/send")
            .count(),
        4
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "session/stop")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "session/create")
            .count(),
        1
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
