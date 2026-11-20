//! Claude can consume an extra user record in the current result or in a later result.
use super::{DriverError, DriverEvent, SessionProcess, lifecycle};
use async_trait::async_trait;
use driver_api::{SteerDelivery, TurnSteering};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};

type Receipt = oneshot::Sender<Result<SteerDelivery, DriverError>>;
pub(super) struct Sender {
    pub commands: mpsc::UnboundedSender<lifecycle::Command>,
    pub turn: mpsc::WeakUnboundedSender<DriverEvent>,
}
pub(super) struct Request {
    turn: mpsc::WeakUnboundedSender<DriverEvent>,
    text: String,
    receipt: Receipt,
}

#[async_trait]
impl TurnSteering for Sender {
    async fn send(&self, _: &str, text: &str) -> Result<SteerDelivery, DriverError> {
        let (receipt, reply) = oneshot::channel();
        let request = Request {
            turn: self.turn.clone(),
            text: text.into(),
            receipt,
        };
        if self
            .commands
            .send(lifecycle::Command::Steer(request))
            .is_err()
        {
            return Ok(SteerDelivery::Rejected);
        }
        reply
            .await
            .map_err(|_| DriverError::Disconnected("Claude steering receipt missing".into()))?
    }
}

#[derive(Default)]
pub(super) struct Pending(HashMap<String, Option<Receipt>>);
impl Pending {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub async fn dispatch(
        &mut self,
        process: &SessionProcess,
        request: Request,
        accepting: bool,
    ) -> Result<(), DriverError> {
        let current = request.turn.upgrade().is_some_and(|turn| {
            process
                .updates
                .lock()
                .expect("updates sink")
                .as_ref()
                .is_some_and(|sink| sink.same_channel(&turn))
        });
        if !accepting || !current {
            let _ = request.receipt.send(Ok(SteerDelivery::Rejected));
            return Ok(());
        }
        let uuid = uuid::Uuid::new_v4().to_string();
        process
            .send_bounded(
                &json!({"type":"user","uuid":uuid,"session_id":process.session_id,
            "parent_tool_use_id":null,"message":{"role":"user","content":request.text}}),
            )
            .await?;
        self.0.insert(uuid, Some(request.receipt));
        Ok(())
    }

    pub fn observe(&mut self, frame: &Value) {
        if frame["type"] == "user" && frame["isReplay"] == true {
            if let Some(receipt) = frame["uuid"]
                .as_str()
                .and_then(|id| self.0.get_mut(id))
                .and_then(Option::take)
            {
                let _ = receipt.send(Ok(SteerDelivery::Accepted));
            }
        } else if frame["type"] == "result" {
            if let Some(ids) = frame["user_message_uuids"].as_array() {
                for id in ids.iter().filter_map(Value::as_str) {
                    if let Some(Some(receipt)) = self.0.remove(id) {
                        let _ = receipt.send(Ok(SteerDelivery::Accepted));
                    }
                }
            } else {
                // Older stream-json versions echo each consumed user UUID before its result.
                self.0.retain(|_, receipt| receipt.is_some());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_results_cover_only_their_own_inputs() {
        let mut pending = Pending::default();
        let (one, mut receipt_one) = oneshot::channel();
        let (two, mut receipt_two) = oneshot::channel();
        pending.0.insert("one".into(), Some(one));
        pending.0.insert("two".into(), Some(two));
        pending.observe(&json!({"type":"user","isReplay":true,"uuid":"two"}));
        assert_eq!(
            receipt_two.try_recv().unwrap().unwrap(),
            SteerDelivery::Accepted
        );
        pending.observe(&json!({"type":"result","user_message_uuids":["initial","one"]}));
        assert_eq!(
            receipt_one.try_recv().unwrap().unwrap(),
            SteerDelivery::Accepted
        );
        assert!(!pending.is_empty());
        pending.observe(&json!({"type":"result","user_message_uuids":["two"]}));
        assert!(pending.is_empty());
    }

    #[test]
    fn result_without_ids_only_settles_replayed_inputs() {
        let mut pending = Pending::default();
        let (one, mut receipt) = oneshot::channel();
        pending.0.insert("one".into(), Some(one));
        pending.observe(&json!({"type":"result"}));
        assert!(!pending.is_empty());
        pending.observe(&json!({"type":"user","isReplay":true,"uuid":"one"}));
        pending.observe(&json!({"type":"result"}));
        assert!(pending.is_empty());
        assert_eq!(
            receipt.try_recv().unwrap().unwrap(),
            SteerDelivery::Accepted
        );
    }
}
