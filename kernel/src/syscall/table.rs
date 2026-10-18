//! Syscall dispatch table and service implementations.

use core::sync::atomic::{AtomicU64, Ordering};

/// Syscall numbers
pub const SYS_EXIT: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_READ: usize = 2;
pub const SYS_YIELD: usize = 3;
pub const SYS_UPTIME: usize = 4;
pub const SYS_GETPID: usize = 5;

static SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Syscall dispatcher called from naked assembly entry.
///
/// Arguments follow x86_64 SysV calling convention:
///   * `number`: rax (passed into rdi)
///   * `arg1`: rdi (passed into rsi)
///   * `arg2`: rsi (passed into rdx)
///   * `arg3`: rdx (passed into rcx)
pub extern "C" fn syscall_dispatch(
    number: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> usize {
    let count = SYSCALL_COUNT.fetch_add(1, Ordering::Relaxed);
    if count == 0 {
        serial_println!("[syscall] first syscall invoked (num={number})");
    }

    match number {
        SYS_EXIT => {
            serial_println!("[syscall] process exit requested (code {arg1})");
            crate::sched::thread_exit();
        }
        SYS_WRITE => {
            // arg1 = fd, arg2 = ptr, arg3 = len
            let fd = arg1;
            let ptr = arg2 as *const u8;
            let len = arg3;

            if ptr.is_null() || len == 0 {
                return 0;
            }

            // fd 1 = stdout (COM1 serial log for now)
            if fd == 1 || fd == 2 {
                let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
                if let Ok(s) = core::str::from_utf8(slice) {
                    serial_print!("{s}");
                    return len;
                }
            }
            0
        }
        SYS_READ => {
            // Placeholder: non-blocking read
            0
        }
        SYS_YIELD => {
            crate::sched::yield_now();
            0
        }
        SYS_UPTIME => {
            crate::timer::uptime_seconds() as usize
        }
        SYS_GETPID => {
            // Return thread ID (1 for background threads)
            1
        }
        _ => {
            serial_println!("[syscall] warning: unhandled syscall #{number}");
            usize::MAX
        }
    }
}
