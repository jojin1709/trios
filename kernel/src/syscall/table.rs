//! Syscall dispatch table and service implementations with strict user pointer validation.

use alloc::{format, string::String, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::control::Cr3;

use crate::sched::process::{
    FdKind, PIPES, PROCESS_TABLE, PipeBuffer, ProcessFileDesc, ProcessState, SHELL_PTY,
    current_pid, spawn_process_with_args, validate_user_buffer,
};

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
pub const SYS_SPAWN: usize = 11;
pub const SYS_WAIT: usize = 12;
pub const SYS_PIPE: usize = 13;
pub const SYS_DUP2: usize = 14;
pub const SYS_READDIR: usize = 15;
pub const SYS_STAT: usize = 16;
pub const SYS_GETCWD: usize = 17;
pub const SYS_CHDIR: usize = 18;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Stat {
    pub size: u64,
    pub is_dir: u8,
    pub _pad: [u8; 7],
}

static SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Helper to read a null-terminated string from a validated user space pointer.
unsafe fn read_user_string(
    pml4: x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>,
    ptr: *const u8,
    max_len: usize,
    is_user: bool,
) -> Result<String, &'static str> {
    if ptr.is_null() {
        return Err("null pointer dereference rejected");
    }
    let mut bytes = Vec::new();
    for i in 0..max_len {
        let addr = (ptr as usize).checked_add(i).ok_or("pointer arithmetic overflow")?;
        if is_user {
            validate_user_buffer(pml4, addr, 1, false)?;
        }
        let b = unsafe { *(addr as *const u8) };
        if b == 0 {
            break;
        }
        bytes.push(b);
    }
    String::from_utf8(bytes).map_err(|_| "invalid UTF-8 in user string")
}

