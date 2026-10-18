# Build and install the standalone Rust TSF text service (DLL only).
# Identity is separate from the C++ TIP and from every Go TIP, including the
# port-golang-windows-ime one this project was migrated from, so both can be
# installed side by side.
param(
    [switch]$Unregister,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
# Missing registry keys are expected while probing an input method that is not installed.
$PSNativeCommandUseErrorActionPreference = $false

function Assert-Amd64Pe([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        $stream.Position = 0x3C
        $pe = $reader.ReadInt32()
        $stream.Position = $pe + 4
        $machine = $reader.ReadUInt16()
        if ($machine -ne 0x8664) {
            throw "TIP DLL is not a 64-bit image (machine 0x$($machine.ToString('X4'))): $Path"
        }
    }
    finally { $stream.Dispose() }
}

$serviceName = "Rust Chinese Input Method"
$clsid = "{32200908-37C6-4149-AED4-664A78DB2082}"
$profile = "{89BD0007-8910-4365-A02A-4B975567138D}"
$cppClsid = "{7B2E4F5A-3A8E-4D74-9F0B-6D5D6F0E6C41}"
$cppProfile = "{C0A3B5B1-3C52-4E8E-A8A7-1F2B3D4C5E60}"
$otherClsids = @(
    $cppClsid,
    "{C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072}",  # port-golang-windows-ime
    "{5D8C1D62-9F73-4B4C-A3D2-1E7B8A9C6401}",
    "{A4C7D921-6B35-4E8A-91F2-7D0B6C3E5A14}"
)
$otherProfiles = @(
    $cppProfile,
    "{E731B5D4-2A90-4F68-AC13-7D5E9B2046F1}",  # port-golang-windows-ime
    "{D47E2A19-6C85-4D31-9B70-3F2A8C5E6102}",
    "{E8B1C4D7-2A63-49F0-B5D8-1C7E3A9F6240}"
)
if ($clsid -in $otherClsids -or $profile -in $otherProfiles) {
    throw "Identity collision with an existing TIP; refusing to continue."
}

$targetDir = Join-Path $PSScriptRoot "target\release"
$installDir = Join-Path $PSScriptRoot "install"
$builtDll = Join-Path $targetDir "port_rust_windows_ime.dll"
$builtHost = Join-Path $targetDir "registerhost.exe"
# Registering and unregistering use the copy in install\, so an uninstall does
# not depend on target\ still existing.
$registerHost = Join-Path $installDir "registerhost.exe"
$version = Get-Date -Format "yyyyMMdd-HHmmssfff"
# A DLL that a running application has loaded cannot be overwritten, so every
# install gets its own file name.
$dll = Join-Path $installDir "PortRustWindowsIME-$version.dll"

Write-Host "Input method: $serviceName"
Write-Host "CLSID:        $clsid"
Write-Host "Profile GUID: $profile"

if (-not $Unregister -and -not $SkipBuild) {
    Push-Location $PSScriptRoot
    try {
        # One build produces both the DLL and registerhost.exe.
        & cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE." }
    }
    finally { Pop-Location }
    if (-not (Test-Path -LiteralPath $builtDll)) { throw "Build did not produce $builtDll" }
    if (-not (Test-Path -LiteralPath $builtHost)) { throw "Build did not produce $builtHost" }
    Assert-Amd64Pe $builtDll
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item -LiteralPath $builtHost -Destination $registerHost -Force
}

$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "Administrator permission is required for TSF registration. Requesting elevation..."
    $log = Join-Path $PSScriptRoot "install-dll.log"
    if (Test-Path -LiteralPath $log) { Remove-Item -LiteralPath $log -Force }
    $arguments = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $PSCommandPath)
    $arguments += "-SkipBuild"
    if ($Unregister) { $arguments += "-Unregister" }
    $elevatedShell = if (Test-Path (Join-Path $PSHOME "pwsh.exe")) { Join-Path $PSHOME "pwsh.exe" } else { "powershell.exe" }
    try {
        $child = Start-Process -FilePath $elevatedShell -ArgumentList $arguments -Verb RunAs -Wait -PassThru
    }
    catch {
        throw "Administrator permission was not granted, so the input method was not registered. Approve the elevation prompt and run this script again."
    }
    if (-not $child -or $child.ExitCode -ne 0) {
        $detail = ""
        if (Test-Path -LiteralPath $log) { $detail = (Get-Content -LiteralPath $log -Raw) }
        $code = if ($child) { $child.ExitCode } else { "cancelled" }
        throw "Elevated registration failed with exit code $code. $detail"
    }
    exit 0
}

