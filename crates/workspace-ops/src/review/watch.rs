//! Bounded invalidations, not a second business-state store. Reconnect always queries facts.
use crate::ReviewFacts;
use crate::workspace_service::watch::WatchMessage;
use std::sync::Arc;

pub struct Changes(Arc<crate::state_changes::Changes>);
impl Default for Changes {
    fn default() -> Self {
        Self::new()
    }
}
impl Changes {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(crate::state_changes::Changes::new()))
    }
    #[must_use]
    pub fn source(&self) -> Arc<crate::state_changes::Changes> {
        self.0.clone()
    }
    pub fn changed(&self) {
        self.0.changed();
    }
    pub async fn open(
        &self,
        core: Arc<dyn ReviewFacts>,
        task: String,
    ) -> Result<Subscription, String> {
        let changes = self.0.subscribe()?;
        super::service::require_task(core.as_ref(), &task).await?;
        Ok(Subscription {
            core,
            task,
            changes,
        })
    }
}
pub struct Subscription {
    core: Arc<dyn ReviewFacts>,
    task: String,
    changes: crate::state_changes::Subscription,
}
impl Subscription {
    pub async fn next(&mut self) -> Result<WatchMessage, String> {
        let message = self.changes.next().await?;
        super::service::require_task(self.core.as_ref(), &self.task).await?;
        Ok(message)
    }
}
