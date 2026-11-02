fn main() {
    #[cfg(windows)]
    windows::run();
}

#[cfg(windows)]
mod windows {
    use serde_json::{Value, json};
    use std::io::{BufRead, Write};

    pub fn run() {
        if std::env::args().any(|arg| arg == "--pi-rpc-fixture") {
            serve_rpc();
            return;
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(rpc_probe_preserves_windows_profile_and_system_locations());
        println!("test rpc_probe_preserves_windows_profile_and_system_locations ... ok");
    }

    fn serve_rpc() {
        // Check the actual peer's environment, not just the .cmd parent's.
        for name in [
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
        ] {
            assert!(
                std::env::var_os(name).is_some_and(|value| !value.is_empty()),
                "missing Windows environment variable: {name}"
            );
        }
        let directory = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        let mut requests = std::fs::File::create(directory.join("requests.log")).unwrap();
        let mut output = std::io::stdout().lock();
        for line in std::io::stdin().lock().lines() {
            let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let kind = request["type"].as_str().unwrap();
            writeln!(requests, "{kind}").unwrap();
            let data = match kind {
                "get_state" => json!({"model": {"provider": "fixture", "id": "model"}}),
                "get_available_models" => {
                    json!({"models": [{"provider": "fixture", "id": "model"}]})
                }
                _ => panic!("unexpected RPC request: {kind}"),
            };
            let response =
                json!({"type": "response", "id": request["id"], "success": true, "data": data});
            writeln!(output, "{response}").unwrap();
            output.flush().unwrap();
        }
    }

    async fn rpc_probe_preserves_windows_profile_and_system_locations() {
        let directory = tempfile::Builder::new()
            .prefix("dogcodes-pi-中文 用户-")
            .tempdir()
            .unwrap();
        // Keep the batch launcher and its Unicode/space-containing path, but
        // avoid depending on PowerShell startup and JSON cmdlets for an RPC peer.
        std::fs::copy(
            std::env::current_exe().unwrap(),
            directory.path().join("rpc-fixture.exe"),
        )
        .unwrap();
        let executable = directory.path().join("pi.cmd");
        std::fs::write(
            &executable,
            "@echo off\r\n\"%~dp0rpc-fixture.exe\" --pi-rpc-fixture 2>\"%~dp0fixture-stderr.log\"\r\n",
        )
        .unwrap();
        // Cover the discovered and canonicalized launcher paths used in practice.
        for executable in [
            executable.clone(),
            std::fs::canonicalize(executable).unwrap(),
        ] {
            let result = pi_driver::PiDriver::probe_ready(&executable).await;
            let requests =
                std::fs::read_to_string(directory.path().join("requests.log")).unwrap_or_default();
            let diagnostics = std::fs::read_to_string(directory.path().join("fixture-stderr.log"))
                .unwrap_or_default();
            assert!(
                result.is_ok(),
                "Pi RPC launchers require native Windows system/profile variables after env_clear: {result:?}; requests: {requests}; fixture: {diagnostics}"
            );
            assert_eq!(
                requests.lines().collect::<Vec<_>>(),
                ["get_state", "get_available_models"]
            );
        }
    }
}
