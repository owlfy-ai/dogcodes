param(
    [string[]]$Agents=@('codex','deepseek-harness','claude-code','cursor','kimi','grok','codebuddy','kiro','opencode','antigravity','zcode'),
    [ValidateRange(1,20)][int]$Repeat=3,
    [switch]$Web,
    [switch]$IncludeClaude=$true,
    [string]$OutputDirectory
)
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$runner=Join-Path $root 'target/debug/examples/live_computer_use.exe'
if (!(Test-Path -LiteralPath $runner)) { throw 'Build cargo build -p agent-store --example live_computer_use first' }
if (!$OutputDirectory) { $OutputDirectory=Join-Path $root ('.local/computer-use/acceptance/matrix-'+[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()) }
[System.IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
$installed=(& $runner --discover | Out-String | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) { throw 'Agent discovery failed' }
$rows=[System.Collections.Generic.List[object]]::new()
foreach ($agent in $Agents) {
    if ($agent -eq 'pi' -or ($agent -eq 'claude-code' -and !$IncludeClaude)) {
        $rows.Add(@{agent=$agent;state='skipped';reason='Excluded by operator policy'})
        continue
    }
    $binary=($installed | Where-Object provider -EQ $agent).installations | Where-Object status -EQ 'installed' | Select-Object -First 1 -ExpandProperty executable
    if (!$binary) { $rows.Add(@{agent=$agent;state='skipped';reason='No supported installation'}); continue }
    for ($iteration=1; $iteration -le $Repeat; $iteration++) {
        $case=Join-Path $OutputDirectory "$agent-$iteration"
        $elapsed=[System.Diagnostics.Stopwatch]::StartNew()
        $entry=@{agent=$agent;iteration=$iteration;state='failed';workspace=$case}
        try {
            $script=if ($Web) { 'computer-use-web-acceptance.ps1' } else { 'computer-use-acceptance.ps1' }
            & (Join-Path $PSScriptRoot $script) -Agent $agent -Executable $binary -OutputDirectory $case
            if ($LASTEXITCODE -ne 0) { throw "Acceptance process exited $LASTEXITCODE" }
            $entry.state='passed'
        } catch { $entry.reason=$_.Exception.Message }
        $entry.elapsedMs=$elapsed.ElapsedMilliseconds
        $summary=Join-Path $case 'evidence/summary.json'
        if (Test-Path -LiteralPath $summary) { $entry.metrics=Get-Content -LiteralPath $summary -Raw | ConvertFrom-Json }
        $rows.Add($entry)
        [System.IO.File]::WriteAllText((Join-Path $OutputDirectory 'results.json'),($rows.ToArray() | ConvertTo-Json -Depth 40),[System.Text.UTF8Encoding]::new($false))
        Write-Output "$agent $iteration/$Repeat $($entry.state) $($entry.elapsedMs)ms"
        # A broken login/model route is not made valid by repeating a paid turn.
        if ($entry.state -eq 'failed') { break }
    }
}
$stats=@($rows | Group-Object agent | ForEach-Object {
    $cases=@($_.Group | Where-Object state -NE 'skipped')
    $passed=@($cases | Where-Object state -EQ 'passed').Count
    @{agent=$_.Name;attempts=$cases.Count;passed=$passed;successRate=if($cases.Count){$passed/$cases.Count}else{$null};complete=($cases.Count -eq $Repeat -and $passed -eq $Repeat)}
})
[System.IO.File]::WriteAllText((Join-Path $OutputDirectory 'summary.json'),($stats | ConvertTo-Json -Depth 6),[System.Text.UTF8Encoding]::new($false))
Write-Output "Evidence: $OutputDirectory"
if (@($stats | Where-Object { !$_.complete }).Count) { throw 'Some agents have not passed the requested repetitions; inspect results.json' }
