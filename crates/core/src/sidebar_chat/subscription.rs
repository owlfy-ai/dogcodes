//! Bounded, task-scoped hints. Re-query `ChatState` after every invalidation.
use super::{Arc, Core, Duration, TaskId, watch};
use tokio::sync::OwnedSemaphorePermit;

pub enum ChatChange {
    Invalidated,
    Heartbeat,
}

pub struct ChatSubscription {
    core: std::sync::Weak<Core>,
    task: TaskId,
    receiver: watch::Receiver<u64>,
    initial: bool,
    _permit: OwnedSemaphorePermit,
}

impl Core {
    pub async fn subscribe_sidebar_chat(
        self: &Arc<Self>,
        task: TaskId,
    ) -> Result<ChatSubscription, String> {
        let permit = self
            .sidebar_chat_watch_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "问答订阅数量已达上限，请关闭不使用的视图")?;
        let mut chats = self.sidebar_chats.lock().await;
        self.require_task(&task).await.map_err(|e| e.to_string())?;
        self.load_chat(&task, &mut chats).await?;
        Ok(ChatSubscription {
            core: Arc::downgrade(self),
            receiver: chats[&task].changes.subscribe(),
            task,
            initial: true,
            _permit: permit,
        })
    }
}

impl ChatSubscription {
    pub async fn next(&mut self) -> Result<ChatChange, String> {
        let change = if self.initial {
            self.initial = false;
            self.receiver.borrow_and_update();
            ChatChange::Invalidated
        } else {
            match tokio::time::timeout(Duration::from_secs(15), self.receiver.changed()).await {
                Ok(result) => {
                    result.map_err(|_| "后台问答订阅已关闭")?;
                    ChatChange::Invalidated
                }
                Err(_) => ChatChange::Heartbeat,
            }
        };
        let core = self.core.upgrade().ok_or("后台问答服务已关闭")?;
        core.require_task(&self.task)
            .await
            .map_err(|e| e.to_string())?;
        Ok(change)
    }
}
