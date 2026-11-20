//! Each native turn owns a separate inbox. A captured sender cannot address the next turn.
use super::{Connection, DriverError, IncomingFrame, REQUEST_TIMEOUT};
use async_trait::async_trait;
use driver_api::{SteerDelivery, TurnSteering};
use serde_json::json;
use std::{collections::HashMap, sync::Arc};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

pub(super) type Slot = Arc<std::sync::Mutex<Option<Sender>>>;
type Receipt = oneshot::Sender<Result<SteerDelivery, DriverError>>;

pub(super) struct Request {
    text: String,
    receipt: Receipt,
}
#[derive(Clone)]
pub(super) struct Sender(pub mpsc::Sender<Request>);

#[async_trait]
impl TurnSteering for Sender {
    async fn send(&self, _: &str, text: &str) -> Result<SteerDelivery, DriverError> {
        let (receipt, result) = oneshot::channel();
        if self
            .0
            .send(Request {
                text: text.into(),
                receipt,
            })
            .await
            .is_err()
        {
            return Ok(SteerDelivery::Rejected);
        }
        timeout(REQUEST_TIMEOUT, result)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_else(|| {
                Err(DriverError::Disconnected(
                    "Codex steering receipt missing".into(),
                ))
            })
    }
}

#[derive(Default)]
pub(super) struct Pending(HashMap<u64, Receipt>);
impl Pending {
    pub async fn dispatch(
        &mut self,
        connection: &mut Connection,
        turn: &str,
        request: Request,
        stopping: bool,
    ) -> Result<(), DriverError> {
        if stopping || request.receipt.is_closed() {
            let _ = request.receipt.send(Ok(SteerDelivery::Rejected));
            return Ok(());
        }
        let params = json!({"threadId":connection.thread_id,"expectedTurnId":turn,"input":[{"type":"text","text":request.text}]});
        let id = connection
            .transport
            .begin_request("turn/steer", params)
            .await?;
        self.0.retain(|_, pending| !pending.is_closed());
        self.0.insert(id, request.receipt);
        Ok(())
    }

    pub fn observe(&mut self, frame: &IncomingFrame, turn: &str) {
        if let IncomingFrame::Response(raw) = frame
            && let Some(receipt) = raw["id"].as_u64().and_then(|id| self.0.remove(&id))
        {
            let result = if raw.get("error").is_some() {
                Ok(SteerDelivery::Rejected)
            } else if raw["result"]["turnId"].as_str() == Some(turn) {
                Ok(SteerDelivery::Accepted)
            } else {
                Err(DriverError::Protocol(
                    "Codex acknowledged a different turn".into(),
                ))
            };
            let _ = receipt.send(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_matches_request_and_expected_native_turn() {
        let mut pending = Pending::default();
        for (id, response, expected) in [
            (1, json!({"result":{"turnId":"old"}}), None),
            (
                2,
                json!({"error":{"code":-32602}}),
                Some(SteerDelivery::Rejected),
            ),
            (
                3,
                json!({"result":{"turnId":"current"}}),
                Some(SteerDelivery::Accepted),
            ),
        ] {
            let (send, mut receive) = oneshot::channel();
            pending.0.insert(id, send);
            pending.observe(
                &IncomingFrame::Response(json!({"id":99,"result":{"turnId":"current"}})),
                "current",
            );
            assert!(receive.try_recv().is_err());
            let mut response = response;
            response["id"] = json!(id);
            pending.observe(&IncomingFrame::Response(response), "current");
            assert_eq!(receive.try_recv().unwrap().ok(), expected);
        }
    }
}
