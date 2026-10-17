//! Coalesced hints only. Subscribers must re-query durable business state.
use crate::workspace_service::watch::WatchMessage;
use std::{sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

pub(crate) struct Changes {
    version: watch::Sender<u64>,
    slots: Arc<Semaphore>,
}
impl Changes {
    pub fn new() -> Self {
        Self {
            version: watch::channel(0).0,
            slots: Arc::new(Semaphore::new(64)),
        }
    }
    pub fn changed(&self) {
        self.version
            .send_modify(|value| *value = value.wrapping_add(1));
    }
    pub fn subscribe(&self) -> Result<Subscription, String> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "状态订阅数量已达上限，请关闭不使用的视图")?;
        Ok(Subscription {
            receiver: self.version.subscribe(),
            initial: true,
            _permit: permit,
        })
    }
    #[cfg(test)]
    pub fn active_count(&self) -> usize {
        64 - self.slots.available_permits()
    }
}
pub(crate) struct Subscription {
    receiver: watch::Receiver<u64>,
    initial: bool,
    _permit: OwnedSemaphorePermit,
}
impl Subscription {
    pub async fn next(&mut self) -> Result<WatchMessage, String> {
        if self.initial {
            self.initial = false;
            self.receiver.borrow_and_update();
            return Ok(WatchMessage::Invalidated);
        }
        match tokio::time::timeout(Duration::from_secs(15), self.receiver.changed()).await {
            Ok(result) => {
                result.map_err(|_| "后台状态订阅已关闭")?;
                Ok(WatchMessage::Invalidated)
            }
            Err(_) => Ok(WatchMessage::Heartbeat),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn hints_coalesce_without_losing_reconnect_refresh_and_release_capacity() {
        let changes = Changes::new();
        let mut first = changes.subscribe().unwrap();
        assert!(matches!(
            first.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        for _ in 0..100 {
            changes.changed();
        }
        assert!(matches!(
            first.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), first.next())
                .await
                .is_err()
        );
        let others: Vec<_> = (0..63).map(|_| changes.subscribe().unwrap()).collect();
        assert!(changes.subscribe().is_err());
        drop(others);
        drop(first);
        assert_eq!(changes.active_count(), 0);
        let mut reopened = changes.subscribe().unwrap();
        assert!(matches!(
            reopened.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        drop(changes);
        assert!(reopened.next().await.is_err());
    }
}
