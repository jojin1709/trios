//! Thread representation and thread stack initialization with guard pages.

use alloc::{boxed::Box, string::String, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};

use super::context::thread_trampoline;
use crate::memory::KernelStack;

/// Unique thread identifier.
pub type ThreadId = u64;

static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// Execution state of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Ready to be scheduled.
    Ready,
    /// Currently running on the CPU.
    Running,
    /// Sleeping until a specific PIT tick counter.
    Sleeping(u64),
    /// Terminated, awaiting stack deallocation.
    Dead,
}

/// 32 KiB default stack size per kernel thread (8 pages).
#[allow(dead_code)]
pub const STACK_SIZE: usize = 32 * 1024;

/// A kernel thread representation.
pub struct Thread {
    pub id: ThreadId,
    pub name: String,
    pub state: ThreadState,
    pub pid: u64,
    /// Current saved stack pointer (`rsp`).
    pub rsp: usize,
    /// Kernel thread stack equipped with an unmapped guard page at the bottom.
    pub kstack: Option<KernelStack>,
    /// Backing heap allocation for loaded executable code/data (kept alive until thread dies).
    #[allow(dead_code)]
    image: Option<Vec<u8>>,
}

impl Drop for Thread {
    fn drop(&mut self) {
        if let Some(ref kstack) = self.kstack {
            crate::memory::free_kernel_stack(kstack);
        }
    }
}

impl Thread {
    /// Creates the special thread representing the main kernel thread already running.
    pub fn main_thread() -> Self {
        Self {
            id: 0,
            name: String::from("kernel_main"),
            state: ThreadState::Running,
            pid: 0,
            rsp: 0, // Populated on the first context switch away from it
            kstack: None, // Uses bootloader kernel stack
            image: None,
        }
    }

    /// Spawns a new kernel thread with the given entry function.
    pub fn new(name: &str, entry: extern "C" fn()) -> Box<Self> {
        Self::new_raw(name, entry as *const () as usize, 0, None, 0)
    }

    /// Spawns a new kernel thread with an argument passed in RDI.
    pub fn new_with_arg(name: &str, entry: extern "C" fn(usize), arg: usize) -> Box<Self> {
        Self::new_raw(name, entry as *const () as usize, arg, None, 0)
    }

    /// Spawns a thread for a user process with PID tracking.
    pub fn new_process_thread(name: &str, entry: extern "C" fn(usize), pid: u64) -> Box<Self> {
        Self::new_raw(name, entry as *const () as usize, pid as usize, None, pid)
    }

    /// Spawns a thread for a loaded ELF executable, keeping its backing image buffer alive.
    #[allow(dead_code)]
    pub fn new_with_image(name: &str, entry_addr: usize, image: Vec<u8>) -> Box<Self> {
        Self::new_raw(name, entry_addr, 0, Some(image), 0)
    }

    /// Constructs a thread frame pointing to any arbitrary entry address with an argument in R13.
    pub fn new_raw(name: &str, entry_addr: usize, arg: usize, image: Option<Vec<u8>>, pid: u64) -> Box<Self> {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed);

        // Allocate 32 KiB (8 pages) kernel stack with a preceding unmapped guard page
        let kstack = crate::memory::alloc_kernel_stack(crate::memory::virt::STACK_PAGES)
            .expect("failed to allocate thread stack with guard page");

        let stack_top = kstack.stack_top.as_u64() as usize;

        // Ensure 16-byte alignment of the stack top
        let aligned_top = stack_top & !0xF;

        // Set up initial stack frame for `switch_context`:
        // When switch_context performs `ret`, it pops the return address (`thread_trampoline`).
        // Then `switch_context` pops `rbp, rbx, r12, r13, r14, r15`.
        // We put `entry` into `r12` and `arg` into `r13` so `thread_trampoline` can call it with RDI=arg.
        //
        // Stack layout (from high to low addresses):
        // [aligned_top - 8]  : return address (`thread_trampoline`)
        // [aligned_top - 16] : rbp (0)
        // [aligned_top - 24] : rbx (0)
        // [aligned_top - 32] : r12 (entry function pointer)
        // [aligned_top - 40] : r13 (arg)
        // [aligned_top - 48] : r14 (0)
        // [aligned_top - 56] : r15 (0)
        let rsp = aligned_top - 56;
        unsafe {
            let stack_ptr = rsp as *mut usize;
            *stack_ptr.add(6) = thread_trampoline as *const () as usize; // Return address for ret
            *stack_ptr.add(5) = 0;                                      // rbp
            *stack_ptr.add(4) = 0;                                      // rbx
            *stack_ptr.add(3) = entry_addr;                             // r12 = function pointer
            *stack_ptr.add(2) = arg;                                    // r13 = argument
            *stack_ptr.add(1) = 0;                                      // r14
            *stack_ptr.add(0) = 0;                                      // r15
        }

        Box::new(Self {
            id,
            name: String::from(name),
            state: ThreadState::Ready,
            pid,
            rsp,
            kstack: Some(kstack),
            image,
        })
    }
}
