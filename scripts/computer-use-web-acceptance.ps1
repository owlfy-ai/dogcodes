param([ValidateSet('codex','deepseek-harness','claude-code','cursor','kimi','grok','codebuddy','kiro','opencode','antigravity','zcode')][string]$Agent='codex', [string]$Executable, [string]$OutputDirectory)
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$stamp=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$caseRoot=if ($OutputDirectory) { [System.IO.Path]::GetFullPath($OutputDirectory) } else { Join-Path $root ".local/computer-use/acceptance/web-$Agent-$stamp" }
[System.IO.Directory]::CreateDirectory($caseRoot) | Out-Null
$runner=Join-Path $root 'target/debug/examples/live_computer_use.exe'
$prepared = & node (Join-Path $PSScriptRoot 'prepare-computer-use.mjs')
if ($LASTEXITCODE -ne 0) { throw 'Cua preparation failed' }
$driver=Join-Path $prepared 'cua-driver.exe'
$binary=$Executable
if (!$binary) {
    $discovered = (& $runner --discover | Out-String | ConvertFrom-Json)
    $binary=($discovered | Where-Object provider -EQ $Agent).installations | Where-Object status -EQ 'installed' | Select-Object -First 1 -ExpandProperty executable
    if (!$binary) { throw "No installed $Agent found" }
}
$node=(Get-Command node).Source
$server=Start-Process -FilePath $node -ArgumentList @(('"'+(Join-Path $PSScriptRoot 'computer-use-web-fixture.mjs')+'"'),('"'+$caseRoot+'"'),"$stamp") -WindowStyle Hidden -PassThru
$profile=Join-Path $caseRoot 'browser-profile'
try {
    $deadline=[DateTimeOffset]::UtcNow.AddSeconds(15)
    while (!(Test-Path -LiteralPath (Join-Path $caseRoot 'fixture.json'))) { if ([DateTimeOffset]::UtcNow -gt $deadline) { throw 'Web fixture did not start' }; Start-Sleep -Milliseconds 100 }
    $fixture=Get-Content -LiteralPath (Join-Path $caseRoot 'fixture.json') -Raw | ConvertFrom-Json
    $edge=@('C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe','C:/Program Files/Microsoft/Edge/Application/msedge.exe') | Where-Object {Test-Path -LiteralPath $_} | Select-Object -First 1
    if (!$edge) { throw 'Microsoft Edge is required for the local web GUI acceptance' }
    # Dedicated interactive app window; existing browser profiles are untouched.
    Start-Process -FilePath $edge -ArgumentList @('--no-first-run',('--user-data-dir="'+$profile+'"'),('--app='+$fixture.url)) | Out-Null
    $prompt=@"
Test and fix the local web application in this workspace. Its existing Edge window title is '$($fixture.title)' and its URL is $($fixture.url).
Use the dogcodes computer-use tools to inspect and operate this specific window. The unit price is 7 dollars. Enter quantity 3 and click Calculate total. Observe the incorrect total in the UI and save a before.png screenshot in this workspace.
Use fresh screenshot-grounded window-local x/y coordinates for at least one Calculate total click, so this run also tests pixel input. Foreground delivery is permitted.
Then use ordinary code editing tools to fix app.js in this workspace so total equals unit price times quantity. Do not change the UI to hard-code the expected answer, do not edit the server or write report.json yourself.
Refresh the page through the GUI, enter quantity 3 again, and verify that the UI shows Total: 21 dollars. Click Save verification and verify 'Verification saved'. Save an after.png screenshot. Do not use DOM/CDP, browser scripts or HTTP requests to operate or inspect the UI; this test verifies native screenshot and input capability. Code inspection/editing in this workspace is permitted.
"@
    $promptFile=Join-Path $caseRoot 'prompt.txt'
    [System.IO.File]::WriteAllText($promptFile,$prompt,[System.Text.UTF8Encoding]::new($false))
    & $runner $Agent $binary $driver $caseRoot $promptFile edit 2>&1 | Tee-Object -FilePath (Join-Path $caseRoot 'run.log')
    if ($LASTEXITCODE -ne 0) { throw "Agent runner failed: $caseRoot" }
    $report=Get-Content -LiteralPath (Join-Path $caseRoot 'report.json') -Raw | ConvertFrom-Json
    if ($report.quantity -ne 3 -or $report.total -ne 21) { throw 'Browser interaction did not produce the correct result' }
    if (!(Test-Path -LiteralPath (Join-Path $caseRoot 'before.png')) -or !(Test-Path -LiteralPath (Join-Path $caseRoot 'after.png'))) { throw 'Missing GUI screenshots' }
    Write-Output "PASS $caseRoot"
} finally {
    # Only processes launched with this fixture's exact private profile are ours.
    Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -and $_.CommandLine.Contains($profile) } | ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
    $server.Refresh(); if (!$server.HasExited) { Stop-Process -Id $server.Id -ErrorAction SilentlyContinue }
}
