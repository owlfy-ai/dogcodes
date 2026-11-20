use super::{Connection, DriverError, json};
use async_trait::async_trait;
use driver_api::{SteerDelivery, TurnSteering};
use serde_json::Value;
use std::sync::{Arc, OnceLock};

use tokio::sync::oneshot;

pub(super) type Outcome = Arc<OnceLock<SteerDelivery>>;

pub(super) struct Sender {
    pub connection: Arc<Connection>,
    pub turn: uuid::Uuid,
}

/// Pi echoes consumed steering input as a native user message.
pub(super) struct Delivery {
    text: String,
    enqueued: bool,
    outcome: Outcome,
    consumed: oneshot::Sender<()>,
}

impl Delivery {
    pub fn observe(slot: &mut Option<Self>, frame: &Value) {
        let Some(pending) = slot.as_mut() else {
            return;
        };
        if frame["type"] == "queue_update" {
            if frame["steering"].as_array().is_some_and(|queue| {
                queue
                    .iter()
                    .any(|item| item.as_str() == Some(&pending.text))
            }) {
                pending.enqueued = true;
            }
        } else if pending.enqueued
            && frame["type"] == "message_start"
            && frame["message"]["role"] == "user"
            && frame["message"]["content"]
                .as_array()
                .is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "text" && block["text"].as_str() == Some(&pending.text)
                    })
                })
        {
            let delivered = slot.take().expect("pending steer");
            let _ = delivered.outcome.set(SteerDelivery::Accepted);
            let _ = delivered.consumed.send(());
        }
    }
}

#[async_trait]
impl TurnSteering for Sender {
    async fn send(&self, _: &str, text: &str) -> Result<SteerDelivery, DriverError> {
        let connection = &self.connection;
        let (consumed, delivery) = oneshot::channel();
        let outcome = Outcome::default();
        {
            let _admission = connection.admission.lock().await;
            {
                let mut state = connection.state.lock().expect("Pi state");
                if state.cancelling {
                    return Ok(SteerDelivery::Rejected);
                }
                let Some(turn) = state
                    .turn
                    .as_mut()
                    .filter(|turn| turn.id == self.turn && turn.delivery.is_none())
                else {
                    return Ok(SteerDelivery::Rejected);
                };
                turn.delivery = Some(Delivery {
                    text: text.to_owned(),
                    outcome: outcome.clone(),
                    enqueued: false,
                    consumed,
                });
                state.steered = Some(outcome.clone());
            }
            match connection
                .call(json!({"type":"steer","message":text}))
                .await
            {
                Ok(data) if data["disposition"] != "handled" => {}
                result => {
                    let mut state = connection.state.lock().expect("Pi state");
                    if let Some(turn) = state.turn.as_mut().filter(|turn| turn.id == self.turn) {
                        turn.delivery = None;
                    }
                    state.steered = None;
                    return match result {
                        Ok(_) => Ok(SteerDelivery::Accepted), // Native input handler consumed the input.
                        Err(DriverError::Protocol(_)) if !state.disconnected => {
                            Ok(SteerDelivery::Rejected)
                        }
                        Err(error) => Err(error),
                    };
                }
            }
        }
        // Allow approvals and stop while Pi runs tools ahead of the queued input.
        if delivery.await.is_ok() {
            return Ok(SteerDelivery::Accepted);
        }

        // Settlement can race a successful native enqueue. Inspect the queue on
        // the same resident before reporting delivery or admitting another prompt.
        let _admission = connection.admission.lock().await;
        if let Some(known) = outcome.get() {
            return Ok(*known);
        }
        connection.settle_steering().await?;
        outcome
            .get()
            .copied()
            .ok_or_else(|| DriverError::Disconnected("Pi steering delivery unconfirmed".into()))
    }
}

impl Connection {
    /// Caller holds admission and the native turn has settled.
    pub(super) async fn settle_steering(&self) -> Result<(), DriverError> {
        let outcome = {
            let mut state = self.state.lock().expect("Pi state");
            if state.turn.is_some() || state.cancelling {
                return Ok(());
            }
            let Some(outcome) = state.steered.clone() else {
                return Ok(());
            };
            if outcome.get().is_some() {
                state.steered = None;
                return Ok(());
            }
            outcome
        };
        let result = self.clear_orphaned_steering().await.inspect_err(|_| {
            // A later prompt must not wake a queue whose contents are unknown.
            self.request_close();
        })?;
        let _ = outcome.set(result);
        self.state.lock().expect("Pi state").steered = None;
        Ok(())
    }

    async fn clear_orphaned_steering(&self) -> Result<SteerDelivery, DriverError> {
        let native = self.call(json!({"type":"get_state"})).await?;
        if native["isStreaming"] != false {
            return Err(DriverError::Protocol(
                "Pi did not confirm steering settlement".into(),
            ));
        }
        if native["pendingMessageCount"].as_u64() == Some(0) {
            return Ok(SteerDelivery::Accepted);
        }
        let cleared = self.call(json!({"type":"clear_queue"})).await?;
        let leftover = cleared["steering"].as_array().ok_or_else(|| {
            DriverError::Protocol("Pi did not confirm clearing its steering queue".into())
        })?;
        Ok(if leftover.is_empty() {
            SteerDelivery::Accepted
        } else {
            SteerDelivery::Rejected
        })
    }
}
