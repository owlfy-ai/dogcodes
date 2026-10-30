//! Opt-in control-plane check; never starts a model turn or prints account data.
#[tokio::test]
#[ignore = "requires DOGCODES_TEST_CODEX_EXECUTABLE and a locally authenticated Codex"]
async fn local_codex_readiness() {
    let executable =
        std::env::var_os("DOGCODES_TEST_CODEX_EXECUTABLE").expect("DOGCODES_TEST_CODEX_EXECUTABLE");
    let result = codex_driver::CodexDriver::probe_ready(std::path::Path::new(&executable)).await;
    assert!(result.is_ok(), "Codex readiness failed: {result:?}");
}
