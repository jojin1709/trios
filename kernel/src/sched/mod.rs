//! Preemptive and cooperative multitasking scheduler.
//!
//! Provides round-robin scheduling of kernel threads, thread spawning,
//! sleeping, yielding, and automatic cleanup of terminated threads.

use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;

pub mod context;
pub mod thread;

use context::switch_context;
pub use thread::{Thread, ThreadId, ThreadState};

/// The global scheduler instance.
pub static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Global flag indicating if the scheduler has been initialized.
static SCHEDULER_ENABLED: AtomicBool = AtomicBool::new(false);

/// Round-robin kernel thread scheduler.
pub struct Scheduler {
    /// Currently running thread on this CPU core.
    current: Box<Thread>,
    /// Queue of threads ready to run.
    ready_queue: VecDeque<Box<Thread>>,
    /// List of threads sleeping until a given timer tick.
    sleep_queue: Vec<Box<Thread>>,
    /// Dead threads waiting for deallocation.
    dead_queue: Vec<Box<Thread>>,
}

impl Scheduler {
    fn new() -> Self {
        Self {
            current: Box::new(Thread::main_thread()),
            ready_queue: VecDeque::new(),
            sleep_queue: Vec::new(),
            dead_queue: Vec::new(),
        }
    }

    /// Appends a new thread to the ready queue.
    pub fn spawn(&mut self, thread: Box<Thread>) -> ThreadId {
        let id = thread.id;
        let name = thread.name.clone();
        self.ready_queue.push_back(thread);
        serial_println!("[sched  ] spawned thread #{id} ('{name}')");
        id
    }

    /// Wakes up any sleeping threads whose deadline has arrived.
    pub fn check_sleepers(&mut self, current_ticks: u64) {
        let mut i = 0;
        while i < self.sleep_queue.len() {
            let ready = match self.sleep_queue[i].state {
                ThreadState::Sleeping(wake_tick) => current_ticks >= wake_tick,
                _ => true,
            };
            if ready {
                let mut thread = self.sleep_queue.swap_remove(i);
                thread.state = ThreadState::Ready;
                self.ready_queue.push_back(thread);
            } else {
                i += 1;
            }
        }
    }

    /// Selects the next ready thread to execute.
    fn next_thread(&mut self) -> Option<Box<Thread>> {
        self.ready_queue.pop_front()
    }
}

/// Initializes the global scheduler.
pub fn init() {
    let mut lock = SCHEDULER.lock();
    *lock = Some(Scheduler::new());
    SCHEDULER_ENABLED.store(true, Ordering::SeqCst);
    serial_println!("[sched  ] multitasking scheduler initialized");
}

/// Returns whether the scheduler is initialized and active.
pub fn is_enabled() -> bool {
    SCHEDULER_ENABLED.load(Ordering::Relaxed)
}

/// Spawns a new kernel thread to run concurrently.
pub fn spawn(name: &str, entry: extern "C" fn()) -> ThreadId {
    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        let sched = lock.as_mut().expect("scheduler not initialized");
        let t = Thread::new(name, entry);
        sched.spawn(t)
    })
}

/// Spawns a new thread executing a loaded ELF binary with a backing image buffer.
pub fn spawn_elf(name: &str, entry_addr: usize, image: Vec<u8>) -> ThreadId {
    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        let sched = lock.as_mut().expect("scheduler not initialized");
        let t = Thread::new_with_image(name, entry_addr, image);
        sched.spawn(t)
    })
}

/// Yields CPU execution to the next ready thread.
pub fn yield_now() {
    switch(false, None);
}

/// Puts the calling thread to sleep for at least `ticks` timer ticks (1 tick = 10 ms at 100 Hz).
pub fn sleep_ticks(ticks: u64) {
    let wake_at = crate::timer::ticks().wrapping_add(ticks);
    switch(false, Some(wake_at));
}

/// Helper called when a thread function finishes execution.
pub extern "C" fn thread_exit() -> ! {
    switch(true, None);
    // Should never reach here
    loop {
        x86_64::instructions::hlt();
    }
}

/// Core switch function. Must be called with interrupts disabled or will disable them internally.
fn switch(exit_current: bool, sleep_until: Option<u64>) {
    let mut prev_rsp_ptr: *mut usize = core::ptr::null_mut();
    let mut next_rsp: usize = 0;

    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        let sched = match lock.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Clean up previously dead threads
        sched.dead_queue.clear();

        // Check if there is another thread ready to run
        let next_thread = match sched.next_thread() {
            Some(t) => t,
            None => {
                // No other threads ready to run.
                // If current thread is exiting or sleeping, we can't continue running it.
                if exit_current || sleep_until.is_some() {
                    // Nothing to switch to, stay in current or spin
                    return;
                }
                return;
            }
        };

        // Prepare current thread state
        let mut current = core::mem::replace(&mut sched.current, next_thread);
        prev_rsp_ptr = &mut current.rsp as *mut usize;

        if exit_current {
            current.state = ThreadState::Dead;
            sched.dead_queue.push(current);
        } else if let Some(wake_tick) = sleep_until {
            current.state = ThreadState::Sleeping(wake_tick);
            sched.sleep_queue.push(current);
        } else {
            current.state = ThreadState::Ready;
            sched.ready_queue.push_back(current);
        }

        // Set next thread state
        sched.current.state = ThreadState::Running;
        next_rsp = sched.current.rsp;
    });

    if !prev_rsp_ptr.is_null() && next_rsp != 0 {
        unsafe {
            switch_context(prev_rsp_ptr, next_rsp);
        }
    }
}

/// Called from the PIT timer interrupt to check sleeping threads and preempt if appropriate.
pub fn timer_tick(current_ticks: u64) {
    if !is_enabled() {
        return;
    }

    // Attempt to lock scheduler without deadlocking in interrupt handler
    if let Some(mut guard) = SCHEDULER.try_lock()
        && let Some(sched) = guard.as_mut()
    {
        sched.check_sleepers(current_ticks);
    }
}
