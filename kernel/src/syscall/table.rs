//! Syscall dispatch table and service implementations with strict user pointer validation.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::control::Cr3;

use crate::sched::process::{PROCESS_TABLE, current_pid, kill_process, validate_user_buffer};

/// Standard system call numbers.
pub const SYS_EXIT: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_READ: usize = 2;
pub const SYS_YIELD: usize = 3;
pub const SYS_UPTIME: usize = 4;
pub const SYS_GETPID: usize = 5;
pub const SYS_SLEEP_MS: usize = 6;
pub const SYS_MMAP: usize = 7;
pub const SYS_MUNMAP: usize = 8;
pub const SYS_OPEN: usize = 9;
pub const SYS_CLOSE: usize = 10;

static SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Helper to read a null-terminated string from a validated user space pointer.
unsafe fn read_user_string(ptr: *const u8, max_len: usize) -> Result<alloc::string::String, &'static str> {
    let mut bytes = alloc::vec::Vec::new();
    for i in 0..max_len {
        let b = unsafe { *ptr.add(i) };
        if b == 0 {
            break;
        }
        bytes.push(b);
    }
    alloc::string::String::from_utf8(bytes).map_err(|_| "invalid UTF-8 in user string")
}

/// Syscall dispatcher called from naked assembly entry.
pub extern "C" fn syscall_dispatch(
    number: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> usize {
    let count = SYSCALL_COUNT.fetch_add(1, Ordering::Relaxed);
    let pid = current_pid();

    if count == 0 {
        serial_println!("[syscall] first syscall invoked (num={number}, PID={pid})");
    }

    let pml4 = Cr3::read().0;

    match number {
        SYS_EXIT => {
            let exit_code = arg1 as i32;
            serial_println!("[syscall] process exit requested: PID #{pid} (code {exit_code})");
            if pid != 0 {
                crate::sched::restore_kernel_pml4();
                kill_process(pid);
                crate::sched::set_current_pid(0);
            }
            x86_64::instructions::interrupts::enable();
            crate::sched::thread_exit();
        }

        SYS_WRITE => {
            // arg1 = fd, arg2 = user ptr, arg3 = len
            let fd = arg1;
            let ptr = arg2;
            let len = arg3;

            if ptr == 0 || len == 0 {
                return 0;
            }

            // Strictly validate user buffer pointer
            if pid != 0 {
                if let Err(err) = validate_user_buffer(pml4, ptr, len, false) {
                    serial_println!("[syscall] SYS_WRITE pointer validation failed: {err}");
                    return usize::MAX; // -1 EFAULT
                }
            }

            let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len) };

            // fd 1 = stdout, fd 2 = stderr
            if fd == 1 || fd == 2 {
                if let Ok(s) = core::str::from_utf8(slice) {
                    serial_print!("{s}");
                    return len;
                }
            } else if fd >= 3 {
                // Write to open ramfs file descriptor
                let mut table = PROCESS_TABLE.lock();
                if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                    if let Some(Some(desc)) = proc.fd_table.get_mut(fd) {
                        if desc.writable {
                            if crate::fs::write_bytes(&desc.path, slice) {
                                desc.offset += len;
                                return len;
                            }
                        }
                    }
                }
            }
            0
        }

        SYS_READ => {
            // arg1 = fd, arg2 = user ptr, arg3 = len
            let fd = arg1;
            let ptr = arg2;
            let len = arg3;

            if ptr == 0 || len == 0 {
                return 0;
            }

            // Strictly validate user buffer pointer with write permission
            if pid != 0 {
                if let Err(err) = validate_user_buffer(pml4, ptr, len, true) {
                    serial_println!("[syscall] SYS_READ pointer validation failed: {err}");
                    return usize::MAX; // -1 EFAULT
                }
            }

            if fd == 0 {
                // Non-blocking read from stdin (placeholder: 0 bytes ready)
                0
            } else if fd >= 3 {
                // Read from open ramfs file descriptor
                let mut table = PROCESS_TABLE.lock();
                if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                    if let Some(Some(desc)) = proc.fd_table.get_mut(fd) {
                        if let Some(file_data) = crate::fs::read_bytes(&desc.path) {
                            if desc.offset < file_data.len() {
                                let available = file_data.len() - desc.offset;
                                let to_read = available.min(len);
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        file_data.as_ptr().add(desc.offset),
                                        ptr as *mut u8,
                                        to_read,
                                    );
                                }
                                desc.offset += to_read;
                                return to_read;
                            }
                        }
                    }
                }
                0
            } else {
                0
            }
        }

        SYS_YIELD => {
            crate::sched::yield_now();
            0
        }

        SYS_UPTIME => crate::timer::uptime_seconds() as usize,

        SYS_GETPID => pid as usize,

        SYS_SLEEP_MS => {
            let ms = arg1;
            let ticks = ((ms + 9) / 10).max(1) as u64;
            crate::sched::sleep_ticks(ticks);
            0
        }

        SYS_MMAP => {
            // arg1 = addr hint, arg2 = len, arg3 = prot
            let len = arg2;
            let mut table = PROCESS_TABLE.lock();
            if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                match proc.mmap(len) {
                    Ok(addr) => addr as usize,
                    Err(e) => {
                        serial_println!("[syscall] SYS_MMAP failed: {e}");
                        0
                    }
                }
            } else {
                0
            }
        }

        SYS_MUNMAP => 0,

        SYS_OPEN => {
            // arg1 = path ptr, arg2 = flags (0 = read, 1 = write)
            let path_ptr = arg1;
            let flags = arg2;

            if pid != 0 {
                if let Err(err) = validate_user_buffer(pml4, path_ptr, 1, false) {
                    serial_println!("[syscall] SYS_OPEN path pointer validation failed: {err}");
                    return usize::MAX;
                }
            }

            let path = match unsafe { read_user_string(path_ptr as *const u8, 128) } {
                Ok(p) => p,
                Err(err) => {
                    serial_println!("[syscall] SYS_OPEN string read failed: {err}");
                    return usize::MAX;
                }
            };

            let mut table = PROCESS_TABLE.lock();
            if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                // Find empty fd slot >= 3
                let mut target_fd = None;
                for i in 3..proc.fd_table.len() {
                    if proc.fd_table[i].is_none() {
                        target_fd = Some(i);
                        break;
                    }
                }
                let fd = match target_fd {
                    Some(idx) => idx,
                    None => {
                        let idx = proc.fd_table.len().max(3);
                        while proc.fd_table.len() < idx {
                            proc.fd_table.push(None);
                        }
                        proc.fd_table.push(None);
                        idx
                    }
                };

                proc.fd_table[fd] = Some(crate::sched::process::ProcessFileDesc {
                    path,
                    offset: 0,
                    writable: (flags & 1) != 0,
                });
                fd
            } else {
                usize::MAX
            }
        }

        SYS_CLOSE => {
            let fd = arg1;
            let mut table = PROCESS_TABLE.lock();
            if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                if fd >= 3 && fd < proc.fd_table.len() {
                    proc.fd_table[fd] = None;
                    return 0;
                }
            }
            usize::MAX
        }

        _ => {
            serial_println!("[syscall] warning: unhandled syscall #{number}");
            usize::MAX
        }
    }
}
