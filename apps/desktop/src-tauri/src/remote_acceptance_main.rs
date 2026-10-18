fn main() {
    if !desktop_lib::daemon::run_remote_acceptance_if_requested() {
        std::process::exit(2);
    }
}
