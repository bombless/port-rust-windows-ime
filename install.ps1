# Registers the x64 Rust TSF input method DLL.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateNotNullOrEmpty()]
    [string] $DllPath
)

$ErrorActionPreference = 'Stop'
$clsid = '{C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072}'

try {
    $resolvedPath = (Resolve-Path -LiteralPath $DllPath).Path
} catch {
    throw "DLL file not found: $DllPath"
}
if (-not (Test-Path -LiteralPath $resolvedPath -PathType Leaf)) {
    throw "DLL path is not a file: $resolvedPath"
}
if ([IO.Path]::GetExtension($resolvedPath) -ine '.dll') {
    throw "Expected a .dll file: $resolvedPath"
}
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'This installer currently supports 64-bit Windows only.'
}

# Use the native x64 regsvr32 even when launched from 32-bit PowerShell.
if (-not [Environment]::Is64BitProcess) {
    $regsvr32 = Join-Path $env:WINDIR 'Sysnative\regsvr32.exe'
} else {
    $regsvr32 = Join-Path $env:WINDIR 'System32\regsvr32.exe'
}
if (-not (Test-Path -LiteralPath $regsvr32 -PathType Leaf)) {
    throw "Could not find the native x64 regsvr32.exe: $regsvr32"
}

$principal = New-Object Security.Principal.WindowsPrincipal(
    [Security.Principal.WindowsIdentity]::GetCurrent()
)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Administrator privileges are required. Re-run PowerShell as Administrator.'
}

Write-Host "Registering TSF input method DLL: $resolvedPath"
$process = Start-Process -FilePath $regsvr32 -ArgumentList @('/s', ('"{0}"' -f $resolvedPath)) -Wait -PassThru
if ($process.ExitCode -ne 0) {
    throw "regsvr32 failed with exit code $($process.ExitCode). The DLL registration routine returned an error."
}

# Verify the COM in-process server points to the requested DLL.
$registryPath = "Registry::HKEY_CLASSES_ROOT\CLSID\$clsid\InprocServer32"
try {
    $key = Get-Item -LiteralPath $registryPath
} catch {
    throw "regsvr32 returned success, but the COM registration key was not found: $registryPath"
}
$registeredPath = [string] $key.GetValue('')
if ([string]::IsNullOrWhiteSpace($registeredPath)) {
    throw "COM registration exists but its DLL path is empty: $registryPath"
}
if (-not [string]::Equals(
    [IO.Path]::GetFullPath($registeredPath),
    [IO.Path]::GetFullPath($resolvedPath),
    [StringComparison]::OrdinalIgnoreCase
)) {
    throw "COM registration path mismatch. Expected '$resolvedPath', found '$registeredPath'."
}
$threadingModel = [string] $key.GetValue('ThreadingModel')
if ($threadingModel -ne 'Apartment') {
    throw "Unexpected COM ThreadingModel '$threadingModel'; expected 'Apartment'."
}

Write-Host 'COM DLL registration verified.' -ForegroundColor Green
Write-Host 'If the profile does not appear immediately, sign out and back in or restart Windows.'
