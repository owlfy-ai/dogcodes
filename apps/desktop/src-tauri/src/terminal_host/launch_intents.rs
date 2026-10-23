//! A durable fence for an unconfirmed launch. An absent read is not a fence:
//! its original request may still be in flight on another connection.
use super::TerminalHost;
use serde_json::{Value, json};

impl TerminalHost {
    pub(super) fn launch_retired(&self, id: &str) -> Result<bool, String> {
        uuid::Uuid::parse_str(id).map_err(|_| "无效的终端启动标识")?;
        if self
            .retired
            .lock()
            .map_err(|_| "终端状态不可用")?
            .contains(id)
        {
            return Ok(true);
        }
        let Some(directory) = &self.directory else {
            return Ok(false);
        };
        match std::fs::read_to_string(directory.join("retired").join(id)) {
            Ok(value) if value == id => Ok(true),
            Ok(_) => Err("原启动取消记录损坏，未重新启动".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }
    pub(crate) fn launch_receipt(&self, id: &str) -> Result<Option<Value>, String> {
        uuid::Uuid::parse_str(id).map_err(|_| "无效的终端启动标识")?;
        if let Some(session) = self.find_session(id)? {
            return Ok(Some(
                json!({"operationId":id,"state":"accepted","intent":session.intent,"terminal":session.info()?}),
            ));
        }
        if self.launch_retired(id)? {
            return Ok(Some(json!({"operationId":id,"state":"retired"})));
        }
        Ok(None)
    }
    pub(crate) async fn retire_launch(&self, id: &str) -> Result<Value, String> {
        let _admission = self.admission.lock().await;
        if let Some(receipt) = self.launch_receipt(id)? {
            return Ok(receipt);
        }
        if let Some(parent) = &self.directory {
            let directory = parent.join("retired");
            std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::fs::File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
            let mut file =
                tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
            std::io::Write::write_all(&mut file, id.as_bytes()).map_err(|e| e.to_string())?;
            file.as_file_mut().sync_all().map_err(|e| e.to_string())?;
            file.persist(directory.join(id))
                .map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::fs::File::open(&directory)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
        }
        self.retired
            .lock()
            .map_err(|_| "终端状态不可用")?
            .insert(id.into());
        Ok(json!({"operationId":id,"state":"retired"}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_host::{LaunchIntent, ProjectLaunchRequest};
    fn intent(id: &str, cwd: &std::path::Path) -> LaunchIntent {
        LaunchIntent {
            project_launch: None,
            operation_id: id.into(),
            project_id: "project".into(),
            directory_index: 0,
            task_id: None,
            command_id: None,
            label: "fixture".into(),
            cwd: cwd.to_path_buf(),
            command: Some("exit 0".into()),
            cols: 80,
            rows: 24,
        }
    }
    #[tokio::test]
    async fn a_retired_launch_survives_restart_and_blocks_both_launch_entrypoints() {
        let temp = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let host = TerminalHost::persistent(temp.path().into());
        assert_eq!(host.retire_launch(&id).await.unwrap()["state"], "retired");
        drop(host);
        let recovered = TerminalHost::persistent(temp.path().into());
        assert_eq!(
            recovered.launch_receipt(&id).unwrap().unwrap()["state"],
            "retired"
        );
        assert!(recovered.start(intent(&id, temp.path())).await.is_err());
        let request = ProjectLaunchRequest {
            operation_id: id,
            task_id: "task".into(),
            generation: "original".into(),
            directory_index: 0,
            command_id: "dev".into(),
            cols: 80,
            rows: 24,
        };
        assert!(recovered.existing_project_launch(&request).is_err());
        assert!(recovered.list("project").unwrap().is_empty());
    }
    #[tokio::test]
    async fn retirement_and_late_launch_share_the_same_admission_lock() {
        let temp = tempfile::tempdir().unwrap();
        let host = std::sync::Arc::new(TerminalHost::persistent(temp.path().into()));
        let id = uuid::Uuid::new_v4().to_string();
        let request = intent(&id, temp.path());
        let lock = host.admission.lock().await;
        let h = host.clone();
        let key = id.clone();
        let retire = tokio::spawn(async move { h.retire_launch(&key).await });
        tokio::task::yield_now().await;
        let h = host.clone();
        let launch = tokio::spawn(async move { h.start_resolved(request, None).await });
        tokio::task::yield_now().await;
        drop(lock);
        assert_eq!(retire.await.unwrap().unwrap()["state"], "retired");
        assert!(launch.await.unwrap().is_err());
        assert!(host.list("project").unwrap().is_empty());
    }
}
