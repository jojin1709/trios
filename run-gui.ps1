# Launch TriOS in a visible VirtualBox GUI window
$Root = "D:\trios"
$VBoxManage = "D:\virtualbox\VBoxManage.exe"
$VmName = "TriOS"
$SerialLog = Join-Path $Root "serial.log"
$env:VBOX_USER_HOME = Join-Path $Root ".vbox-home"

Write-Host "Preparing TriOS for interactive GUI window..."

# Power off any running instance
try { & $VBoxManage controlvm $VmName poweroff 2>$null | Out-Null } catch {}
try { & $VBoxManage unregistervm $VmName --delete 2>$null | Out-Null } catch {}

# Convert latest release image
$Image = Join-Path $Root "target\release\trios-uefi-release.img"
$Vmdk = Join-Path $Root "trios-uefi-test.vmdk"
if (Test-Path $Vmdk) { Remove-Item $Vmdk -Force }
& $VBoxManage convertfromraw $Image $Vmdk --format VMDK | Out-Null

# Create & configure VM
$MachineFolder = Join-Path $env:VBOX_USER_HOME "Machines"
New-Item -ItemType Directory -Force -Path $MachineFolder | Out-Null
& $VBoxManage createvm --name $VmName --ostype Other_64 --basefolder $MachineFolder --register | Out-Null
& $VBoxManage modifyvm $VmName `
    --firmware efi `
    --memory 256 `
    --vram 16 `
    --boot1 disk --boot2 none --boot3 none --boot4 none `
    --nic1 none `
    --mouse ps2 --keyboard ps2 `
    --audio-out off --audio-in off `
    --uart1 0x3F8 4 `
    --uartmode1 file $SerialLog | Out-Null
& $VBoxManage storagectl $VmName --name SATA --add sata --controller IntelAhci --portcount 1 | Out-Null
& $VBoxManage storageattach $VmName --storagectl SATA --port 0 --device 0 --type hdd --medium $Vmdk | Out-Null

Write-Host "Starting TriOS with visible display window..."
& $VBoxManage startvm $VmName --type gui
Write-Host "VirtualBox window opened! You can now view and interact directly with TriOS."
