//! Stands in for the app executable as `SSH_ASKPASS` in integration tests.
fn main() {
    if target_link::proxy::run_if_requested() {
        return;
    }
    if !target_link::askpass::run_if_requested() {
        eprintln!("askpass variables are not set");
        std::process::exit(2);
    }
}
