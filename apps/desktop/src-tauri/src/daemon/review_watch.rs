use super::{CoreClient, read_frame, write_frame};
#[cfg(any(test, feature = "review-acceptance"))]
use crate::review::watch::Subscription;
use crate::{review::watch::Changes, workspace_service::watch::WatchMessage};
use agent_core::Core;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{io::AsyncReadExt, net::TcpStream};

impl CoreClient {
    pub(crate) async fn review_watch(&self, task: String) -> Result<InvalidationStream, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, services) = self {
            return Ok(InvalidationStream::Local(
                services
                    .control
                    .review
                    .changes
                    .open(core.clone(), task)
                    .await?,
            ));
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut stream = self.connect("review_watch", json!(task)).await?;
            let reply: Result<(), String> = read_frame(&mut stream).await?;
            reply?;
            Ok(InvalidationStream::Remote(stream))
        })
        .await
        .map_err(|_| "状态订阅连接超时")?
    }
    pub(crate) async fn project_runs_watch(
        &self,
        task: String,
    ) -> Result<InvalidationStream, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, services) = self {
            return Ok(InvalidationStream::Local(
                services
                    .control
                    .review
                    .changes
                    .open(core.clone(), task)
                    .await?,
            ));
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut stream = self.connect("project_runs_watch", json!(task)).await?;
            let reply: Result<(), String> = read_frame(&mut stream).await?;
            reply?;
            Ok(InvalidationStream::Remote(stream))
        })
        .await
        .map_err(|_| "运行状态订阅连接超时")?
    }
    pub(crate) async fn submission_watch(&self) -> Result<InvalidationStream, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(_, services) = self {
            return Ok(InvalidationStream::LocalState(
                services.control.submissions.subscribe()?,
            ));
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut stream = self.connect("submission_watch", Value::Null).await?;
            let reply: Result<(), String> = read_frame(&mut stream).await?;
            reply?;
            Ok(InvalidationStream::Remote(stream))
        })
        .await
        .map_err(|_| "发送状态订阅连接超时")?
    }
}
pub(crate) enum InvalidationStream {
    Remote(TcpStream),
    #[cfg(any(test, feature = "review-acceptance"))]
    Local(Subscription),
    #[cfg(any(test, feature = "review-acceptance"))]
    LocalState(crate::state_changes::Subscription),
}
impl InvalidationStream {
    pub(crate) async fn next(&mut self) -> Result<WatchMessage, String> {
        match self {
            Self::Remote(stream) => {
                tokio::time::timeout(Duration::from_secs(45), read_frame(stream))
                    .await
                    .map_err(|_| "状态订阅连接超时".to_owned())?
            }
            #[cfg(any(test, feature = "review-acceptance"))]
            Self::Local(subscription) => subscription.next().await,
            #[cfg(any(test, feature = "review-acceptance"))]
            Self::LocalState(subscription) => subscription.next().await,
        }
    }
}
pub(super) async fn serve(
    core: &Arc<Core>,
    changes: &Changes,
    stream: &mut TcpStream,
    args: Value,
) -> Result<(), String> {
    let subscription = tokio::select! {
        _ = stream.read_u8() => return Ok(()),
        result = async {
            let task: String = serde_json::from_value(args).map_err(|e| e.to_string())?;
            changes.open(core.clone(), task).await.map(LocalSubscription::Review)
        } => result,
    };
    forward(stream, subscription).await
}
enum LocalSubscription {
    Review(crate::review::watch::Subscription),
    State(crate::state_changes::Subscription),
}
impl LocalSubscription {
    async fn next(&mut self) -> Result<WatchMessage, String> {
        match self {
            Self::Review(stream) => stream.next().await,
            Self::State(stream) => stream.next().await,
        }
    }
}
pub(super) async fn serve_submissions(
    submissions: &crate::submission_intents::SubmissionIntents,
    stream: &mut TcpStream,
    args: Value,
) -> Result<(), String> {
    let subscription = if args.is_null() {
        submissions.subscribe().map(LocalSubscription::State)
    } else {
        Err("发送状态订阅参数无效".into())
    };
    forward(stream, subscription).await
}
async fn forward(
    stream: &mut TcpStream,
    subscription: Result<LocalSubscription, String>,
) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(15),
        write_frame(
            stream,
            &subscription.as_ref().map(|_| ()).map_err(Clone::clone),
        ),
    )
    .await
    .map_err(|_| "状态订阅连接写入超时")??;
    let mut subscription = subscription?;
    loop {
        let message = tokio::select! {
            _ = stream.read_u8() => return Ok(()),
            result = subscription.next() => result.unwrap_or_else(|message| WatchMessage::Error { code: "disconnected".into(), message }),
        };
        let ended = matches!(message, WatchMessage::Error { .. });
        tokio::time::timeout(Duration::from_secs(15), write_frame(stream, &message))
            .await
            .map_err(|_| "状态订阅客户端停止读取")??;
        if ended {
            return Ok(());
        }
    }
}
