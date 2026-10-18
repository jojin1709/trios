//! Fast system call (`syscall`/`sysret`) hardware configuration and assembly entry.

use core::arch::naked_asm;
use x86_64::registers::model_specific::{Efer, EferFlags, Msr};

pub mod table;

use table::syscall_dispatch;

// Architectural Model Specific Registers (MSRs) for syscall/sysret
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;

#[unsafe(no_mangle)]
static mut USER_RSP_SCRATCH: u64 = 0;

const KERNEL_SYSCALL_STACK_SIZE: usize = 16384;
static mut KERNEL_SYSCALL_STACK: [u8; KERNEL_SYSCALL_STACK_SIZE] = [0; KERNEL_SYSCALL_STACK_SIZE];

#[unsafe(no_mangle)]
static mut KERNEL_SYSCALL_RSP: u64 = 0;

/// Low-level naked assembly entry point for the `syscall` instruction.
///
/// Hardware behavior on `syscall`:
///   * RIP is saved in RCX.
///   * RFLAGS is saved in R11.
///   * RFLAGS is masked with IA32_FMASK (interrupts disabled).
///   * CS and SS are loaded from IA32_STAR.
///   * RIP is loaded from IA32_LSTAR (jumps directly here).
///
/// On entry, we save caller's user RSP to `USER_RSP_SCRATCH` and switch
/// to the dedicated kernel syscall stack `KERNEL_SYSCALL_RSP`.
/// We preserve user registers, call `syscall_dispatch`, restore registers and user stack,
/// and return to Ring 3 via `sysretq`.
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_entry() {
    naked_asm!(
        // Save caller's user stack pointer
        "mov [rip + {user_rsp_scratch}], rsp",

        // Switch to dedicated kernel syscall stack
        "mov rsp, [rip + {kernel_rsp}]",

        // Save registers on kernel stack
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

        // Restore user stack pointer
        "mov rsp, [rip + {user_rsp_scratch}]",

        // Fast transition back to Ring 3 userspace
        "sysretq",

        user_rsp_scratch = sym USER_RSP_SCRATCH,
        kernel_rsp = sym KERNEL_SYSCALL_RSP,
        dispatch = sym syscall_dispatch
    );
}

/// Initializes the CPU Model-Specific Registers (MSRs) to enable fast syscalls.
pub fn init() {
    unsafe {
        // Initialize kernel syscall stack top
        KERNEL_SYSCALL_RSP = core::ptr::addr_of!(KERNEL_SYSCALL_STACK) as u64 + KERNEL_SYSCALL_STACK_SIZE as u64;

        // 1. Enable SCE (System Call Enable) bit in EFER MSR
        let mut efer = Efer::read();
        efer.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
        Efer::write(efer);

        // 2. Program IA32_STAR MSR:
        // Bits 32..47: Kernel CS/SS base selector (used by syscall) -> 0x0008
        // Bits 48..63: User base selector (used by sysret) -> 0x0010
        // When sysretq executes:
        //   CS is loaded with (STAR[63:48] + 16) | 3 = (0x10 + 0x10) | 3 = 0x20 | 3 = 0x23 (user_code)
        //   SS is loaded with (STAR[63:48] + 8) | 3 = (0x10 + 0x08) | 3 = 0x18 | 3 = 0x1B (user_data)
        let star_val = (0x0010u64 << 48) | (0x0008u64 << 32);
        Msr::new(IA32_STAR).write(star_val);

        // 3. Program IA32_LSTAR with the address of `syscall_entry`
        let lstar_val = syscall_entry as *const () as u64;
        Msr::new(IA32_LSTAR).write(lstar_val);

        // 4. Program IA32_FMASK: Mask Interrupt Flag (0x200) on syscall entry
        Msr::new(IA32_FMASK).write(0x200);
    }

    serial_println!("[syscall] IA32_EFER / STAR / LSTAR / FMASK programmed (fast syscalls ready)");
}

/// Helper function to invoke a syscall from Rust (direct dispatch for kernel threads).
pub fn syscall(num: usize, arg1: usize, arg2: usize, arg3: usize) -> usize {
    table::syscall_dispatch(num, arg1, arg2, arg3)
}
