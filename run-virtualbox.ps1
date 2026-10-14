<#
.SYNOPSIS
  Boots the TriOS UEFI disk image in VirtualBox (headless by default, or with
  a visible window via -Gui) and captures the COM1 serial console to serial.log.

.DESCRIPTION
  All VirtualBox side-effects (VM config, machine home) are kept inside the
  repository via VBOX_USER_HOME=<repo>\.vbox-home, so nothing leaks to the
  user profile. The VM is recreated from scratch on every run for a
  deterministic boot; delete with -KeepVm to inspect it afterwards.

  Modes:
    * Default (headless): boots for -Seconds (15 s), powers off, prints
      serial.log, removes the VM. CI-style, non-interactive.
    * -Gui: boots with a *visible* VirtualBox window for visual UAT (boot
      splash, idle screen). The script waits until you close the VM window.
      -Gui implies -KeepVm so you can inspect or reboot the VM afterwards.
      serial.log is still captured and printed once the VM exits.
      If -Seconds is passed *explicitly* alongside -Gui, it acts as a
      timeout: the VM is powered off after that many seconds.

  Requirements:
    * VirtualBox with VBoxManage on PATH, or set $env:VBOXMANAGE,
      or install to the default D:\virtualbox\
    * a built image: cargo build --release (target\release\trios-uefi-release.img)
      or cargo build (target\debug\trios-uefi.img)

  Input devices:
    The VM is configured with --mouse ps2 --keyboard ps2. TriOS only
    implements PS/2 (ports 0x60/0x64, IRQ1 + IRQ12); VirtualBox 7 defaults
    OSType=Other_64 to USB Tablet/USB Keyboard (HID), which the guest cannot
    drive. Forcing PS/2 makes host input reach the kernel's PS/2 drivers.
    PS/2 mouse always uses capture mode: click inside the VM window once to
    hide the host cursor and let the guest receive relative motion packets
    via IRQ12. (There is no 'mouseintegration' VBoxManage subcommand — an
    earlier attempt to run 'controlvm mouseintegration off' failed silently.)

.EXAMPLE
  .\run-virtualbox.ps1
  .\run-virtualbox.ps1 -Seconds 20 -Image target\debug\trios-uefi.img
  .\run-virtualbox.ps1 -Gui
  .\run-virtualbox.ps1 -Gui -Seconds 30
