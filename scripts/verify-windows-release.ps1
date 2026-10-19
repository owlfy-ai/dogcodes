param(
    [string]$Installer,
    [string]$CertificateThumbprint,
    [switch]$CheckTools
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT') { throw 'Verify Windows packages on Windows.' }
Import-Module (Join-Path $PSHOME 'Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
$root = Split-Path $PSScriptRoot -Parent
$archiveTool = $env:SEVEN_ZIP_PATH
if (-not $archiveTool) {
    $command = Get-Command 7z.exe -ErrorAction SilentlyContinue
    if ($command) { $archiveTool = $command.Source }
    else { $archiveTool = Join-Path $env:ProgramFiles '7-Zip/7z.exe' }
}
if (-not (Test-Path -LiteralPath $archiveTool -PathType Leaf)) {
    throw 'Install full 7-Zip or set SEVEN_ZIP_PATH to 7z.exe. It is needed to verify the executable inside the NSIS installer.'
}
if ($CheckTools) { Write-Host 'Installer verification tools available.'; return }
if (-not $Installer -or $CertificateThumbprint -notmatch '^[0-9A-Fa-f]{40}$') { throw 'Installer and certificate thumbprint are required.' }
function Assert-Signed([string]$File) {
    $signature = Get-AuthenticodeSignature -LiteralPath $File
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Thumbprint -ne $CertificateThumbprint -or -not $signature.TimeStamperCertificate) {
        throw "Invalid, missing or unexpected signature/timestamp: $File"
    }
}
Assert-Signed $Installer
$stage = Join-Path $root ('release/private/verify-windows-' + [guid]::NewGuid().ToString('N'))
try {
    & $archiveTool x -y "-o$stage" $Installer | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Could not extract the NSIS installer.' }
    # Cargo's application binary is desktop.exe; Tauri restores the unsigned
    # build-directory copy after bundling. Verify the copy users actually install.
    $executables = @(Get-ChildItem -LiteralPath $stage -Recurse -File -Filter 'desktop.exe')
    if ($executables.Count -ne 1) { throw 'Expected one desktop.exe inside the installer.' }
    Assert-Signed $executables[0].FullName
    Write-Host 'Installer and packaged application signatures and timestamps verified.'
} finally {
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
