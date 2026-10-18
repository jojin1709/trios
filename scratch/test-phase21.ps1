# Automated CLI VirtualBox Test for TriOS Phase 21 (Ring 3 Real Processes, Syscalls, Isolation & ps/kill)
$ErrorActionPreference = "Stop"
$Root = "D:\trios"
$VBoxManage = "D:\virtualbox\VBoxManage.exe"
$VmName = "TriOS"
$SerialLog = Join-Path $Root "serial.log"
$Screenshot = Join-Path $Root "phase21-screenshot.png"
$env:VBOX_USER_HOME = Join-Path $Root ".vbox-run"

Write-Host "=== TriOS Phase 21 CLI VirtualBox Test (Ring 3 Processes & Fault Isolation) ==="

# 1. Clean previous VM if running
Stop-Process -Name VBoxSVC, VirtualBoxVM, VirtualBox -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
try { & $VBoxManage controlvm $VmName poweroff 2>$null | Out-Null } catch {}
try { & $VBoxManage unregistervm $VmName --delete 2>$null | Out-Null } catch {}

# 2. Convert VMDK
$Image = Join-Path $Root "target\release\trios-uefi-release.img"
$Vmdk = Join-Path $Root "trios-uefi-test.vmdk"
if (Test-Path $Vmdk) { Remove-Item $Vmdk -Force }
& $VBoxManage convertfromraw $Image $Vmdk --format VMDK | Out-Null

# 3. Create or register VM
$MachineFolder = Join-Path $env:VBOX_USER_HOME "Machines"
$MachineDir = Join-Path $MachineFolder $VmName
$VboxFile = Join-Path $MachineDir "$VmName.vbox"

$vmList = (& $VBoxManage list vms) -join "`n"
if ($vmList -notmatch "`"$VmName`"") {
    if (Test-Path $VboxFile) {
        & $VBoxManage registervm $VboxFile | Out-Null
    } else {
        New-Item -ItemType Directory -Force -Path $MachineFolder | Out-Null
        & $VBoxManage createvm --name $VmName --ostype Other_64 --basefolder $MachineFolder --register | Out-Null
    }
}

& $VBoxManage modifyvm $VmName `
    --firmware efi `
    --memory 512 `
    --vram 128 `
    --boot1 disk --boot2 none --boot3 none --boot4 none `
    --nic1 nat --nic-type1 82540EM `
    --mouse ps2 --keyboard ps2 `
    --audio-out off --audio-in off `
    --uart1 0x3F8 4 `
    --uartmode1 file $SerialLog | Out-Null
& $VBoxManage setextradata $VmName "VBoxInternal2/EfiGraphicsResolution" "1920x1080" | Out-Null
try { & $VBoxManage storagectl $VmName --name SATA --add sata --controller IntelAhci --portcount 1 2>$null | Out-Null } catch {}
& $VBoxManage storageattach $VmName --storagectl SATA --port 0 --device 0 --type hdd --medium $Vmdk | Out-Null

# 4. Start VM headless
if (Test-Path $SerialLog) { Remove-Item $SerialLog -Force }
if (Test-Path $Screenshot) { Remove-Item $Screenshot -Force }
Write-Host "Starting VM headless with 1920x1080 GOP resolution..."
& $VBoxManage startvm $VmName --type headless | Out-Null

# 5. Wait for desktop ready
Write-Host "Waiting for TriOS desktop to boot..."
$booted = $false
$sw = [System.Diagnostics.Stopwatch]::StartNew()
while ($sw.Elapsed.TotalSeconds -lt 40) {
    if (Test-Path $SerialLog) {
        $content = Get-Content $SerialLog -Raw -ErrorAction SilentlyContinue
        if ($content -match "desktop ready") {
            $booted = $true
            Write-Host "Desktop ready after $([int]$sw.Elapsed.TotalSeconds)s!"
            break
        }
    }
    Start-Sleep -Milliseconds 500
}

if (-not $booted) {
    Write-Warning "Desktop ready timeout, checking serial anyway..."
}

# 6. Focus Terminal using Ctrl+3 (1d 04 84 9d)
Start-Sleep -Seconds 2
Write-Host "Sending Ctrl+3 to focus Terminal window..."
& $VBoxManage controlvm $VmName keyboardputscancode 1d 04 84 9d | Out-Null
Start-Sleep -Milliseconds 500

# Type 'ps' and press Enter:
Write-Host "Typing 'ps' in Terminal..."
& $VBoxManage controlvm $VmName keyboardputscancode 19 99 1f 9f 1c 9c | Out-Null
Start-Sleep -Seconds 1

# Type 'elfrun /bin/hello.elf' and Enter:
Write-Host "Typing 'elfrun /bin/hello.elf' in Terminal..."
& $VBoxManage controlvm $VmName keyboardputscancode 12 92 26 a6 21 a1 13 93 16 96 31 b1 39 b9 35 b5 30 b0 17 97 31 b1 35 b5 23 a3 12 92 26 a6 26 a6 18 98 34 b4 12 92 26 a6 21 a1 1c 9c | Out-Null
Start-Sleep -Seconds 2

# Type 'elfrun /bin/fault.elf' to test Ring 3 user fault isolation:
Write-Host "Typing 'elfrun /bin/fault.elf' in Terminal to test user exception isolation..."
& $VBoxManage controlvm $VmName keyboardputscancode 12 92 26 a6 21 a1 13 93 16 96 31 b1 39 b9 35 b5 30 b0 17 97 31 b1 35 b5 21 a1 1e 9e 16 96 26 a6 14 94 34 b4 12 92 26 a6 21 a1 1c 9c | Out-Null
Start-Sleep -Seconds 3

# 7. Take screenshot
Write-Host "Capturing 1080p screenshot..."
& $VBoxManage controlvm $VmName screenshotpng $Screenshot | Out-Null

# 8. Copy to artifact dir for reporting
$ArtifactDir = "C:\Users\jojin\.gemini\antigravity-ide\brain\c891bc54-28a4-47b1-ba6d-f8c67a69312a"
Copy-Item $Screenshot (Join-Path $ArtifactDir "phase21-screenshot.png") -Force
Copy-Item $Screenshot (Join-Path $Root "live-desktop.png") -Force

# 9. Cleanly poweroff VM
Write-Host "Powering off VM..."
try { & $VBoxManage controlvm $VmName poweroff 2>$null | Out-Null } catch {}

# 10. Display serial log snippet
Write-Host "`n=== Serial Output Summary ==="
if (Test-Path $SerialLog) {
    Get-Content $SerialLog | Select-Object -Last 60
}
