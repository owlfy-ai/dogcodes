use super::{CoreClient, read_frame, write_frame};
use crate::workspace_service::{
    DirectoryRequest, Entry, FileMatches, Preview,
    watch::{WatchMessage, WatchService},
};
use agent_core::Core;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{io::AsyncReadExt, net::TcpStream};

impl CoreClient {
    pub async fn workspace_roots(&self, task_id: String) -> Result<Vec<PathBuf>, String> {
        self.call("workspace_roots", json!(task_id)).await
    }
    pub async fn workspace_directory(
        &self,
        request: DirectoryRequest,
    ) -> Result<Vec<Entry>, String> {
        self.call("workspace_directory", json!(request)).await
    }
    pub async fn workspace_preview(&self, request: DirectoryRequest) -> Result<Preview, String> {
        self.call("workspace_preview", json!(request)).await
    }
    pub async fn workspace_find_files(
        &self,
        task_id: String,
        query: String,
        hide_noise: bool,
    ) -> Result<FileMatches, String> {
        self.call("workspace_find_files", json!((task_id, query, hide_noise)))
            .await
    }
    pub async fn workspace_watch(
        &self,
        request: DirectoryRequest,
    ) -> Result<WorkspaceWatchStream, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, services) = self {
            return Ok(WorkspaceWatchStream::Local(
                services
                    .control
                    .workspace_watchers
                    .open(core.clone(), request)
                    .await?,
            ));
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut stream = self.connect("workspace_watch", json!(request)).await?;
            let reply: Result<(), String> = read_frame(&mut stream).await?;
            reply?;
            Ok(WorkspaceWatchStream::Remote(stream))
        })
        .await
        .map_err(|_| "目录订阅连接超时".to_owned())?
    }
}
pub enum WorkspaceWatchStream {
    Remote(TcpStream),
    #[cfg(any(test, feature = "review-acceptance"))]
    Local(crate::workspace_service::watch::Subscription),
}
impl WorkspaceWatchStream {
    pub async fn next(&mut self) -> Result<WatchMessage, String> {
        match self {
            Self::Remote(stream) => {
                tokio::time::timeout(Duration::from_secs(45), read_frame(stream))
                    .await
                    .map_err(|_| "目录监听连接超时，正在重新连接".to_owned())?
            }
            #[cfg(any(test, feature = "review-acceptance"))]
            Self::Local(subscription) => subscription.next().await,
        }
    }
}
pub(super) async fn serve_watch(
    core: &Arc<Core>,
    watches: &WatchService,
    stream: &mut TcpStream,
    args: Value,
) -> Result<(), String> {
    let subscription = tokio::select! {
        _ = stream.read_u8() => return Ok(()),
        result = async {
            let request: DirectoryRequest = serde_json::from_value(args).map_err(|e| e.to_string())?;
            watches.open(core.clone(), request).await
        } => result,
    };
    write_frame(
        stream,
        &subscription.as_ref().map(|_| ()).map_err(Clone::clone),
    )
    .await?;
    let mut subscription = subscription?;
    loop {
        let message = tokio::select! {
            _ = stream.read_u8() => return Ok(()),
            result = subscription.next() => result.unwrap_or_else(|message| WatchMessage::Error { code: "disconnected".into(), message }),
        };
        let ended = matches!(message, WatchMessage::Error { .. });
        tokio::time::timeout(Duration::from_secs(15), write_frame(stream, &message))
            .await
            .map_err(|_| "workspace subscriber stopped reading")??;
        if ended {
            return Ok(());
        }
    }
}
