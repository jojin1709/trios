//! Assembly routines for multitasking and context switching.
//!
//! Provides the core primitives to switch execution between thread stacks:
//!   * `switch_context(prev_rsp_ptr: *mut usize, next_rsp: usize)`
//!     Pushes callee-saved registers (r15, r14, r13, r12, rbx, rbp) onto current stack,
//!     stores current `rsp` into `*prev_rsp_ptr`,
//!     loads `next_rsp` into `rsp`,
//!     pops callee-saved registers,
//!     and `ret`s to the new thread's saved instruction pointer.
//!   * `thread_entry()`
//!     Trampoline that unlocks the scheduler and invokes the thread's Rust entry point.

use core::arch::naked_asm;

/// Performs a cooperative context switch between two execution contexts.
///
/// # Safety
/// Caller must ensure that `prev_rsp` points to valid memory where the current stack
/// pointer can be stored, and `next_rsp` is a valid stack frame prepared with
/// callee-saved registers and a return address.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(prev_rsp: *mut usize, next_rsp: usize) {
    naked_asm!(
        // 1. Save callee-saved registers of current thread onto its stack
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // 2. Save current stack pointer to *prev_rsp (rdi is 1st arg in SysV ABI)
        "mov [rdi], rsp",

        // 3. Load target thread's stack pointer from next_rsp (rsi is 2nd arg)
        "mov rsp, rsi",

        // 4. Restore callee-saved registers from target thread's stack
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",

        // 5. Jump to target thread (pops return address from stack)
        "ret"
    );
}

/// Trampoline called when a newly spawned thread starts running for the first time.
///
/// The stack is set up so that `r12` holds the function pointer (`extern "C" fn()`)
/// and `ret` from `switch_context` jumps directly here.
#[unsafe(naked)]
pub unsafe extern "C" fn thread_trampoline() -> ! {
    naked_asm!(
        // On entry from switch_context:
        // Call the entry function pointer stored in r12
        "call r12",

        // If thread entry function returns, call thread_exit
        "call {exit_fn}",

        // If exit_fn somehow returns, halt safely
        "2:",
        "hlt",
        "jmp 2b",
        exit_fn = sym crate::sched::thread_exit
    );
}
