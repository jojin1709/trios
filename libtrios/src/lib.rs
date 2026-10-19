#![no_std]

extern crate alloc;

pub mod allocator;
pub mod io;
pub mod syscall;

pub use allocator::UserBumpAllocator;
pub use syscall::*;

#[global_allocator]
pub static ALLOCATOR: UserBumpAllocator = UserBumpAllocator::new();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    eprintln!("[panic] {info}");
    exit(1);
}

/// Terminates the calling process with `code`.
pub fn exit(code: i32) -> ! {
    unsafe {
        syscall1(SYS_EXIT, code as usize);
        loop {
            core::arch::asm!("pause");
        }
    }
}

/// Writes `buf` to file descriptor `fd`. Returns number of bytes written or -1 on error.
pub fn write(fd: usize, buf: &[u8]) -> isize {
    let res = unsafe { syscall3(SYS_WRITE, fd, buf.as_ptr() as usize, buf.len()) };
    res as isize
}

/// Reads up to `buf.len()` bytes from file descriptor `fd`. Returns bytes read or -1 on error.
pub fn read(fd: usize, buf: &mut [u8]) -> isize {
    let res = unsafe { syscall3(SYS_READ, fd, buf.as_mut_ptr() as usize, buf.len()) };
    res as isize
}

/// Opens `path` with `flags` (0 = read, 1 = write/create). Returns file descriptor or -1 on error.
pub fn open(path: &str, flags: usize) -> isize {
    let mut path_buf = [0u8; 128];
    if path.len() >= 127 {
        return -1;
    }
    path_buf[..path.len()].copy_from_slice(path.as_bytes());
    path_buf[path.len()] = 0;
    let res = unsafe { syscall2(SYS_OPEN, path_buf.as_ptr() as usize, flags) };
    res as isize
}

/// Closes file descriptor `fd`. Returns 0 on success or -1 on error.
pub fn close(fd: usize) -> isize {
    let res = unsafe { syscall1(SYS_CLOSE, fd) };
    res as isize
}

/// Returns the calling process ID.
pub fn getpid() -> usize {
    unsafe { syscall0(SYS_GETPID) }
}

/// Puts the calling process to sleep for `ms` milliseconds.
pub fn sleep_ms(ms: usize) -> isize {
    let res = unsafe { syscall1(SYS_SLEEP_MS, ms) };
    res as isize
}

/// Voluntarily yields CPU execution to another ready process/thread.
pub fn yield_now() {
    unsafe {
        syscall0(SYS_YIELD);
    }
}

/// Maps `len` bytes of user virtual memory. Returns start virtual address or 0 on error.
pub fn mmap(addr: usize, len: usize, prot: usize, flags: usize) -> usize {
    unsafe { syscall4(SYS_MMAP, addr, len, prot, flags) }
}

/// Unmaps user virtual memory.
pub fn munmap(addr: usize, len: usize) -> isize {
    let res = unsafe { syscall2(SYS_MUNMAP, addr, len) };
    res as isize
}

/// Returns system uptime in seconds.
pub fn uptime() -> usize {
    unsafe { syscall0(SYS_UPTIME) }
}

/// Spawns a new executable process with arguments. Returns child PID or -1 on error.
pub fn spawn(path: &str, args: &[&str]) -> isize {
    let mut path_buf = [0u8; 128];
    if path.len() >= 127 {
        return -1;
    }
    path_buf[..path.len()].copy_from_slice(path.as_bytes());
    path_buf[path.len()] = 0;

    let mut arg_storage = [[0u8; 64]; 16];
    let mut argv_ptrs = [core::ptr::null::<u8>(); 17];
    let count = args.len().min(16);
    for i in 0..count {
        let a = args[i];
        let len = a.len().min(63);
        arg_storage[i][..len].copy_from_slice(&a.as_bytes()[..len]);
        arg_storage[i][len] = 0;
        argv_ptrs[i] = arg_storage[i].as_ptr();
    }
    argv_ptrs[count] = core::ptr::null();

    let res = unsafe {
        syscall2(
            SYS_SPAWN,
            path_buf.as_ptr() as usize,
            argv_ptrs.as_ptr() as usize,
        )
    };
    res as isize
}