#>
[CmdletBinding()]
param(
    # Disk image to boot (defaults to release, then debug build).
    [string]$Image = "",
    # How long the VM runs before it is powered off.
    # Headless: always honoured. With -Gui: only honoured when passed
    # explicitly (acts as a timeout); otherwise the VM runs until closed.
    [int]$Seconds = 15,
    # Name of the throw-away VM.
    [string]$VmName = "TriOS",
    # Keep the VM registered after the run (for interactive debugging).
    # Implied by -Gui.
    [switch]$KeepVm,
    # Boot with a visible VirtualBox window (interactive visual UAT)
    # instead of headless. The script waits until the VM window is closed.
    [switch]$Gui
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot

# --- locate VBoxManage -------------------------------------------------------
$VBoxManage = $env:VBOXMANAGE
if (-not $VBoxManage) {
    $candidate = Get-Command VBoxManage.exe -ErrorAction SilentlyContinue
    if ($candidate) { $VBoxManage = $candidate.Source }
}
if (-not $VBoxManage -and (Test-Path "D:\virtualbox\VBoxManage.exe")) {
    $VBoxManage = "D:\virtualbox\VBoxManage.exe"
}
if (-not $VBoxManage -or -not (Test-Path $VBoxManage)) {
    throw "VBoxManage.exe not found. Install VirtualBox or set `$env:VBOXMANAGE."
}

# --- locate the disk image ---------------------------------------------------
if (-not $Image) {
    $release = Join-Path $Root "target\release\trios-uefi-release.img"
    $debug = Join-Path $Root "target\debug\trios-uefi.img"
    if (Test-Path $release) { $Image = $release } elseif (Test-Path $debug) { $Image = $debug }
}
if (-not (Test-Path $Image)) {
    throw "Disk image not found ($Image). Run 'cargo build' or 'cargo build --release' first."
}
$Image = (Resolve-Path $Image).Path

# --- keep every VBox artefact inside the repo --------------------------------
$env:VBOX_USER_HOME = Join-Path $Root ".vbox-home"
$SerialLog = Join-Path $Root "serial.log"
$Vmdk = Join-Path $Root "trios-uefi-test.vmdk"

Write-Host "image    : $Image"
Write-Host "serial   : $SerialLog"
Write-Host "vbox home: $env:VBOX_USER_HOME"

# --- fresh start -------------------------------------------------------------
if (Test-Path $SerialLog) { Remove-Item $SerialLog -Force }

# Remove any previous test VM (expected to fail when none exists — the
# redirected stderr would otherwise trip $ErrorActionPreference='Stop').
try { & $VBoxManage controlvm $VmName poweroff 2>$null | Out-Null } catch {}
try { & $VBoxManage unregistervm $VmName --delete 2>$null | Out-Null } catch {}

# Convert the raw image to a VMDK VirtualBox can attach.
if (Test-Path $Vmdk) { Remove-Item $Vmdk -Force }
& $VBoxManage convertfromraw $Image $Vmdk --format VMDK
if ($LASTEXITCODE -ne 0) { throw "convertfromraw failed" }

# --- create + configure the VM ----------------------------------------------
# --basefolder keeps the machine files inside the repo (VBOX_USER_HOME alone
# still defaults new VMs to %USERPROFILE%\VirtualBox VMs on Windows).
$MachineFolder = Join-Path $env:VBOX_USER_HOME "Machines"
New-Item -ItemType Directory -Force -Path $MachineFolder | Out-Null
& $VBoxManage createvm --name $VmName --ostype Other_64 --basefolder $MachineFolder --register
if ($LASTEXITCODE -ne 0) { throw "createvm failed" }

# --mouse/--keyboard ps2: TriOS only implements PS/2 (ports 0x60/0x64, IRQ1 +
# IRQ12). VirtualBox 7 for OSType=Other_64 defaults to USB Tablet (HID) +
# USB keyboard, so without these flags the guest gets no mouse packets and
# only stray keyboard scancodes.
& $VBoxManage modifyvm $VmName `
    --firmware efi `
    --memory 256 `
    --vram 16 `
    --boot1 disk --boot2 none --boot3 none --boot4 none `
    --nic1 none `
    --mouse ps2 --keyboard ps2 `
    --audio-out off --audio-in off `
    --uart1 0x3F8 4 `
    --uartmode1 file $SerialLog
if ($LASTEXITCODE -ne 0) { throw "modifyvm failed" }

& $VBoxManage storagectl $VmName --name SATA --add sata --controller IntelAhci --portcount 1
if ($LASTEXITCODE -ne 0) { throw "storagectl failed" }

& $VBoxManage storageattach $VmName --storagectl SATA --port 0 --device 0 --type hdd --medium $Vmdk
if ($LASTEXITCODE -ne 0) { throw "storageattach failed" }

# --- boot + wait --------------------------------------------------------------
if ($Gui) {
    # Interactive mode: visible window, serial still goes to the uart file.
    # Only treat -Seconds as a timeout when the caller passed it explicitly;
    # otherwise stay in the wait loop until the user closes the VM window.
    $GuiTimeout = $PSBoundParameters.ContainsKey("Seconds")
    if ($GuiTimeout) {
        Write-Host "booting with GUI, timeout ${Seconds}s ..."
    } else {
        Write-Host "booting with GUI; close the VM window when finished ..."
    }
    & $VBoxManage startvm $VmName --type gui
    if ($LASTEXITCODE -ne 0) { throw "startvm failed" }

    # NOTE: 'controlvm mouseintegration off' was REMOVED — it is NOT a valid
    # VBoxManage 7.2 subcommand (only keyboardputscancode, poweroff, etc.
    # exist). It failed silently inside try/catch on every run. With
    # --mouse=ps2 (set in modifyvm above), mouse integration does not apply
    # (that concept is for USB-tablet devices). PS/2 always uses capture
    # mode: the user clicks inside the VM window once -> host cursor hides
    # -> the guest receives relative motion + button packets via IRQ12.
    Write-Host "GUI mode: click inside the VM window once to capture mouse/keyboard."

    $Start = Get-Date
    while ($true) {
        # Poll the machine state until the VM has stopped (user closed the
        # window, powered off, aborted or saved it).
        $State = ""
        foreach ($Line in (& $VBoxManage showvminfo $VmName --machinereadable)) {
            if ($Line -like "VMState=*") {
                $State = $Line -replace '^VMState="', "" -replace '"$', ""
                break
            }
        }
        if ($State -in @("poweroff", "aborted", "saved")) { break }
        if ($GuiTimeout -and ((Get-Date) - $Start).TotalSeconds -ge $Seconds) {
            Write-Host "timeout after ${Seconds}s; powering off ..."
            & $VBoxManage controlvm $VmName poweroff
            break
        }
        Start-Sleep -Milliseconds 500
    }
} else {
    Write-Host "booting headless for $Seconds s ..."
    & $VBoxManage startvm $VmName --type headless
    if ($LASTEXITCODE -ne 0) { throw "startvm failed" }

    # (headless: no phantom 'controlvm mouseintegration off' — not a valid
    # subcommand. See the note in the GUI branch above.)

    Start-Sleep -Seconds $Seconds

    & $VBoxManage controlvm $VmName poweroff
}
Start-Sleep -Milliseconds 1500   # let VBox flush the serial file

# --- report ------------------------------------------------------------------
if (Test-Path $SerialLog) {
    $bytes = (Get-Item $SerialLog).Length
    Write-Host "`n--- serial.log ($bytes bytes) ---"
    Get-Content $SerialLog
    Write-Host "--- end of serial.log ---"
} else {
    Write-Warning "serial.log was not created (guest never reached the UART, or UEFI did not boot the image)."
}

if ($Gui) {
    # -Gui implies -KeepVm: leave the VM registered so it can be inspected
    # or rebooted from the VirtualBox Manager.
    Write-Host "keeping VM '$VmName' registered (Gui mode)."
} elseif (-not $KeepVm) {
    try { & $VBoxManage unregistervm $VmName --delete 2>$null | Out-Null } catch {}
}
