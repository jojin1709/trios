//! Low-level system call wrappers and constants for TriOS Ring 3 userland.

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
pub const SYS_SOCKET: usize = 19;
pub const SYS_CONNECT: usize = 20;
pub const SYS_BIND: usize = 21;
pub const SYS_LISTEN: usize = 22;
pub const SYS_ACCEPT: usize = 23;

pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Stat {
    pub size: u64,
    pub is_dir: u8,
    pub _pad: [u8; 7],
}

#[inline(always)]
pub unsafe fn syscall0(num: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") num,
            lateout("rax") ret,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall1(num: usize, arg1: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") num,
            in("rdi") arg1,
            lateout("rax") ret,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall2(num: usize, arg1: usize, arg2: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") num,
            in("rdi") arg1,
            in("rsi") arg2,
            lateout("rax") ret,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall3(num: usize, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") num,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            lateout("rax") ret,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall4(num: usize, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") num,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            lateout("rax") ret,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    ret
}
