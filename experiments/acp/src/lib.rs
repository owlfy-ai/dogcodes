//! Isolated ACP stable-v1 fact probe support.
//!
//! The dependency manifest deliberately enables no unstable ACP features. Product Driver code
//! must not be added here until a real Agent installation passes the M0 contract matrix.

use agent_client_protocol::Client;
use serde_json::Value;

pub const STABLE_PROTOCOL_VERSION: u16 = 1;

/// Lossless envelope for ACP extension messages intercepted before typed SDK dispatch.
///
/// The official SDK's stable-v1 enum currently removes the required leading underscore while
/// parsing an extension method. Product integrations therefore need to retain the raw wire method
/// and parameters until that behavior changes upstream.
#[derive(Debug, Clone, PartialEq)]
pub struct PreservedExtension {
    pub method: String,
    pub params: Value,
}

/// Preserves a valid ACP extension method and its opaque parameters exactly as received.
#[must_use]
pub fn preserve_extension(method: &str, params: &Value) -> Option<PreservedExtension> {
    (method.len() > 1 && method.starts_with('_')).then(|| PreservedExtension {
        method: method.to_owned(),
        params: params.clone(),
    })
}

/// Compile-time gate for the official SDK's stable-v1 client builder.
#[must_use]
pub fn stable_client_builder() -> impl Sized {
    Client.builder()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use agent_client_protocol::schema::{
        ProtocolVersion,
        v1::{
            AgentCapabilities, AgentNotification, CancelNotification, ClientNotification,
            ContentBlock, ContentChunk, InitializeRequest, InitializeResponse, NewSessionRequest,
            NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest,
            PromptResponse, RequestPermissionOutcome, RequestPermissionRequest,
            RequestPermissionResponse, SessionId, SessionNotification, SessionUpdate, StopReason,
            TextContent, ToolCall,
        },
    };
    use agent_client_protocol::{Agent, ByteStreams, ConnectionTo, JsonRpcMessage, Responder};
    use serde_json::json;
    use tokio::sync::oneshot;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    #[test]
    fn official_sdk_stable_v1_builder_is_available() {
        let _builder = stable_client_builder();
        assert_eq!(STABLE_PROTOCOL_VERSION, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn official_sdk_negotiates_stable_v1_and_capabilities() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (client_writer, agent_reader) = tokio::io::duplex(8 * 1024);
                let (agent_writer, client_reader) = tokio::io::duplex(8 * 1024);
                let agent_transport =
                    ByteStreams::new(agent_writer.compat_write(), agent_reader.compat());
                let client_transport =
                    ByteStreams::new(client_writer.compat_write(), client_reader.compat());

                let agent = Agent.builder().on_receive_request(
                    async |initialize: InitializeRequest, responder, _connection| {
                        responder.respond(
                            InitializeResponse::new(initialize.protocol_version)
                                .agent_capabilities(AgentCapabilities::new().load_session(true)),
                        )
                    },
                    agent_client_protocol::on_receive_request!(),
                );
                tokio::task::spawn_local(async move {
                    let _ = agent.connect_to(agent_transport).await;
                });

                let result = Client
                    .builder()
                    .connect_with(client_transport, async |connection| {
                        let response = connection
                            .send_request(InitializeRequest::new(ProtocolVersion::V1))
                            .block_task()
                            .await?;
                        assert_eq!(response.protocol_version, ProtocolVersion::V1);
                        assert!(response.agent_capabilities.load_session);
                        Ok::<_, agent_client_protocol::Error>(())
                    })
                    .await;
                assert!(result.is_ok(), "stable-v1 contract failed: {result:?}");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stable_v1_session_stream_and_cancel_are_lossless() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let session_id = SessionId::new("redacted-session");
                let new_session_id = session_id.clone();
                let prompt_session_id = session_id.clone();
                let cancel_session_id = session_id.clone();
                let update_count = Arc::new(AtomicUsize::new(0));
                let received_updates = Arc::clone(&update_count);
                let (cancelled_tx, cancelled_rx) = oneshot::channel();
                let cancelled_tx = Arc::new(Mutex::new(Some(cancelled_tx)));

                let agent = Agent
                    .builder()
                    .on_receive_request(
                        async |initialize: InitializeRequest, responder, _connection| {
                            responder.respond(InitializeResponse::new(initialize.protocol_version))
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .on_receive_request(
                        async move |_request: NewSessionRequest,
                                    responder: Responder<NewSessionResponse>,
                                    _connection| {
                            responder.respond(NewSessionResponse::new(new_session_id.clone()))
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .on_receive_request(
                        async move |request: PromptRequest,
                                    responder: Responder<PromptResponse>,
                                    connection| {
                            assert_eq!(request.session_id, prompt_session_id);
                            connection.send_notification(SessionNotification::new(
                                request.session_id,
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                    ContentBlock::Text(TextContent::new("redacted response")),
                                )),
                            ))?;
                            responder.respond(PromptResponse::new(StopReason::EndTurn))
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .on_receive_notification(
                        async move |notification: CancelNotification, _connection| {
                            assert_eq!(notification.session_id, cancel_session_id);
                            if let Some(sender) = cancelled_tx
                                .lock()
                                .expect("cancel acknowledgement mutex should not be poisoned")
                                .take()
                            {
                                let _ = sender.send(());
                            }
                            Ok(())
                        },
                        agent_client_protocol::on_receive_notification!(),
                    );

                let client = Client.builder().on_receive_notification(
                    async move |notification: SessionNotification, _connection| {
                        assert!(matches!(
                            notification.update,
                            SessionUpdate::AgentMessageChunk(_)
                        ));
                        received_updates.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                );

                let result = client
                    .connect_with(agent, async move |connection| {
                        let initialized = connection
                            .send_request(InitializeRequest::new(ProtocolVersion::V1))
                            .block_task()
                            .await?;
                        assert_eq!(initialized.protocol_version, ProtocolVersion::V1);

                        let session = connection
                            .send_request(NewSessionRequest::new("/tmp"))
                            .block_task()
                            .await?;
                        assert_eq!(session.session_id, session_id);

                        let prompt = connection
                            .send_request(PromptRequest::new(
                                session.session_id.clone(),
                                vec![ContentBlock::Text(TextContent::new("redacted prompt"))],
                            ))
                            .block_task()
                            .await?;
                        assert_eq!(prompt.stop_reason, StopReason::EndTurn);

                        connection
                            .send_notification(CancelNotification::new(session.session_id))?;
                        cancelled_rx
                            .await
                            .map_err(|_| agent_client_protocol::Error::internal_error())?;
                        Ok::<_, agent_client_protocol::Error>(())
                    })
                    .await;

                assert!(
                    result.is_ok(),
                    "stable-v1 session contract failed: {result:?}"
                );
                assert_eq!(update_count.load(Ordering::SeqCst), 1);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stable_v1_permission_options_are_preserved_and_never_auto_approved() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (client_writer, agent_reader) = tokio::io::duplex(8 * 1024);
                let (agent_writer, client_reader) = tokio::io::duplex(8 * 1024);
                let agent_transport =
                    ByteStreams::new(agent_writer.compat_write(), agent_reader.compat());
                let client_transport =
                    ByteStreams::new(client_writer.compat_write(), client_reader.compat());

                let client = Client.builder().on_receive_request(
                    async |request: RequestPermissionRequest, responder, _connection| {
                        let option_ids = request
                            .options
                            .iter()
                            .map(|option| option.option_id.0.as_ref())
                            .collect::<Vec<_>>();
                        assert_eq!(option_ids, ["allow-once", "reject-once"]);
                        responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ))
                    },
                    agent_client_protocol::on_receive_request!(),
                );
                let client_task = tokio::task::spawn_local(client.connect_to(client_transport));

                let result = Agent
                    .builder()
                    .connect_with(agent_transport, async |connection: ConnectionTo<_>| {
                        let request = RequestPermissionRequest::new(
                            "redacted-session",
                            ToolCall::new("redacted-tool", "write file").into(),
                            vec![
                                PermissionOption::new(
                                    "allow-once",
                                    "Allow once",
                                    PermissionOptionKind::AllowOnce,
                                ),
                                PermissionOption::new(
                                    "reject-once",
                                    "Reject once",
                                    PermissionOptionKind::RejectOnce,
                                ),
                            ],
                        );
                        let response = connection.send_request(request).block_task().await?;
                        assert!(matches!(
                            response.outcome,
                            RequestPermissionOutcome::Cancelled
                        ));
                        Ok::<_, agent_client_protocol::Error>(())
                    })
                    .await;

                assert!(result.is_ok(), "permission contract failed: {result:?}");
                client_task.abort();
            })
            .await;
    }

    #[test]
    fn stable_v1_sdk_prefix_loss_is_documented_and_raw_envelope_is_lossless() {
        let params = json!({"opaque": {"nested": true}, "sequence": 7});

        let from_agent = AgentNotification::parse_message("_vendor/update", &params)
            .expect("agent extension notification should parse");
        assert_eq!(from_agent.method(), "vendor/update");

        let from_client = ClientNotification::parse_message("_vendor/cancel", &params)
            .expect("client extension notification should parse");
        assert_eq!(from_client.method(), "vendor/cancel");

        let preserved = preserve_extension("_vendor/update", &params)
            .expect("underscore-prefixed method should be preserved");
        assert_eq!(preserved.method, "_vendor/update");
        assert_eq!(preserved.params, params);
        assert!(preserve_extension("vendor/update", &json!({})).is_none());
    }
}
