use super::*;

#[tokio::test]
async fn a_cancelled_partial_read_retains_bytes_for_the_next_frame_read() {
    let (client, mut server) = tokio::io::duplex(256);
    let (reader, writer) = tokio::io::split(client);
    let mut transport = JsonlTransport::new(reader, writer);
    server.write_all(b"{\"method\":\"war").await.unwrap();
    assert!(
        timeout(Duration::from_millis(20), transport.read_value())
            .await
            .is_err()
    );
    server.write_all(b"ning\",\"params\":{}}\n").await.unwrap();
    let frame = transport.read_value().await.unwrap();
    assert_eq!(frame["method"], "warning");
}

#[tokio::test]
async fn a_cancelled_partial_write_rejects_any_following_frame() {
    let (client, _server) = tokio::io::duplex(128);
    let (reader, writer) = tokio::io::split(client);
    let mut transport = JsonlTransport::new(reader, writer);
    assert!(
        timeout(
            Duration::from_millis(20),
            transport.send_value(&json!({"data":"x".repeat(512*1024)}))
        )
        .await
        .is_err()
    );
    let next = timeout(
        Duration::from_millis(20),
        transport.send_value(&json!({"method":"next"})),
    )
    .await;
    assert!(matches!(next, Ok(Err(_))));
}

#[tokio::test]
async fn cancelling_after_a_control_request_is_sent_rejects_reuse_of_unknown_state() {
    let (client, mut server) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(client);
    let mut transport = JsonlTransport::new(reader, writer);
    assert!(
        timeout(
            Duration::from_millis(20),
            transport.request("thread/resume", json!({}))
        )
        .await
        .is_err()
    );
    let mut bytes = vec![0; 4096];
    let read = server.read(&mut bytes).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes[..read]).unwrap()["method"],
        "thread/resume"
    );
    assert!(
        transport
            .send_value(&json!({"method":"next"}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn an_explicit_native_error_does_not_poison_the_next_control_request() {
    let responses=b"{\"id\":1,\"error\":{\"code\":-32602,\"message\":\"invalid\"}}\n{\"id\":2,\"result\":{\"ok\":true}}\n";
    let mut transport = JsonlTransport::new(responses.as_slice(), Vec::new());
    assert!(transport.request("invalid", json!({})).await.is_err());
    assert_eq!(
        transport.request("valid", json!({})).await.unwrap().result["ok"],
        true
    );
}
