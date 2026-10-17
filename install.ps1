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

# This artifact is built for x64 MSVC. Use the native 64-bit regsvr32 even
# when this script was started from a 32-bit PowerShell process on 64-bit Windows.
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'This installer currently supports 64-bit Windows only.'
}
if (-not [Environment]::Is64BitProcess -and $env:WINDIR) {
    $regsvr32 = Join-Path $env:WINDIR 'Sysnative\regsvr32.exe'
} else {
    $regsvr32 = Join-Path $env:WINDIR 'System32\regsvr32.exe'
}
if (-not (Test-Path -LiteralPath $regsvr32 -PathType Leaf)) {
    throw "Could not find the native x64 regsvr32.exe: $regsvr32"
}

# The DLL writes COM and TSF registration under HKEY_CLASSES_ROOT.
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

# Confirm the COM in-process server points to the DLL just registered.
$registryPath = "Registry::HKEY_CLASSES_ROOT\CLSID\$clsid\InprocServer32"
try {
    $registration = Get-ItemProperty -LiteralPath $registryPath
} catch {
    throw "regsvr32 returned success, but the COM registration key was not found: $registryPath"
}
$registeredPath = [string] $registration.'(default)'
if ([string]::IsNullOrWhiteSpace($registeredPath)) {
    # PowerShell registry provider exposes the unnamed value via GetValue().
    $registeredPath = [string] (Get-Item -LiteralPath $registryPath).GetValue('')
}
if (-not [string]::Equals(
    [IO.Path]::GetFullPath($registeredPath),
    [IO.Path]::GetFullPath($resolvedPath),
    [StringComparison]::OrdinalIgnoreCase
)) {
    throw "COM registration path mismatch. Expected '$resolvedPath', found '$registeredPath'."
}

$threadingModel = [string] (Get-Item -LiteralPath $registryPath).GetValue('ThreadingModel')
if ($threadingModel -ne 'Apartment') {
    throw "Unexpected COM ThreadingModel '$threadingModel'; expected 'Apartment'."
}

Write-Host 'COM DLL registration verified.' -ForegroundColor Green
Write-Host 'The TSF language profile should now be available after Windows refresh/sign-out or restart if necessary.'
Write-Warning 'Registration does not prove that TSF activation or text input works; validate on a real Windows host.'
