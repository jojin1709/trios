//! Builds the bootable TriOS UEFI disk image from the kernel ELF.
//!
//! Pipeline (all inside `cargo build` at the workspace root):
//!   1. cargo compiles `kernel` for `x86_64-unknown-none` (artifact dependency,
//!      enabled by `.cargo/config.toml` -> `[unstable] bindeps = true`).
//!   2. this script receives the kernel ELF path in `CARGO_BIN_FILE_KERNEL_kernel`.
//!   3. the `bootloader` crate wraps the ELF in a GPT disk image with an ESP
//!      containing `EFI/BOOT/BOOTX64.EFI` (its own UEFI stage, built on demand).
//!   4. the image is copied next to the profile output for easy discovery:
//!      `target/debug/trios-uefi.img` or `target/release/trios-uefi.img`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));

    // Path to the kernel ELF, provided by cargo's artifact-dependency feature.
    let kernel_elf = PathBuf::from(
        env::var_os("CARGO_BIN_FILE_KERNEL_kernel")
            .expect("artifact dependency `kernel` should export CARGO_BIN_FILE_KERNEL_kernel"),
    );

    let is_release = env::var("PROFILE").as_deref() == Ok("release");
    let image_name = if is_release {
        "trios-uefi-release.img"
    } else {
        "trios-uefi.img"
    };

    // Create the raw UEFI disk image inside OUT_DIR.
    let out_image = out_dir.join(image_name);
    bootloader::UefiBoot::new(&kernel_elf)
        .create_disk_image(&out_image)
        .expect("failed to create UEFI disk image from kernel ELF");

    // Publish it at a stable, documented location:
    // OUT_DIR = <target-dir>/<profile>/build/<unit>/<hash>/out -> up four levels.
    let profile_dir = out_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .and_then(|p| p.parent());

    let image_path = match profile_dir {
        Some(profile_dir) => {
            let dest = profile_dir.join(image_name);
            match fs::copy(&out_image, &dest) {
                Ok(_) => dest,
                Err(e) => {
                    println!(
                        "cargo:warning=could not publish image to {}: {e}",
                        dest.display()
                    );
                    out_image.clone()
                }
            }
        }
        None => out_image.clone(),
    };

    // Consumed by src/main.rs and usable by the run-*.ps1 scripts.
    println!("cargo:rustc-env=TRIOS_UEFI_IMAGE={}", image_path.display());
}