# A 32-bit host redirects System32 to SysWOW64. TSF loads the 64-bit DLL.
$system32 = if ([Environment]::Is64BitProcess) { Join-Path $env:WINDIR "System32" } else { Join-Path $env:WINDIR "Sysnative" }
$reg = Join-Path $system32 "reg.exe"
$cmd = Join-Path $system32 "cmd.exe"
$ctfmon = Join-Path $system32 "ctfmon.exe"
if (-not (Test-Path -LiteralPath $registerHost)) { throw "Registration host was not built: $registerHost. Run this script without -SkipBuild first." }
# The elevated host can be 32-bit. Force the 64-bit registry view used by TSF.
$reg64 = @("/reg:64")
$log = Join-Path $PSScriptRoot "install-dll.log"
$transcript = $false
try {
    Start-Transcript -LiteralPath $log -Force -ErrorAction Stop | Out-Null
    $transcript = $true
}
catch {}
try {

if ($Unregister) {
    $inprocKey = "HKLM\SOFTWARE\Classes\CLSID\$clsid\InprocServer32"
    $query = & $reg query $inprocKey /ve @reg64 2>$null
    if ($LASTEXITCODE -eq 0) {
        $line = @($query | Where-Object { $_ -match "REG_SZ" } | Select-Object -Last 1)
        $registered = ""
        if ($line) { $registered = ($line -replace "^.*REG_SZ\s+", "").Trim() }
        if ($registered -and (Test-Path -LiteralPath $registered)) {
            # registerhost calls the export directly and reports its raw HRESULT.
            $process = Start-Process -FilePath $registerHost -ArgumentList @("unregister", $registered) -Wait -PassThru -RedirectStandardError (Join-Path $PSScriptRoot "registerhost.log")
            if ($process.ExitCode -ne 0) { throw "DllUnregisterServer failed with exit code $($process.ExitCode) for $registered." }
        }
    }
    & $reg delete "HKLM\SOFTWARE\Classes\CLSID\$clsid" /f @reg64 2>$null | Out-Null
    & $reg delete "HKLM\SOFTWARE\Microsoft\CTF\TIP\$clsid" /f @reg64 2>$null | Out-Null
    & $reg delete "HKCU\Software\Microsoft\CTF\TIP\$clsid" /f @reg64 2>$null | Out-Null
    Write-Host "Unregistered $serviceName ($clsid)."
    exit 0
}

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$previousKey = "HKLM\SOFTWARE\Classes\CLSID\$clsid\InprocServer32"
$previousQuery = & $reg query $previousKey /ve @reg64 2>$null
if ($LASTEXITCODE -eq 0) {
    $previousLine = @($previousQuery | Where-Object { $_ -match "REG_SZ" } | Select-Object -Last 1)
    $previousDll = ""
    if ($previousLine) { $previousDll = ($previousLine -replace "^.*REG_SZ\s+", "").Trim() }
    if ($previousDll -like "*PortRustWindowsIME*.dll" -and (Test-Path -LiteralPath $previousDll)) {
        $removed = Start-Process -FilePath $registerHost -ArgumentList @("unregister", $previousDll) -Wait -PassThru -RedirectStandardError (Join-Path $PSScriptRoot "registerhost.log")
        if ($removed.ExitCode -ne 0) { throw "Could not replace the existing registration. DllUnregisterServer failed with exit code $($removed.ExitCode) for $previousDll." }
    }
}
Copy-Item -LiteralPath $builtDll -Destination $dll -Force
Assert-Amd64Pe $dll
$process = Start-Process -FilePath $registerHost -ArgumentList @("register", $dll) -Wait -PassThru -RedirectStandardError (Join-Path $PSScriptRoot "registerhost.log")
if ($process.ExitCode -ne 0) { throw "DllRegisterServer failed with exit code $($process.ExitCode) for $dll." }

$clsidKey = "HKLM\SOFTWARE\Classes\CLSID\$clsid"
$inprocKey = "$clsidKey\InprocServer32"
& $reg add $clsidKey /ve /t REG_SZ /d $serviceName /f @reg64 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Could not write COM class name: $clsidKey" }
& $reg add $inprocKey /ve /t REG_SZ /d $dll /f @reg64 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Could not write COM DLL path: $inprocKey" }
& $reg add $inprocKey /v ThreadingModel /t REG_SZ /d Apartment /f @reg64 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Could not write COM threading model: $inprocKey" }
$comQuery = & $reg query $inprocKey /ve @reg64 2>$null
if ($LASTEXITCODE -ne 0 -or -not (($comQuery -join "`n") -like "*$dll*")) {
    throw "COM registration verification failed. Expected: $dll"
}

$profileKey = "HKLM\SOFTWARE\Microsoft\CTF\TIP\$clsid\LanguageProfile\0x00000804\$profile"
& $reg query $profileKey @reg64 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) { throw "TSF registration verification failed. Language profile was not created: $profileKey" }
& $reg add $profileKey /v Enable /t REG_DWORD /d 1 /f @reg64 | Out-Null

$userZhProfileKey = "HKCU\Software\Microsoft\CTF\TIP\$clsid\LanguageProfile\0x00000804\$profile"
& $reg add $userZhProfileKey /v Enable /t REG_DWORD /d 1 /f @reg64 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Per-user zh-CN TSF profile enable failed: $userZhProfileKey" }

$cppKey = "HKLM\SOFTWARE\Classes\CLSID\$cppClsid\InprocServer32"
$cppQuery = & $reg query $cppKey /ve @reg64 2>$null
if ($LASTEXITCODE -eq 0 -and (($cppQuery -join "`n") -like "*$dll*")) {
    throw "C++ TIP registration was overwritten. Refusing to continue."
}

Write-Host "Installed and verified:"
Write-Host "  Name:    $serviceName"
Write-Host "  CLSID:   $clsid"
Write-Host "  Profile: $profile"
Write-Host "  DLL:     $dll"
if (-not (Test-Path $ctfmon)) { throw "ctfmon.exe was not found at '$ctfmon'." }
# Start-Process cannot launch a Sysnative path from a 32-bit host. cmd.exe can.
& $cmd /c "taskkill /F /IM ctfmon.exe >nul 2>&1 & start `"`" /D C:\Windows C:\Windows\System32\ctfmon.exe"
if (-not (Get-Process ctfmon -ErrorAction SilentlyContinue)) { throw "ctfmon.exe did not stay running." }
Write-Host "Restart the target application and select '$serviceName'."
}
catch {
    Write-Host $_.Exception.Message
    Write-Host $_.ScriptStackTrace
    if (-not $transcript) { Add-Content -LiteralPath $log -Value $_.Exception.Message }
    exit 1
}
finally {
    if ($transcript) {
        try { Stop-Transcript | Out-Null } catch {}
    }
}
