param([Parameter(Mandatory = $true)][string]$CertificateThumbprint)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT') { throw 'Check the signing certificate on the Windows signing machine.' }
Import-Module (Join-Path $PSHOME 'Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
$thumbprint = $CertificateThumbprint.Trim().ToUpperInvariant()
if ($thumbprint -notmatch '^[0-9A-F]{40}$') { throw 'Invalid signing certificate thumbprint.' }
$certificate = Get-Item "Cert:\CurrentUser\My\$thumbprint" -ErrorAction SilentlyContinue
if (-not $certificate -or -not $certificate.HasPrivateKey) {
    throw 'Connect and log in to SimplySign Desktop. The certificate and its private-key provider must be available in CurrentUser\My.'
}
$now = Get-Date
if ($certificate.NotBefore -gt $now -or $certificate.NotAfter -le $now) { throw 'The signing certificate is not currently valid.' }
$eku = $certificate.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.37' }
if (-not $eku -or -not ($eku.EnhancedKeyUsages | Where-Object { $_.Value -eq '1.3.6.1.5.5.7.3.3' })) { throw 'The certificate does not allow code signing.' }
$sdkRoot = ${env:ProgramFiles(x86)}
$tools = @(Get-ChildItem "$sdkRoot\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue)
if ($tools.Count -eq 0 -and -not (Get-Command signtool.exe -ErrorAction SilentlyContinue)) { throw 'Install Windows SDK SignTool before packaging.' }
Write-Host "Certificate available: $($certificate.Subject); expires $($certificate.NotAfter.ToString('yyyy-MM-dd'))."
Write-Host 'This checks certificate availability only. Actual signing may still require SimplySign login or PIN.'
