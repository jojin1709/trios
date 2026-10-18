//! Thread representation and thread stack initialization.

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};

use super::context::thread_trampoline;

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
pub const STACK_SIZE: usize = 32 * 1024;

/// A kernel thread representation.
pub struct Thread {
    pub id: ThreadId,
    pub name: String,
    pub state: ThreadState,
    /// Current saved stack pointer (`rsp`).
    pub rsp: usize,
    /// Backing heap allocation for the thread stack (kept alive as long as thread lives).
    #[allow(dead_code)]
    stack: Option<Vec<u8>>,
    /// Backing heap allocation for loaded executable code/data (kept alive until thread dies).
    #[allow(dead_code)]
    image: Option<Vec<u8>>,
}

impl Thread {
    /// Creates the special thread representing the main kernel thread already running.
    pub fn main_thread() -> Self {
        Self {
            id: 0,
            name: String::from("kernel_main"),
            state: ThreadState::Running,
            rsp: 0, // Will be populated on the first context switch away from it
            stack: None, // Uses bootloader kernel stack
            image: None,
        }
    }

    /// Spawns a new kernel thread with the given entry function.
    pub fn new(name: &str, entry: extern "C" fn()) -> Box<Self> {
        Self::new_raw(name, entry as *const () as usize, None)
    }

    /// Spawns a thread for a loaded ELF executable, keeping its backing image buffer alive.
    pub fn new_with_image(name: &str, entry_addr: usize, image: Vec<u8>) -> Box<Self> {
        Self::new_raw(name, entry_addr, Some(image))
    }

    /// Constructs a thread frame pointing to any arbitrary entry address.
    pub fn new_raw(name: &str, entry_addr: usize, image: Option<Vec<u8>>) -> Box<Self> {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed);
        let mut stack = vec![0u8; STACK_SIZE];

        // x86_64 stack grows downward. Top of stack is base + STACK_SIZE.
        let stack_top = stack.as_mut_ptr() as usize + STACK_SIZE;

        // Ensure 16-byte alignment of the stack top
        let aligned_top = stack_top & !0xF;

        // Set up initial stack frame for `switch_context`:
        // When switch_context performs `ret`, it pops the return address.
        // We set that return address to `thread_trampoline`.
        // Then `switch_context` pops `rbp, rbx, r12, r13, r14, r15`.
        // We put `entry` into `r12` so `thread_trampoline` can call it.
        //
        // Stack layout (from high to low addresses):
        // [aligned_top - 8]  : return address (`thread_trampoline`)
        // [aligned_top - 16] : rbp (0)
        // [aligned_top - 24] : rbx (0)
        // [aligned_top - 32] : r12 (entry function pointer)
        // [aligned_top - 40] : r13 (0)
        // [aligned_top - 48] : r14 (0)
        // [aligned_top - 56] : r15 (0)
        //
        // rsp points to [aligned_top - 56]

        let rsp = aligned_top - 56;
        unsafe {
            let stack_ptr = rsp as *mut usize;
            *stack_ptr.add(6) = thread_trampoline as *const () as usize; // Return address for ret
            *stack_ptr.add(5) = 0;                                      // rbp
            *stack_ptr.add(4) = 0;                                      // rbx
            *stack_ptr.add(3) = entry_addr;                             // r12 = function pointer
            *stack_ptr.add(2) = 0;                                      // r13
            *stack_ptr.add(1) = 0;                                      // r14
            *stack_ptr.add(0) = 0;                                      // r15
        }

        Box::new(Self {
            id,
            name: String::from(name),
            state: ThreadState::Ready,
            rsp,
            stack: Some(stack),
            image,
        })
    }
}
