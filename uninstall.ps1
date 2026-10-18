# Uninstall the standalone Rust text service registered by install-dll.ps1.
# This intentionally removes only this project's CLSID/profile and versioned DLLs.
[CmdletBinding()]
param(
    [switch]$KeepFiles
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
$installScript = Join-Path $PSScriptRoot "install-dll.ps1"
if (-not (Test-Path -LiteralPath $installScript)) {
    throw "Cannot find installer: $installScript"
}

# Reuse the installer's tested DllUnregisterServer and TSF registry cleanup.
# -SkipBuild is always passed: uninstalling must not need a compiler.
$arguments = @(
    "-NoProfile",
    "-ExecutionPolicy", "Bypass",
    "-File", $installScript,
    "-Unregister",
    "-SkipBuild"
)
$shell = if (Test-Path (Join-Path $PSHOME "pwsh.exe")) {
    Join-Path $PSHOME "pwsh.exe"
} else {
    Join-Path $env:WINDIR "System32\WindowsPowerShell\v1.0\powershell.exe"
}
$process = Start-Process -FilePath $shell -ArgumentList $arguments -Wait -PassThru
if ($process.ExitCode -ne 0) {
    throw "TSF unregistration failed with exit code $($process.ExitCode). Installed files were left in place."
}

if (-not $KeepFiles) {
    $installDir = Join-Path $PSScriptRoot "install"
    if (Test-Path -LiteralPath $installDir) {
        # Only remove this TIP's versioned DLLs and its registration helper.
        Get-ChildItem -LiteralPath $installDir -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^PortRustWindowsIME(?:-[0-9]{8}-[0-9]{9})?\.dll$' -or $_.Name -eq 'registerhost.exe' } |
            Remove-Item -Force
    }
    Write-Host "Unregistered Rust Chinese Input Method and removed its installed DLL files."
} else {
    Write-Host "Unregistered Rust Chinese Input Method. Installed files were kept."
}
Write-Host "If the input method remains visible in the language switcher, sign out and sign back in."
