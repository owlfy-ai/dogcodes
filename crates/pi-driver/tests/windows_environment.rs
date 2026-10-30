#![cfg(windows)]

#[tokio::test]
async fn rpc_probe_preserves_windows_profile_and_system_locations() {
    let directory = tempfile::Builder::new()
        .prefix("dogcodes-pi-中文 用户-")
        .tempdir()
        .unwrap();
    let executable = directory.path().join("pi.cmd");
    std::fs::write(
        &executable,
        "@echo off\r\nif not defined SystemRoot exit /b 41\r\nif not defined USERPROFILE exit /b 42\r\nif not defined LOCALAPPDATA exit /b 43\r\nif not defined APPDATA exit /b 44\r\n\"%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0fixture.ps1\"\r\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("fixture.ps1"),
        r"
while ($null -ne ($line = [Console]::In.ReadLine())) {
    $request = ConvertFrom-Json -InputObject $line
    $data = switch ($request.type) {
        'get_state' { @{model = @{provider = 'fixture'; id = 'model'}} }
        'get_available_models' { @{models = @(@{provider = 'fixture'; id = 'model'})} }
        default { throw 'unexpected RPC request' }
    }
    $response = @{type = 'response'; id = $request.id; success = $true; data = $data}
    [Console]::Out.WriteLine((ConvertTo-Json -InputObject $response -Depth 8 -Compress))
}
",
    )
    .unwrap();
    pi_driver::PiDriver::probe_ready(&std::fs::canonicalize(executable).unwrap())
        .await
        .expect("Pi RPC launchers require native Windows system/profile variables after env_clear");
}
