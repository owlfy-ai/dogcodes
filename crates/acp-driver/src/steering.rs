//! Text steering on the connection and prompt that accepted the original request.
use super::{AcpDriver, Command, ConnectionHandle, driver_error_from_channel, turn};
use agent_client_protocol::{Agent, ConnectionTo};
use driver_api::{DriverError, SessionHandle};
use driver_api::{SteerDelivery, TurnSteering};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use tokio::sync::oneshot;

#[derive(Clone, Copy)]
enum Wire {
    CodeBuddy,
    Grok,
    Kiro,
    OpenCode,
}

impl Wire {
    fn message(self, turn: &turn::Turn, id: &str, text: &str) -> (&'static str, Value) {
        let session = &turn.session;
        let blocks = json!([{"type":"text","text":text}]);
        match self {
            Self::CodeBuddy => (
                "session/steer",
                json!({"sessionId":session,"contentBlocks":blocks,"clientUserMessageId":id,"expectedRequestId":turn.codebuddy_request.as_deref()}),
            ),
            Self::Grok => ("_x.ai/interject", json!({"sessionId":session,"text":text})),
            Self::Kiro => (
                "_session/steer",
                json!({"sessionId":session,"message":text}),
            ),
            Self::OpenCode => (
                "_session/steering",
                json!({"sessionId":session,"prompt":blocks,"_meta":{"steering":{"idleBehavior":"promptRequired"}}}),
            ),
        }
    }

    fn receipt(self, turn: &turn::Turn, value: &Value) -> Result<SteerDelivery, DriverError> {
        let accepted = match self {
            Self::CodeBuddy if value["steered"] == false => return Ok(SteerDelivery::Rejected),
            Self::CodeBuddy => {
                value["steered"] == true
                    && value["ownerRequestId"].as_str() == turn.codebuddy_request.as_deref()
            }
            Self::Kiro => value["queued"] == true && value["messageId"].is_string(),
            Self::Grok => value["result"]["status"] == "queued",
            Self::OpenCode if value["outcome"] == "promptRequired" => {
                return Ok(SteerDelivery::Rejected);
            }
            Self::OpenCode => value["outcome"] == "injected",
        };
        if accepted {
            Ok(SteerDelivery::Accepted)
        } else {
            Err(DriverError::Protocol(
                "Unrecognized steering receipt".into(),
            ))
        }
    }
}

struct Sender {
    connection: ConnectionHandle,
    turn: Arc<turn::Turn>,
    wire: Wire,
}

pub(super) async fn capture(
    driver: &AcpDriver,
    handle: &SessionHandle,
) -> Option<Arc<dyn TurnSteering>> {
    let connection = driver.shared.connection.lock().await.clone()?;
    let _gate = driver.shared.lifecycle.lock().expect("ACP lifecycle");
    driver.shared.validate_handle(handle).ok()?;
    let wire = match driver.executor.provider_id.as_str() {
        "codebuddy" => Wire::CodeBuddy,
        "grok" => Wire::Grok,
        "kiro" => Wire::Kiro,
        "opencode" if driver.shared.opencode_steering.load(Ordering::Acquire) => Wire::OpenCode,
        _ => return None,
    };
    let turn = driver
        .shared
        .latest_turn
        .lock()
        .expect("ACP turn")
        .clone()?;
    if turn.session != handle.native_session_id || !Arc::ptr_eq(&connection.finished, &turn.epoch) {
        return None;
    }
    Some(Arc::new(Sender {
        connection,
        turn,
        wire,
    }))
}

#[async_trait::async_trait]
impl TurnSteering for Sender {
    async fn send(&self, id: &str, text: &str) -> Result<SteerDelivery, DriverError> {
        let (reply, rx) = oneshot::channel();
        let (method, params) = self.wire.message(&self.turn, id, text);
        self.connection
            .commands
            .send(Command::Steer(Request {
                turn: self.turn.clone(),
                wire: self.wire,
                method,
                params,
                reply,
            }))
            .map_err(|_| driver_error_from_channel())?;
        rx.await.map_err(|_| driver_error_from_channel())?
    }
}

pub(super) struct Request {
    turn: Arc<turn::Turn>,
    wire: Wire,
    method: &'static str,
    params: Value,
    reply: oneshot::Sender<Result<SteerDelivery, DriverError>>,
}

impl Request {
    pub(super) async fn dispatch(self, connection: &ConnectionTo<Agent>) {
        let Ok(request) = agent_client_protocol::UntypedMessage::new(self.method, self.params)
        else {
            return;
        };
        let pending = {
            let dispatch = self.turn.dispatch.lock().expect("ACP prompt dispatch");
            if !dispatch.sent
                || dispatch.cancel_requested
                || self.turn.done.borrow().is_some()
                || self.turn.retiring.load(Ordering::Acquire)
                || self.turn.epoch.load(Ordering::Acquire)
            {
                let _ = self.reply.send(Ok(SteerDelivery::Rejected));
                return;
            }
            connection.send_request(request)
        };
        let result =
            match tokio::time::timeout(std::time::Duration::from_secs(15), pending.block_task())
                .await
            {
                Ok(Ok(value)) => self.wire.receipt(&self.turn, &value),
                Ok(Err(error))
                    if matches!(
                        error.code,
                        agent_client_protocol::ErrorCode::MethodNotFound
                            | agent_client_protocol::ErrorCode::InvalidParams
                    ) =>
                {
                    Ok(SteerDelivery::Rejected)
                }
                Ok(Err(_)) => Err(DriverError::Protocol("Steering receipt unavailable".into())),
                Err(_) => Err(DriverError::Disconnected(
                    "Steering receipt timed out".into(),
                )),
            };
        let _ = self.reply.send(result);
    }
}
