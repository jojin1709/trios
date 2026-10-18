# Automated CLI VirtualBox Test for TriOS Phase 22 (VFS + Persistent Writes)
$ErrorActionPreference = "Stop"
$Root = "D:\trios"
$VBoxManage = "D:\virtualbox\VBoxManage.exe"
$VmName = "TriOS"
$SerialLog = Join-Path $Root "serial.log"
$Screenshot = Join-Path $Root "phase22-screenshot.png"
$env:VBOX_USER_HOME = Join-Path $Root ".vbox-run"

Write-Host "=== TriOS Phase 22 CLI VirtualBox Test (VFS & Persistent Writes) ==="

# 1. Clean previous VM if running
Get-Process | Where-Object { $_.ProcessName -match "vbox" } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
if (Test-Path $env:VBOX_USER_HOME) {
    Remove-Item -Recurse -Force $env:VBOX_USER_HOME -ErrorAction SilentlyContinue
}
Start-Sleep -Milliseconds 500

# 2. Convert VMDK from release image
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

# Keystroke sender helper
function Send-Cmd($text) {
    $scancodes = @()
    foreach ($ch in $text.ToCharArray()) {
        switch ($ch) {
            'a' { $scancodes += "1e", "9e" }
            'b' { $scancodes += "30", "b0" }
            'c' { $scancodes += "2e", "ae" }
            'd' { $scancodes += "20", "a0" }
            'e' { $scancodes += "12", "92" }
            'f' { $scancodes += "21", "a1" }
            'g' { $scancodes += "22", "a2" }
            'h' { $scancodes += "23", "a3" }
            'i' { $scancodes += "17", "97" }
            'j' { $scancodes += "24", "a4" }
            'k' { $scancodes += "25", "a5" }
            'l' { $scancodes += "26", "a6" }
            'm' { $scancodes += "32", "b2" }
            'n' { $scancodes += "31", "b1" }
            'o' { $scancodes += "18", "98" }
            'p' { $scancodes += "19", "99" }
            'q' { $scancodes += "10", "90" }
            'r' { $scancodes += "13", "93" }
            's' { $scancodes += "1f", "9f" }
            't' { $scancodes += "14", "94" }
            'u' { $scancodes += "16", "96" }
            'v' { $scancodes += "2f", "af" }
            'w' { $scancodes += "11", "91" }
            'x' { $scancodes += "2d", "ad" }
            'y' { $scancodes += "15", "95" }
            'z' { $scancodes += "2c", "ac" }
            '0' { $scancodes += "0b", "8b" }
            '1' { $scancodes += "02", "82" }
            '2' { $scancodes += "03", "83" }
            '3' { $scancodes += "04", "84" }
            '4' { $scancodes += "05", "85" }
            '5' { $scancodes += "06", "86" }
            '6' { $scancodes += "07", "87" }
            '7' { $scancodes += "08", "88" }
            '8' { $scancodes += "09", "89" }
            '9' { $scancodes += "0a", "8a" }
            ' ' { $scancodes += "39", "b9" }
            '/' { $scancodes += "35", "b5" }
            '.' { $scancodes += "34", "b4" }
            '-' { $scancodes += "0c", "8c" }
            '_' { $scancodes += "2a", "0c", "8c", "aa" }
            default { }
        }
    }
    $scancodes += "1c", "9c" # Enter
    for ($i = 0; $i -lt $scancodes.Count; $i += 16) {
        $chunk = $scancodes[$i..([Math]::Min($i + 15, $scancodes.Count - 1))]
        $cmdArgs = @('controlvm', $VmName, 'keyboardputscancode') + $chunk
        & $VBoxManage @cmdArgs | Out-Null
        Start-Sleep -Milliseconds 60
    }
}

# 6. Focus Terminal using Ctrl+3 (1d 04 84 9d)
Start-Sleep -Seconds 2
Write-Host "Sending Ctrl+3 to focus Terminal window..."
& $VBoxManage controlvm $VmName keyboardputscancode 1d 04 84 9d | Out-Null
Start-Sleep -Milliseconds 600

# 7. Run initial fsck-lite
Write-Host "Running: fsck-lite"
Send-Cmd "fsck-lite"
Start-Sleep -Seconds 1

# 8. Write a file to /disk
Write-Host "Running: write /disk/test.txt hello_fat32"
Send-Cmd "write /disk/test.txt hello_fat32"
Start-Sleep -Seconds 1

# 9. Create a directory on /disk
Write-Host "Running: mkdir /disk/myfolder"
Send-Cmd "mkdir /disk/myfolder"
Start-Sleep -Seconds 1

# 10. List /disk
Write-Host "Running: ls /disk"
Send-Cmd "ls /disk"
Start-Sleep -Seconds 1

# 11. Read back /disk/test.txt
Write-Host "Running: cat /disk/test.txt"
Send-Cmd "cat /disk/test.txt"
Start-Sleep -Seconds 1

# 12. Run fsck-lite to verify volume health
Write-Host "Running: fsck-lite (verify clean volume)"
Send-Cmd "fsck-lite"
Start-Sleep -Seconds 2

# 13. Take screenshot
Write-Host "Capturing 1080p screenshot..."
& $VBoxManage controlvm $VmName screenshotpng $Screenshot | Out-Null

# 14. Copy to artifact dir for reporting
$ArtifactDir = "C:\Users\jojin\.gemini\antigravity-ide\brain\c891bc54-28a4-47b1-ba6d-f8c67a69312a"
Copy-Item $Screenshot (Join-Path $ArtifactDir "phase22-screenshot.png") -Force
Copy-Item $Screenshot (Join-Path $Root "live-desktop.png") -Force

# 15. Cleanly poweroff VM
Write-Host "Powering off VM..."
try { & $VBoxManage controlvm $VmName poweroff 2>$null | Out-Null } catch {}

# 16. Display serial log snippet
Write-Host "`n=== Serial Output Summary ==="
if (Test-Path $SerialLog) {
    Get-Content $SerialLog | Select-String "term:", "vfs", "fat" | Select-Object -Last 35
}
Write-Host "`nScreenshot saved to: $Screenshot and $ArtifactDir\phase22-screenshot.png"
