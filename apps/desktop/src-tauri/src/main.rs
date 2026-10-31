#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if agent_core::computer_use::run_proxy_if_requested() {
        return;
    }
    if agent_core::history_bridge::run_query_if_requested() {
        return;
    }
    if antigravity_driver::approval_bridge::run_hook_if_requested() {
        return;
    }
    if desktop_lib::daemon::run_if_requested() {
        return;
    }
    desktop_lib::run();
}
