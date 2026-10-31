param([ValidateSet('codex','deepseek-harness','claude-code','cursor','kimi','grok','codebuddy','kiro','opencode','antigravity','zcode')][string]$Agent='codex', [string]$Executable, [string]$OutputDirectory)
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$stamp=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$caseRoot=if ($OutputDirectory) { [System.IO.Path]::GetFullPath($OutputDirectory) } else { Join-Path $root ".local/computer-use/acceptance/$Agent-$stamp" }
[System.IO.Directory]::CreateDirectory($caseRoot) | Out-Null
$prepared = & node (Join-Path $PSScriptRoot 'prepare-computer-use.mjs')
if ($LASTEXITCODE -ne 0) { throw 'Cua preparation failed' }
$driver=Join-Path $prepared 'cua-driver.exe'
$runner=Join-Path $root 'target/debug/examples/live_computer_use.exe'
if (!(Test-Path -LiteralPath $runner)) { throw 'Build cargo build -p agent-store --example live_computer_use first' }
$binary=$Executable
if (!$binary) {
    $discovered = (& $runner --discover | Out-String | ConvertFrom-Json)
    $binary=($discovered | Where-Object provider -EQ $Agent).installations | Where-Object status -EQ 'installed' | Select-Object -First 1 -ExpandProperty executable
    if (!$binary) { throw "No installed $Agent found" }
}
$code=(Get-Random -Minimum 1000 -Maximum 9999).ToString()
$instance="$Agent-$stamp"
# This is the interactive GUI under test; its window must be visible to capture.
$fixture=Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-STA','-File',('"'+(Join-Path $PSScriptRoot 'computer-use-fixture.ps1')+'"'),'-OutputDirectory',('"'+$caseRoot+'"'),'-Instance',$instance,'-VisualCode',$code) -WindowStyle Normal -PassThru -RedirectStandardError (Join-Path $caseRoot 'fixture-error.log')
try {
    $deadline=[DateTimeOffset]::UtcNow.AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 200
        $fixture.Refresh()
        if ($fixture.HasExited) { throw 'GUI fixture exited; inspect fixture-error.log' }
    } while ($fixture.MainWindowTitle -notlike '*dog.codes Computer Use Fixture*' -and [DateTimeOffset]::UtcNow -lt $deadline)
    $prompt=@"
Operate ONLY the existing window titled 'dog.codes Computer Use Fixture - $instance' in process $($fixture.Id), using the dogcodes computer-use tools. Discover it with list_windows filtered by that pid.
Set Project name to 'dogcodes-$Agent'. Set Notes to the exact text 'GUI verified 中文输入'.
Read the four-digit code from the blue picture using a real screenshot, then enter it into Verification code and click Save report.
Use actual GUI interaction to read and operate the form; do not control the UI through scripts, shell commands, filesystem writes or application APIs. Native image-view tools may inspect returned screenshots, and a read-only listing of this workspace may confirm your saved screenshot files. Do not read the fixture source, prompt files or other acceptance files. Do not guess the code or request it from the user.
Verify the visible 'Report saved successfully' result and capture the final window screenshot with screenshot_out_file under this workspace. Report what you actually observed. If image understanding is unavailable, report that limitation honestly.
"@
    $promptFile=Join-Path $caseRoot 'prompt.txt'
    [System.IO.File]::WriteAllText($promptFile,$prompt,[System.Text.UTF8Encoding]::new($false))
    & $runner $Agent $binary $driver $caseRoot $promptFile 2>&1 | Tee-Object -FilePath (Join-Path $caseRoot 'run.log')
    $exitCode=$LASTEXITCODE
    $resultFile=Join-Path $caseRoot 'result.json'
    if (!(Test-Path -LiteralPath $resultFile)) { throw "No saved GUI result: $caseRoot" }
    $result=Get-Content -LiteralPath $resultFile -Raw | ConvertFrom-Json
    if ($result.project -ne "dogcodes-$Agent" -or $result.notes -ne 'GUI verified 中文输入' -or $result.visualCode -ne $code) { throw "Incorrect GUI result: $caseRoot" }
    $actions=Get-Content -LiteralPath (Join-Path $caseRoot 'evidence/actions.json') -Raw | ConvertFrom-Json
    if (!($actions | Where-Object { $_.payload.imageCount -gt 0 })) { throw 'No image returned through the production MCP proxy' }
    if ($exitCode -ne 0) { throw "Agent runner failed: $exitCode" }
    Write-Output "PASS $caseRoot"
} finally {
    $fixture.Refresh()
    if (!$fixture.HasExited) { Stop-Process -Id $fixture.Id -ErrorAction SilentlyContinue }
}
