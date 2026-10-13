//! TriOS disk-image builder (workspace root crate).
//!
//! This binary doesn't run on the target — it prints where the bootable image
//! landed and how to boot it. The image itself is created in `build.rs` from
//! the kernel artifact dependency (see `CARGO_BIN_FILE_KERNEL_kernel`).

fn main() {
    let image = env!("TRIOS_UEFI_IMAGE");

    println!("TriOS image builder");
    println!("===================");
    println!("bootable UEFI disk image: {image}");
    println!();
    println!("Next steps:");
    println!("  QEMU         : powershell -ExecutionPolicy Bypass -File run-qemu.ps1");
    println!("  VirtualBox   : powershell -ExecutionPolicy Bypass -File run-virtualbox.ps1");
    println!("  Serial log   : type the produced image path above into your VM's serial setup");
    println!();
    println!("See README.md for manual VBoxManage commands (UEFI + serial capture).");
}
