//! Compose bounded per-directory captures without confusing identical relative paths.
use crate::{WorkspaceContextProvider, WorkspaceTurnCapture};
use agent_protocol::{WorkspaceCollectionDiagnostic, WorkspaceDiagnosticKind};
use async_trait::async_trait;
use serde_json::{Value, json};

fn unavailable() -> WorkspaceCollectionDiagnostic {
    WorkspaceCollectionDiagnostic {
        kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
    }
}
fn bounded(mut value: Value) -> Result<Value, WorkspaceCollectionDiagnostic> {
    if serde_json::to_vec(&value).map_err(|_| unavailable())?.len() > 1024 * 1024 {
        crate::review_analysis::workflow::discard_context(&mut value);
    }
    if serde_json::to_vec(&value).map_err(|_| unavailable())?.len() > 1024 * 1024 {
        return Err(unavailable());
    }
    Ok(value)
}
struct Capture(Vec<(String, Box<dyn WorkspaceTurnCapture>)>);
#[async_trait]
impl WorkspaceTurnCapture for Capture {
    async fn checkpoint(&self) -> Result<Option<Value>, WorkspaceCollectionDiagnostic> {
        let mut roots = Vec::new();
        for (path, capture) in &self.0 {
            let value = capture.checkpoint().await?.ok_or_else(unavailable)?;
            roots.push(json!({"workspacePath":path,"capture":value}));
        }
        bounded(json!({"schemaVersion":2,"roots":roots})).map(Some)
    }
    async fn finish(&self) -> Result<Value, WorkspaceCollectionDiagnostic> {
        let mut roots = Vec::new();
        for (path, capture) in &self.0 {
            roots.push((path.clone(), capture.finish().await?));
        }
        merge(roots)
    }
}
fn merge(roots: Vec<(String, Value)>) -> Result<Value, WorkspaceCollectionDiagnostic> {
    let mut files = Vec::new();
    let mut patches = Vec::new();
    for (root, mut value) in roots {
        for mut file in value["files"].as_array().ok_or_else(unavailable)?.clone() {
            let relative = file["path"].as_str().ok_or_else(unavailable)?.to_owned();
            let path = std::path::Path::new(&relative);
            if path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(unavailable());
            }
            file["relativePath"] = json!(relative);
            file["workspacePath"] = json!(root);
            file["path"] = json!(std::path::Path::new(&root).join(path).to_string_lossy());
            files.push(file);
        }
        if let Some(object) = value.as_object_mut() {
            object.remove("files");
        }
        patches.push(json!({"workspacePath":root,"changes":value}));
    }
    bounded(
        json!({"schemaVersion":2,"attribution":"workspace_interval","scope":"all selected project directories; bounded per-root capture","files":files,"patch":"","roots":patches}),
    )
}
pub(crate) async fn begin(
    provider: &dyn WorkspaceContextProvider,
    paths: &[String],
) -> Result<Box<dyn WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic> {
    if paths.len() == 1 {
        return provider.begin_turn(&paths[0]).await;
    }
    if paths.is_empty() {
        return Err(unavailable());
    }
    let mut captures = Vec::new();
    for path in paths {
        captures.push((path.clone(), provider.begin_turn(path).await?));
    }
    Ok(Box::new(Capture(captures)))
}
pub(crate) async fn recover(
    provider: &dyn WorkspaceContextProvider,
    paths: &[String],
    checkpoint: Value,
) -> Result<Value, WorkspaceCollectionDiagnostic> {
    if checkpoint["schemaVersion"] != 2 {
        return provider
            .recover_turn(paths.first().ok_or_else(unavailable)?, checkpoint)
            .await;
    }
    let roots = checkpoint["roots"].as_array().ok_or_else(unavailable)?;
    if roots.len() != paths.len()
        || roots
            .iter()
            .zip(paths)
            .any(|(root, path)| root["workspacePath"].as_str() != Some(path.as_str()))
    {
        return Err(unavailable());
    }
    let mut recovered = Vec::new();
    for (root, path) in roots.iter().zip(paths) {
        recovered.push((
            path.clone(),
            provider.recover_turn(path, root["capture"].clone()).await?,
        ));
    }
    let mut result = merge(recovered)?;
    result["attribution"] = json!("workspace_recovery_interval");
    result["recoveredAfterInterruption"] = json!(true);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspaceContextSnapshot;
    struct Provider;
    struct Root(String);
    #[async_trait]
    impl WorkspaceTurnCapture for Root {
        async fn checkpoint(&self) -> Result<Option<Value>, WorkspaceCollectionDiagnostic> {
            Ok(Some(json!({"schemaVersion":1,"root":self.0})))
        }
        async fn finish(&self) -> Result<Value, WorkspaceCollectionDiagnostic> {
            Ok(
                json!({"files":[{"path":"same.txt","status":"M","before":{"hex":"61"},"after":{"hex":"62"}}],"patch":"sample"}),
            )
        }
    }
    #[async_trait]
    impl WorkspaceContextProvider for Provider {
        async fn collect(&self, _: &str) -> WorkspaceContextSnapshot {
            WorkspaceContextSnapshot::default()
        }
        async fn begin_turn(
            &self,
            path: &str,
        ) -> Result<Box<dyn WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic> {
            if path == "/unavailable" {
                return Err(unavailable());
            }
            Ok(Box::new(Root(path.into())))
        }
        async fn recover_turn(
            &self,
            path: &str,
            checkpoint: Value,
        ) -> Result<Value, WorkspaceCollectionDiagnostic> {
            if checkpoint["root"] != path {
                return Err(unavailable());
            }
            Root(path.into()).finish().await
        }
    }
    #[tokio::test]
    async fn same_names_recovery_root_validation_and_partial_failure() {
        let paths = vec!["/one".into(), "/two".into()];
        let capture = begin(&Provider, &paths).await.unwrap();
        let checkpoint = capture.checkpoint().await.unwrap().unwrap();
        let changes = capture.finish().await.unwrap();
        assert_eq!(changes["files"][0]["path"], "/one/same.txt");
        assert_eq!(changes["files"][1]["path"], "/two/same.txt");
        assert_eq!(changes["roots"][1]["changes"]["patch"], "sample");
        let recovered = recover(&Provider, &paths, checkpoint.clone())
            .await
            .unwrap();
        assert_eq!(recovered["files"], changes["files"]);
        assert_eq!(recovered["attribution"], "workspace_recovery_interval");
        assert!(
            recover(&Provider, &["/two".into(), "/one".into()], checkpoint)
                .await
                .is_err()
        );
        assert!(
            begin(&Provider, &["/one".into(), "/unavailable".into()])
                .await
                .is_err()
        );
    }
    #[test]
    fn reject_traversal_and_aggregate_overflow() {
        assert!(
            merge(vec![(
                "/one".into(),
                json!({"files":[{"path":"../escape"}]})
            )])
            .is_err()
        );
        assert!(
            merge(vec![(
                "/one".into(),
                json!({"files":[],"patch":"x".repeat(1024*1024)})
            )])
            .is_err()
        );
    }
}
