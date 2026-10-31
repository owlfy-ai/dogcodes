param([ValidateSet('codex','deepseek-harness','claude-code')][string]$Agent='codex', [string]$OutputDirectory)
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$names=@('CalculatorApp','Calculator')
$caseRoot=if ($OutputDirectory) { [System.IO.Path]::GetFullPath($OutputDirectory) } else { Join-Path $root ('.local/computer-use/acceptance/calculator-'+$Agent+'-'+[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()) }
[System.IO.Directory]::CreateDirectory($caseRoot) | Out-Null
$runner=Join-Path $root 'target/debug/examples/live_computer_use.exe'
$installed=(& $runner --discover | Out-String | ConvertFrom-Json)
$binary=($installed | Where-Object provider -EQ $Agent).installations | Where-Object status -EQ 'installed' | Select-Object -First 1 -ExpandProperty executable
if (!$binary) { throw "No installed $Agent found" }
$prepared=& node (Join-Path $PSScriptRoot 'prepare-computer-use.mjs')
if ($LASTEXITCODE -ne 0) { throw 'Cua preparation failed' }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
function CalculatorWindows {
    foreach ($process in @(Get-Process -Name $names -ErrorAction SilentlyContinue)) {
        if ($process.MainWindowHandle -ne 0) {
            $window=[System.Windows.Automation.AutomationElement]::FromHandle($process.MainWindowHandle)
            [pscustomobject]@{element=$window;pid=$process.Id;windowId=$process.MainWindowHandle.ToInt64()}
        }
    }
}
$existing=@(CalculatorWindows | Select-Object -ExpandProperty windowId)
$existingProcesses=@(Get-Process -Name $names -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
$startedProcesses=@()
$owned=$null
try {
    Start-Process -FilePath (Join-Path $env:WINDIR 'System32/calc.exe') | Out-Null
    $deadline=[DateTimeOffset]::UtcNow.AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 200
        $startedProcesses=@(Get-Process -Name $names -ErrorAction SilentlyContinue | Where-Object { $existingProcesses -notcontains $_.Id })
        $owned=CalculatorWindows | Where-Object { $existing -notcontains $_.windowId } | Select-Object -First 1
    } while (!$owned -and [DateTimeOffset]::UtcNow -lt $deadline)
    if (!$owned) { throw 'Calculator did not create a separate window; existing user windows were not changed' }
    $prompt=@"
Operate ONLY the newly opened Windows Calculator window_id $($owned.windowId), pid $($owned.pid), using dogcodes computer-use tools. Discover it with list_windows filtered by that pid, and select exactly that window_id, then inspect a real screenshot. Other Calculator windows belong to the user and must not be operated.
Calculate 19 + 23 using the visible calculator controls. Use fresh screenshot-grounded window-local x/y coordinates for at least one button click, to verify visual input with this UWP/MSIX application. Foreground delivery is permitted. Verify the final visible result is 42 and save a final window screenshot to '$caseRoot/calculator.png' with screenshot_out_file.
Do not use shell, scripts, filesystem writes or another application to calculate or change its state. Report what the Calculator actually displays.
"@
    $promptFile=Join-Path $caseRoot 'prompt.txt'
    [System.IO.File]::WriteAllText($promptFile,$prompt,[System.Text.UTF8Encoding]::new($false))
    & $runner $Agent $binary (Join-Path $prepared 'cua-driver.exe') $caseRoot $promptFile 2>&1 | Tee-Object -FilePath (Join-Path $caseRoot 'run.log')
    if ($LASTEXITCODE -ne 0) { throw "Agent runner failed: $caseRoot" }
    $display=$null
    $result=$owned.element.FindFirst([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::AutomationIdProperty,'CalculatorResults'))
    if ($result) { $display=$result.Current.Name }
    if (!$display -or $display -notmatch '(?<!\d)42(?!\d)') { throw "Independent UI Automation verification failed: $display" }
    if (!(Test-Path -LiteralPath (Join-Path $caseRoot 'calculator.png'))) { throw 'Final Calculator screenshot is missing' }
    [System.IO.File]::WriteAllText((Join-Path $caseRoot 'verification.json'),(@{display=$display;pid=$owned.pid;windowId=$owned.windowId;independentObserver='Windows UI Automation';passed=$true} | ConvertTo-Json),[System.Text.UTF8Encoding]::new($false))
    Write-Output "PASS $caseRoot"
} catch {
    [System.IO.File]::WriteAllText((Join-Path $caseRoot 'verification.json'),(@{passed=$false;reason=$_.Exception.Message} | ConvertTo-Json),[System.Text.UTF8Encoding]::new($false))
    throw
} finally {
    # A new window can share a process with the user's existing calculators.
    # Close only this original UIA window, never the entire Calculator process.
    if ($owned) { try { $owned.element.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern).Close() } catch {} }
    foreach ($process in $startedProcesses) {
        $process.Refresh()
        if (!$process.HasExited -and $process.MainWindowHandle -eq 0) { Stop-Process -InputObject $process -ErrorAction SilentlyContinue }
    }
}
