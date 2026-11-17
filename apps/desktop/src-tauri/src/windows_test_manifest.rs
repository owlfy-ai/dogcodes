#[test]
fn common_controls_v6_manifest_is_linked() {
    // Referencing the crate keeps its comctl32 v6 manifest in this test executable.
    desktop_test_manifest::embed();
}
