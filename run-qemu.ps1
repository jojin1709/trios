<#
.SYNOPSIS
  Boots the TriOS UEFI disk image in QEMU with OVMF firmware and captures
  the COM1 serial console to serial-qemu.log.

.DESCRIPTION
  Requires:
    * QEMU (qemu-system-x86_64) on PATH, or set $env:QEMU
    * OVMF firmware (ships with QEMU on most platforms; on Windows install
      via `pacman -S edk2-ovmf` in MSYS2 or download from the OVMF repo and
      point $env:OVMF_CODE / $env:OVMF_VARS at the .fd files).
    * a built image: cargo build --release (target\release\trios-uefi-release.img)
      or cargo build (target\debug\trios-uefi.img)

  NOTE: This script is provided for completeness but has NOT been verified on
  this machine (QEMU is not installed here). The verified boot path is
  run-virtualbox.ps1. Report any QEMU-specific issues in the project tracker.

.EXAMPLE
  .\run-qemu.ps1
  .\run-qemu.ps1 -Seconds 20 -Image target\debug\trios-uefi.img
#>
[CmdletBinding()]
param(
    # Disk image to boot (defaults to release, then debug build).
    [string]$Image = "",
    # How long QEMU runs before it is killed.
    [int]$Seconds = 15,
    # Path to OVMF_CODE.fd (auto-detected if omitted).
    [string]$OvmfCode = "",
    # Path to OVMF_VARS.fd (auto-detected if omitted; a copy is used).
    [string]$OvmfVars = ""
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot

# --- locate QEMU --------------------------------------------------------------
$Qemu = $env:QEMU
if (-not $Qemu) {
    $candidate = Get-Command qemu-system-x86_64.exe -ErrorAction SilentlyContinue
    if ($candidate) { $Qemu = $candidate.Source }
}
if (-not $Qemu) {
    throw "qemu-system-x86_64 not found. Install QEMU or set `$env:QEMU."
}

# --- locate OVMF --------------------------------------------------------------
$ovmfDirs = @()
if ($env:OVMF_CODE -and (Test-Path $env:OVMF_CODE)) {
    $OvmfCode = $env:OVMF_CODE
    if ($env:OVMF_VARS -and (Test-Path $env:OVMF_VARS)) { $OvmfVars = $env:OVMF_VARS }
}
if (-not $OvmfCode) {
    # Common locations on Windows (MSYS2, Fedora cross-builds, manual installs).
    $ovmfDirs = @(
        "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
        "C:\Program Files\qemu\share\edk2-x86_64-vars.fd",
        "D:\qemu\share\edk2-x86_64-code.fd",
        "D:\qemu\share\edk2-x86_64-vars.fd",
        "C:\msys64\usr\share\edk2\x64\OVMF_CODE.fd",
        "C:\msys64\usr\share\edk2\x64\OVMF_VARS.fd",
        "C:\msys64\usr\share\edk2\OVMF_CODE.fd",
        "C:\msys64\usr\share\edk2\OVMF_VARS.fd"
    )
    foreach ($dir in $ovmfDirs) {
        if (Test-Path $dir) {
            if (-not $OvmfCode -and $dir -match 'CODE') { $OvmfCode = $dir }
            if (-not $OvmfVars -and $dir -match 'VARS') { $OvmfVars = $dir }
        }
    }
}
if (-not $OvmfCode) {
    throw "OVMF firmware not found. Install OVMF / edk2-ovmf or set `$env:OVMF_CODE."
}

# --- locate the disk image ----------------------------------------------------
if (-not $Image) {
    $release = Join-Path $Root "target\release\trios-uefi-release.img"
    $debug = Join-Path $Root "target\debug\trios-uefi.img"
    if (Test-Path $release) { $Image = $release } elseif (Test-Path $debug) { $Image = $debug }
}
if (-not (Test-Path $Image)) {
    throw "Disk image not found ($Image). Run 'cargo build' or 'cargo build --release' first."
}
$Image = (Resolve-Path $Image).Path

# --- serial log ---------------------------------------------------------------
$SerialLog = Join-Path $Root "serial-qemu.log"
if (Test-Path $SerialLog) { Remove-Item $SerialLog -Force }

Write-Host "image    : $Image"
Write-Host "serial   : $SerialLog"
Write-Host "ovmf code: $OvmfCode"
if ($OvmfVars) { Write-Host "ovmf vars: $OvmfVars" }

# --- build the QEMU command ---------------------------------------------------
$QemuArgs = @(
    "-machine", "q35",
    "-m", "256M",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`""
)
if ($OvmfVars) {
    # QEMU needs a writable copy of the VARS store; use a scratch copy.
    $VarsCopy = Join-Path $Root "ovmf-vars-copy.fd"
    Copy-Item $OvmfVars $VarsCopy -Force
    $QemuArgs += @("-drive", "if=pflash,format=raw,file=`"$VarsCopy`"")
}
$QemuArgs += @(
    "-drive", "format=raw,file=`"$Image`"",
    "-nic", "none"
)

Write-Host "`nbooting QEMU for $Seconds s ..."
Write-Host "cmdline : qemu-system-x86_64 $($QemuArgs -join ' ')"

# --- launch, wait, kill -------------------------------------------------------
$proc = Start-Process -FilePath $Qemu -ArgumentList $QemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $Seconds
if (-not $proc.HasExited) { $proc.Kill() }
$proc.WaitForExit()

# --- report -------------------------------------------------------------------
if (Test-Path $SerialLog) {
    $bytes = (Get-Item $SerialLog).Length
    Write-Host "`n--- serial-qemu.log ($bytes bytes) ---"
    Get-Content $SerialLog
    Write-Host "--- end of serial-qemu.log ---"
} else {
    Write-Warning "serial-qemu.log was not created (guest never reached the UART, or UEFI did not boot the image)."
}

# Clean up the VARS scratch copy.
$VarsCopy = Join-Path $Root "ovmf-vars-copy.fd"
if (Test-Path $VarsCopy) { Remove-Item $VarsCopy -Force }