/// Waits for child process `pid` to exit (or any child if `pid == 0`). Returns exit code.
pub fn wait(pid: usize) -> i32 {
    let res = unsafe { syscall1(SYS_WAIT, pid) };
    res as i32
}

/// Creates a unidirectional pipe. Stores `[read_fd, write_fd]` in `fds`. Returns 0 on success.
pub fn pipe(fds: &mut [i32; 2]) -> isize {
    let res = unsafe { syscall1(SYS_PIPE, fds.as_mut_ptr() as usize) };
    res as isize
}

/// Duplicates `oldfd` onto `newfd`. Returns `newfd` or -1 on error.
pub fn dup2(oldfd: usize, newfd: usize) -> isize {
    let res = unsafe { syscall2(SYS_DUP2, oldfd, newfd) };
    res as isize
}

/// Reads directory entries from `path` into `buf` as newline-separated strings.
pub fn readdir(path: &str, buf: &mut [u8]) -> isize {
    let mut path_buf = [0u8; 128];
    if path.len() >= 127 {
        return -1;
    }
    path_buf[..path.len()].copy_from_slice(path.as_bytes());
    path_buf[path.len()] = 0;
    let res = unsafe {
        syscall3(
            SYS_READDIR,
            path_buf.as_ptr() as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
        )
    };
    res as isize
}

/// Retrieves file/directory metadata. Returns 0 on success or -1 on error.
pub fn stat(path: &str, stat_buf: &mut Stat) -> isize {
    let mut path_buf = [0u8; 128];
    if path.len() >= 127 {
        return -1;
    }
    path_buf[..path.len()].copy_from_slice(path.as_bytes());
    path_buf[path.len()] = 0;
    let res = unsafe {
        syscall2(
            SYS_STAT,
            path_buf.as_ptr() as usize,
            stat_buf as *mut Stat as usize,
        )
    };
    res as isize
}

/// Retrieves current working directory into `buf`. Returns length of cwd or -1.
pub fn getcwd(buf: &mut [u8]) -> isize {
    let res = unsafe { syscall2(SYS_GETCWD, buf.as_mut_ptr() as usize, buf.len()) };
    res as isize
}

/// Changes the current working directory to `path`. Returns 0 on success or -1.
pub fn chdir(path: &str) -> isize {
    let mut path_buf = [0u8; 128];
    if path.len() >= 127 {
        return -1;
    }
    path_buf[..path.len()].copy_from_slice(path.as_bytes());
    path_buf[path.len()] = 0;
    let res = unsafe { syscall1(SYS_CHDIR, path_buf.as_ptr() as usize) };
    res as isize
}

/// Helper trampoline to parse argc/argv and call application main.
pub unsafe fn entry_trampoline(argc: usize, argv: *const *const u8, f: fn(&[&str]) -> i32) -> ! {
    let mut args_buf = [""; 16];
    let count = argc.min(16);
    if !argv.is_null() {
        for i in 0..count {
            let ptr = unsafe { *argv.add(i) };
            if !ptr.is_null() {
                let mut len = 0;
                while unsafe { *ptr.add(len) } != 0 {
                    len += 1;
                }
                let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
                if let Ok(s) = core::str::from_utf8(slice) {
                    args_buf[i] = s;
                }
            }
        }
    }
    let code = f(&args_buf[..count]);
    exit(code);
}

/// Declares the main entry point for a TriOS userland binary.
#[macro_export]
macro_rules! trios_main {
    ($path:path) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn _start(argc: usize, argv: *const *const u8) -> ! {
            $crate::entry_trampoline(argc, argv, $path)
        }
    };
}
