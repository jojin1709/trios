//! Build script for TriOS kernel.
//! Compiles userland binaries from `../user` and prepares them for embedding into `/bin` ramfs.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=../user/src");
    println!("cargo:rerun-if-changed=../user/Cargo.toml");
    println!("cargo:rerun-if-changed=../libtrios/src");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let root_dir = manifest_dir.parent().unwrap_or(&manifest_dir);

    let release_dir = root_dir.join("target").join("x86_64-unknown-none").join("release");

    let bin_names = ["init", "sh", "hello", "ls", "cat", "echo", "ps", "clear", "fetch"];

    for name in &bin_names {
        let dest = out_dir.join(format!("{}.elf", name));
        let src = release_dir.join(name);

        if src.exists() {
            let _ = fs::copy(&src, &dest);
        } else {
            // Write a dummy minimal ELF if binary missing
            let dummy_elf = [
                0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                2, 0, 0x3E, 0, 1, 0, 0, 0, 0x78, 0x00, 0x40, 0, 0, 0, 0, 0,
                64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 64, 0, 56, 0, 1, 0, 0, 0, 0, 0, 0, 0,
                // Program header
                1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0,
                128, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0,
                0, 0x10, 0, 0, 0, 0, 0, 0,
                // Instructions: exit(0)
                0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
                0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00, // mov rdi, 0
                0x0F, 0x05,                               // syscall
            ];
            let _ = fs::write(&dest, &dummy_elf);
        }
    }
}
