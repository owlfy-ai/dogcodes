use super::{CleanupPlan, MetadataCleanupPlan, ReviewResult, ReviewStore};
use serde::Serialize;

#[derive(Clone)]
pub struct StorageCleanupPlan {
    pub content: CleanupPlan,
    pub metadata: MetadataCleanupPlan,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCleanupResult {
    pub removed_count: usize,
    pub removed_bytes: u64,
    pub removed_metadata_count: usize,
}

impl ReviewStore {
    pub fn preview_storage_cleanup(&self) -> ReviewResult<StorageCleanupPlan> {
        Ok(StorageCleanupPlan {
            content: self.preview_cleanup()?,
            metadata: self.preview_metadata_cleanup()?,
        })
    }
    pub fn execute_storage_cleanup(
        &self,
        plan: &StorageCleanupPlan,
    ) -> ReviewResult<StorageCleanupResult> {
        self.validate_metadata_cleanup(&plan.metadata)?;
        let content = self.execute_cleanup(&plan.content)?;
        let metadata = self
            .execute_metadata_cleanup(&plan.metadata)
            .map_err(|error| {
                format!(
                    "{error}; 已回收 {} 个内容对象（{} 字节），请重新盘点",
                    content.removed_count, content.removed_bytes
                )
            })?;
        Ok(StorageCleanupResult {
            removed_count: content.removed_count + metadata,
            removed_bytes: content.removed_bytes + plan.metadata.bytes,
            removed_metadata_count: metadata,
        })
    }
}