/// Syscall dispatcher called from naked assembly entry.
pub extern "C" fn syscall_dispatch(
    number: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    _arg4: usize,
    _arg5: usize,
) -> usize {
    let count = SYSCALL_COUNT.fetch_add(1, Ordering::Relaxed);
    let pid = current_pid();

    if count == 0 {
        serial_println!("[syscall] first syscall invoked (num={number}, PID={pid})");
    }

    let pml4 = Cr3::read().0;
    if pid != 0 {
        serial_println!("[syscall] PID #{pid} call {number} (arg1={arg1:#x}, arg2={arg2:#x}, arg3={arg3:#x})");
    }

    match number {
        SYS_EXIT => {
            let exit_code = arg1 as i32;
            serial_println!("[syscall] process exit requested: PID #{pid} (code {exit_code})");
            if pid != 0 {
                let mut table = PROCESS_TABLE.lock();
                if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                    proc.state = ProcessState::Dead;
                    proc.exit_code = exit_code;
                }
                drop(table);
                crate::sched::restore_kernel_pml4();
                crate::sched::set_current_pid(0);
            }
            x86_64::instructions::interrupts::enable();
            crate::sched::thread_exit();
        }

        SYS_WRITE => {
            let fd = arg1;
            let ptr = arg2;
            let len = arg3;

            if ptr == 0 || len == 0 {
                return 0;
            }

            if pid != 0 {
                if let Err(err) = validate_user_buffer(pml4, ptr, len, false) {
                    serial_println!("[syscall] SYS_WRITE pointer validation failed: {err}");
                    return usize::MAX;
                }
            }

            let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len) };

            // Handle default stdio for kernel threads or early processes
            if pid == 0 || fd == 1 || fd == 2 {
                if let Ok(s) = core::str::from_utf8(slice) {
                    serial_print!("{s}");
                }
                let mut pty = SHELL_PTY.lock();
                for &b in slice {
                    pty.output_queue.push_back(b);
                }
                return len;
            }

            let mut table = PROCESS_TABLE.lock();
            let proc = match table.iter_mut().find(|p| p.pid == pid) {
                Some(p) => p,
                None => return usize::MAX,
            };

            let desc = match proc.fd_table.get_mut(fd).and_then(|d| d.as_mut()) {
                Some(d) => d.clone(),
                None => return usize::MAX,
            };
            drop(table);

            match desc.kind {
                FdKind::Stdout | FdKind::Stderr => {
                    if let Ok(s) = core::str::from_utf8(slice) {
                        serial_print!("{s}");
                    }
                    let mut pty = SHELL_PTY.lock();
                    for &b in slice {
                        pty.output_queue.push_back(b);
                    }
                    len
                }
                FdKind::PipeWrite { pipe_id } => {
                    let mut pipes = PIPES.lock();
                    if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                        if pipe.reader_count == 0 {
                            return usize::MAX; // Broken pipe (EPIPE)
                        }
                        for &b in slice {
                            pipe.data.push_back(b);
                        }
                        len
                    } else {
                        usize::MAX
                    }
                }
                FdKind::File { path, offset: _, writable } => {
                    if !writable {
                        return usize::MAX;
                    }
                    if crate::fs::vfs::write(&path, slice).is_ok() {
                        let mut table = PROCESS_TABLE.lock();
                        if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                            if let Some(Some(d)) = proc.fd_table.get_mut(fd) {
                                if let FdKind::File { offset: ref mut off, .. } = d.kind {
                                    *off += len;
                                }
                            }
                        }
                        len
                    } else {
                        usize::MAX
                    }
                }
                _ => usize::MAX,
            }
        }

        SYS_READ => {
            let fd = arg1;
            let ptr = arg2;
            let len = arg3;

            if ptr == 0 || len == 0 {
                return 0;
            }

            if pid != 0 {
                if let Err(err) = validate_user_buffer(pml4, ptr, len, true) {
                    serial_println!("[syscall] SYS_READ pointer validation failed: {err}");
                    return usize::MAX;
                }
            }

            let desc_kind = if pid == 0 {
                FdKind::Stdin
            } else {
                let table = PROCESS_TABLE.lock();
                match table.iter().find(|p| p.pid == pid) {
                    Some(p) => match p.fd_table.get(fd).and_then(|d| d.as_ref()) {
                        Some(d) => d.kind.clone(),
                        None => return usize::MAX,
                    },
                    None => return usize::MAX,
                }
            };

            match desc_kind {
                FdKind::Stdin => {
                    // Read from SHELL_PTY input queue
                    loop {
                        let mut pty = SHELL_PTY.lock();
                        if !pty.input_queue.is_empty() {
                            let mut count = 0;
                            while count < len && let Some(b) = pty.input_queue.pop_front() {
                                unsafe {
                                    *((ptr as *mut u8).add(count)) = b;
                                }
                                count += 1;
                                if b == b'\n' {
                                    break;
                                }
                            }
                            return count;
                        }
                        drop(pty);

                        // Yield CPU execution
                        x86_64::instructions::interrupts::enable();
                        crate::sched::yield_now();
                        crate::sched::sleep_ticks(1);
                    }
                }
                FdKind::PipeRead { pipe_id } => {
                    loop {
                        let mut pipes = PIPES.lock();
                        if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                            if !pipe.data.is_empty() {
                                let mut count = 0;
                                while count < len && let Some(b) = pipe.data.pop_front() {
                                    unsafe {
                                        *((ptr as *mut u8).add(count)) = b;
                                    }
                                    count += 1;
                                }
                                return count;
                            }
                            if pipe.writer_count == 0 {
                                // EOF: all writers closed
                                return 0;
                            }
                        } else {
                            return usize::MAX;
                        }
                        drop(pipes);

                        x86_64::instructions::interrupts::enable();
                        crate::sched::yield_now();
                    }
                }
                FdKind::File { path, offset, .. } => {
                    if let Some(file_data) = crate::fs::vfs::read(&path) {
                        if offset < file_data.len() {
                            let available = file_data.len() - offset;
                            let to_read = available.min(len);
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    file_data.as_ptr().add(offset),
                                    ptr as *mut u8,
                                    to_read,
                                );
                            }
                            let mut table = PROCESS_TABLE.lock();
                            if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                                if let Some(Some(d)) = proc.fd_table.get_mut(fd) {
                                    if let FdKind::File { offset: ref mut off, .. } = d.kind {
                                        *off += to_read;
                                    }
                                }
                            }
                            return to_read;
                        } else {
                            return 0; // EOF
                        }
                    }
                    0
                }
                _ => usize::MAX,
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
            let path_ptr = arg1;
            let flags = arg2;

            let path = match unsafe { read_user_string(pml4, path_ptr as *const u8, 128, pid != 0) } {
                Ok(p) => p,
                Err(err) => {
                    serial_println!("[syscall] SYS_OPEN string read failed: {err}");
                    return usize::MAX;
                }
            };

            let mut table = PROCESS_TABLE.lock();
            if let Some(proc) = table.iter_mut().find(|p| p.pid == pid) {
                let resolved = if path.starts_with('/') {
                    path
                } else if proc.cwd == "/" {
                    format!("/{}", path)
                } else {
                    format!("{}/{}", proc.cwd, path)
                };

                if (flags & 1) != 0 && !crate::fs::vfs::exists(&resolved) {
                    let _ = crate::fs::vfs::write(&resolved, &[]);
                }

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

                proc.fd_table[fd] = Some(ProcessFileDesc {
                    kind: FdKind::File {
                        path: resolved,
                        offset: 0,
                        writable: (flags & 1) != 0,
                    },
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
                if fd < proc.fd_table.len() {
                    if let Some(desc) = proc.fd_table[fd].take() {
                        match desc.kind {
                            FdKind::PipeRead { pipe_id } => {
                                let mut pipes = PIPES.lock();
                                if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                                    pipe.reader_count = pipe.reader_count.saturating_sub(1);
                                    if pipe.reader_count == 0 && pipe.writer_count == 0 {
                                        pipes[pipe_id] = None;
                                    }
                                }
                            }
                            FdKind::PipeWrite { pipe_id } => {
                                let mut pipes = PIPES.lock();
                                if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                                    pipe.writer_count = pipe.writer_count.saturating_sub(1);
                                    if pipe.reader_count == 0 && pipe.writer_count == 0 {
                                        pipes[pipe_id] = None;
                                    }
                                }
                            }
                            _ => {}
                        }
                        return 0;
                    }
                }
            }
            usize::MAX
        }

        SYS_SPAWN => {
            let path_ptr = arg1;
            let argv_ptr = arg2;

            let path = match unsafe { read_user_string(pml4, path_ptr as *const u8, 128, pid != 0) } {
                Ok(p) => p,
                Err(_) => return usize::MAX,
            };

            let mut args = Vec::new();
            if argv_ptr != 0 {
                if pid != 0 && validate_user_buffer(pml4, argv_ptr, 8, false).is_err() {
                    return usize::MAX;
                }
                for i in 0..16 {
                    let ptr_addr = match (argv_ptr as usize).checked_add(i * 8) {
                        Some(a) => a,
                        None => break,
                    };
                    if pid != 0 && validate_user_buffer(pml4, ptr_addr, 8, false).is_err() {
                        break;
                    }
                    let str_ptr = unsafe { *(ptr_addr as *const *const u8) };
                    if str_ptr.is_null() {
                        break;
                    }
                    if let Ok(arg_str) = unsafe { read_user_string(pml4, str_ptr, 128, pid != 0) } {
                        args.push(arg_str);
                    } else {
                        break;
                    }
                }
            }

            // Resolve binary executable in VFS
            let cwd = {
                let table = PROCESS_TABLE.lock();
                table.iter().find(|p| p.pid == pid).map(|p| p.cwd.clone()).unwrap_or_else(|| String::from("/"))
            };

            let candidate_paths = [
                path.clone(),
                if cwd == "/" { format!("/{}", path) } else { format!("{}/{}", cwd, path) },
                format!("/bin/{}", path),
                format!("/bin/{}.elf", path),
            ];

            let mut found_bytes = None;
            let mut resolved_path = String::new();
            for c in &candidate_paths {
                if let Some(bytes) = crate::fs::vfs::read(c) {
                    found_bytes = Some(bytes);
                    resolved_path = c.clone();
                    break;
                }
            }

            let elf_bytes = match found_bytes {
                Some(b) => b,
                None => {
                    serial_println!("[syscall] SYS_SPAWN binary not found: '{path}'");
                    return usize::MAX;
                }
            };

            match spawn_process_with_args(&resolved_path, &elf_bytes, &args, Some(pid)) {
                Ok(child_pid) => child_pid as usize,
                Err(err) => {
                    serial_println!("[syscall] SYS_SPAWN failed: {err}");
                    usize::MAX
                }
            }
        }

        SYS_WAIT => {
            let target_pid = arg1;
            loop {
                let mut table = PROCESS_TABLE.lock();
                let mut found_dead = None;
                let mut has_child = false;

                for (idx, p) in table.iter().enumerate() {
                    let matches = if target_pid == 0 || target_pid == usize::MAX {
                        p.parent_pid == Some(pid)
                    } else {
                        p.pid == target_pid as u64
                    };

                    if matches {
                        if p.state == ProcessState::Dead {
                            found_dead = Some((idx, p.exit_code));
                            break;
                        } else {
                            has_child = true;
                        }
                    }
                }

                if let Some((idx, exit_code)) = found_dead {
                    let mut dead_proc = table.remove(idx);
                    dead_proc.teardown();
                    return exit_code as usize;
                }

                if !has_child {
                    return usize::MAX;
                }

                drop(table);
                x86_64::instructions::interrupts::enable();
                crate::sched::yield_now();
            }
        }

        SYS_PIPE => {
            let pipefd_ptr = arg1;
            if pipefd_ptr == 0 {
                return usize::MAX;
            }
            if pid != 0 && validate_user_buffer(pml4, pipefd_ptr, 8, true).is_err() {
                return usize::MAX;
            }

            let mut pipes = PIPES.lock();
            let mut pipe_id = None;
            for (idx, slot) in pipes.iter_mut().enumerate() {
                if slot.is_none() {
                    *slot = Some(PipeBuffer {
                        data: alloc::collections::VecDeque::new(),
                        reader_count: 1,
                        writer_count: 1,
                    });
                    pipe_id = Some(idx);
                    break;
                }
            }
            let pipe_idx = match pipe_id {
                Some(idx) => idx,
                None => {
                    let idx = pipes.len();
                    pipes.push(Some(PipeBuffer {
                        data: alloc::collections::VecDeque::new(),
                        reader_count: 1,
                        writer_count: 1,
                    }));
                    idx
                }
            };
            drop(pipes);

            let mut table = PROCESS_TABLE.lock();
            let proc = match table.iter_mut().find(|p| p.pid == pid) {
                Some(p) => p,
                None => return usize::MAX,
            };

            // Allocate two lowest free fds
            let mut read_fd = None;
            let mut write_fd = None;

            for i in 3..proc.fd_table.len() {
                if proc.fd_table[i].is_none() {
                    if read_fd.is_none() {
                        read_fd = Some(i);
                    } else if write_fd.is_none() {
                        write_fd = Some(i);
                        break;
                    }
                }
            }

            let rfd = read_fd.unwrap_or_else(|| {
                let idx = proc.fd_table.len().max(3);
                while proc.fd_table.len() <= idx {
                    proc.fd_table.push(None);
                }
                idx
            });
            proc.fd_table[rfd] = Some(ProcessFileDesc {
                kind: FdKind::PipeRead { pipe_id: pipe_idx },
            });

            let wfd = write_fd.unwrap_or_else(|| {
                let idx = proc.fd_table.len().max(3);
                while proc.fd_table.len() <= idx {
                    proc.fd_table.push(None);
                }
                idx
            });
            proc.fd_table[wfd] = Some(ProcessFileDesc {
                kind: FdKind::PipeWrite { pipe_id: pipe_idx },
            });

            unsafe {
                let ptr = pipefd_ptr as *mut i32;
                *ptr = rfd as i32;
                *ptr.add(1) = wfd as i32;
            }

            0
        }

        SYS_DUP2 => {
            let oldfd = arg1;
            let newfd = arg2;

            if newfd > 64 {
                return usize::MAX;
            }
            if oldfd == newfd {
                return newfd;
            }

            let mut table = PROCESS_TABLE.lock();
            let proc = match table.iter_mut().find(|p| p.pid == pid) {
                Some(p) => p,
                None => return usize::MAX,
            };

            if oldfd >= proc.fd_table.len() || proc.fd_table[oldfd].is_none() {
                return usize::MAX;
            }

            let src_desc = proc.fd_table[oldfd].as_ref().unwrap().clone();

            // Increment pipe refcount if duplicating pipe fd
            match src_desc.kind {
                FdKind::PipeRead { pipe_id } => {
                    let mut pipes = PIPES.lock();
                    if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                        pipe.reader_count += 1;
                    }
                }
                FdKind::PipeWrite { pipe_id } => {
                    let mut pipes = PIPES.lock();
                    if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                        pipe.writer_count += 1;
                    }
                }
                _ => {}
            }

            while proc.fd_table.len() <= newfd {
                proc.fd_table.push(None);
            }

            // Close existing newfd if present
            if let Some(existing) = proc.fd_table[newfd].take() {
                match existing.kind {
                    FdKind::PipeRead { pipe_id } => {
                        let mut pipes = PIPES.lock();
                        if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                            pipe.reader_count = pipe.reader_count.saturating_sub(1);
                        }
                    }
                    FdKind::PipeWrite { pipe_id } => {
                        let mut pipes = PIPES.lock();
                        if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                            pipe.writer_count = pipe.writer_count.saturating_sub(1);
                        }
                    }
                    _ => {}
                }
            }

            proc.fd_table[newfd] = Some(src_desc);
            newfd
        }

        SYS_READDIR => {
            let path_ptr = arg1;
            let buf_ptr = arg2;
            let max_len = arg3;

            if buf_ptr == 0 || max_len == 0 {
                return usize::MAX;
            }
            if pid != 0 && validate_user_buffer(pml4, buf_ptr, max_len, true).is_err() {
                return usize::MAX;
            }

            let path = match unsafe { read_user_string(pml4, path_ptr as *const u8, 128, pid != 0) } {
                Ok(p) => p,
                Err(_) => return usize::MAX,
            };

            let entries = match crate::fs::vfs::list_dir(&path) {
                Ok(e) => e,
                Err(_) => return usize::MAX,
            };

            let mut out = String::new();
            for e in entries {
                out.push_str(&e.name);
                out.push('\n');
            }

            let bytes = out.as_bytes();
            let to_copy = bytes.len().min(max_len - 1);
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf_ptr as *mut u8, to_copy);
                *((buf_ptr as *mut u8).add(to_copy)) = 0;
            }

            to_copy
        }

        SYS_STAT => {
            let path_ptr = arg1;
            let stat_ptr = arg2;

            if stat_ptr == 0 {
                return usize::MAX;
            }
            if pid != 0 && validate_user_buffer(pml4, stat_ptr, core::mem::size_of::<Stat>(), true).is_err() {
                return usize::MAX;
            }

            let path = match unsafe { read_user_string(pml4, path_ptr as *const u8, 128, pid != 0) } {
                Ok(p) => p,
                Err(_) => return usize::MAX,
            };

            if !crate::fs::vfs::exists(&path) {
                return usize::MAX;
            }

            let is_dir = crate::fs::vfs::is_dir(&path);
            let size = if is_dir {
                0
            } else {
                crate::fs::vfs::read(&path).map(|b| b.len() as u64).unwrap_or(0)
            };

            let stat = Stat {
                size,
                is_dir: if is_dir { 1 } else { 0 },
                _pad: [0; 7],
            };

            unsafe {
                *(stat_ptr as *mut Stat) = stat;
            }
            0
        }

        SYS_GETCWD => {
            let buf_ptr = arg1;
            let max_len = arg2;

            if buf_ptr == 0 || max_len == 0 {
                return usize::MAX;
            }
            if pid != 0 && validate_user_buffer(pml4, buf_ptr, max_len, true).is_err() {
                return usize::MAX;
            }

            let cwd = {
                let table = PROCESS_TABLE.lock();
                table.iter().find(|p| p.pid == pid).map(|p| p.cwd.clone()).unwrap_or_else(|| String::from("/"))
            };

            let bytes = cwd.as_bytes();
            if bytes.len() + 1 > max_len {
                return usize::MAX;
            }

            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf_ptr as *mut u8, bytes.len());
                *((buf_ptr as *mut u8).add(bytes.len())) = 0;
            }
            bytes.len()
        }

        SYS_CHDIR => {
            let path_ptr = arg1;
            let path = match unsafe { read_user_string(pml4, path_ptr as *const u8, 128, pid != 0) } {
                Ok(p) => p,
                Err(_) => return usize::MAX,
            };

            let mut table = PROCESS_TABLE.lock();
            let proc = match table.iter_mut().find(|p| p.pid == pid) {
                Some(p) => p,
                None => return usize::MAX,
            };

            let target = if path.starts_with('/') {
                path
            } else if proc.cwd == "/" {
                format!("/{}", path)
            } else {
                format!("{}/{}", proc.cwd, path)
            };

            let norm = crate::fs::vfs::normalize_path(&target);
            if crate::fs::vfs::is_dir(&norm) {
                proc.cwd = norm;
                0
            } else {
                usize::MAX
            }
        }

        _ => {
            serial_println!("[syscall] warning: unhandled syscall #{number}");
            usize::MAX
        }
    }
}
