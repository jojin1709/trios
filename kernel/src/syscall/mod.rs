//! Fast system call (`syscall`/`sysret`) hardware configuration and assembly entry.

use core::arch::naked_asm;
use x86_64::registers::model_specific::{Efer, EferFlags, Msr};

pub mod table;

use table::syscall_dispatch;

// Architectural Model Specific Registers (MSRs) for syscall/sysret
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;

/// Low-level naked assembly entry point for the `syscall` instruction.
///
/// Hardware behavior on `syscall`:
///   * RIP is saved in RCX.
///   * RFLAGS is saved in R11.
///   * RFLAGS is masked with IA32_FMASK (interrupts disabled).
///   * CS and SS are loaded from IA32_STAR.
///   * RIP is loaded from IA32_LSTAR (jumps directly here).
///
/// On entry, RSP still points to the caller's stack. We must preserve registers,
/// call `syscall_dispatch`, and restore registers before `sysretq`.
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_entry() {
    naked_asm!(
        // Save caller's user stack and registers
        "push rcx", // User RIP
        "push r11", // User RFLAGS
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Arguments for syscall_dispatch(number, arg1, arg2, arg3):
        // rax = syscall number -> rdi (1st arg)
        // rdi = arg1           -> rsi (2nd arg)
        // rsi = arg2           -> rdx (3rd arg)
        // rdx = arg3           -> rcx (4th arg)
        "mov rcx, rdx",
        "mov rdx, rsi",
        "mov rsi, rdi",
        "mov rdi, rax",

        // Call Rust syscall dispatcher
        "call {dispatch}",

        // Restore registers (rax contains the return value from dispatch)
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11", // User RFLAGS
        "pop rcx", // User RIP

        // Restore user RFLAGS
        "push r11",
        "popfq",

        // Return to caller via return address saved in rcx
        "jmp rcx",
        dispatch = sym syscall_dispatch
    );
}

/// Initializes the CPU Model-Specific Registers (MSRs) to enable fast syscalls.
pub fn init() {
    let selectors = crate::gdt::selectors();

    unsafe {
        // 1. Enable SCE (System Call Enable) bit in EFER MSR
        let mut efer = Efer::read();
        efer.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
        Efer::write(efer);

        // 2. Program IA32_STAR MSR:
        // Bits 32..47: Kernel CS/SS base selector (used by syscall)
        // Bits 48..63: User CS/SS base selector (used by sysret)
        let kernel_cs = selectors.kernel_code.0 as u64;
        let user_cs = (selectors.user_data.0 as u64) | 3;
        let star_val = (user_cs << 48) | (kernel_cs << 32);
        Msr::new(IA32_STAR).write(star_val);

        // 3. Program IA32_LSTAR with the address of `syscall_entry`
        let lstar_val = syscall_entry as *const () as u64;
        Msr::new(IA32_LSTAR).write(lstar_val);

        // 4. Program IA32_FMASK: Mask Interrupt Flag (0x200) on syscall entry
        // This ensures interrupts are disabled during the initial register/stack swap
        Msr::new(IA32_FMASK).write(0x200);
    }

    serial_println!("[syscall] IA32_EFER / STAR / LSTAR / FMASK programmed (fast syscalls ready)");
}

/// Helper function to invoke a syscall from Rust (used for kernel-space userland testing).
pub fn syscall(num: usize, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            out("rcx") _, // clobbered by CPU (saves RIP)
            out("r11") _, // clobbered by CPU (saves RFLAGS)
        );
    }
    ret
}
