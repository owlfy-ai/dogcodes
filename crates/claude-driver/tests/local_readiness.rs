//! Opt-in control-plane check; never starts a model turn or prints account data.
#[tokio::test]
#[ignore = "requires DOGCODES_TEST_CLAUDE_EXECUTABLE and a local Claude Code installation"]
async fn local_claude_readiness() {
    let executable = std::env::var_os("DOGCODES_TEST_CLAUDE_EXECUTABLE")
        .expect("DOGCODES_TEST_CLAUDE_EXECUTABLE");
    let result = claude_driver::ClaudeDriver::probe_ready(std::path::Path::new(&executable)).await;
    assert!(result.is_ok(), "Claude readiness failed: {result:?}");
}
