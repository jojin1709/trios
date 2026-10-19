//! Userland memory allocator backed by `SYS_MMAP`.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct UserBumpAllocator {
    heap_pos: AtomicUsize,
    heap_end: AtomicUsize,
}

impl UserBumpAllocator {
    pub const fn new() -> Self {
        Self {
            heap_pos: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
        }
    }
}

unsafe impl GlobalAlloc for UserBumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();

        loop {
            let current = self.heap_pos.load(Ordering::Relaxed);
            let end = self.heap_end.load(Ordering::Relaxed);

            let aligned = (current + align - 1) & !(align - 1);
            let next = aligned + size;

            if next <= end && current != 0 {
                if self
                    .heap_pos
                    .compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    return aligned as *mut u8;
                }
            } else {
                // Allocate more pages via SYS_MMAP (at least 64 KiB chunks)
                let chunk_size = ((size + 4095) / 4096 * 4096).max(64 * 1024);
                let new_chunk = unsafe { crate::syscall::syscall4(crate::syscall::SYS_MMAP, 0, chunk_size, 3, 0) };
                if new_chunk == 0 || new_chunk == usize::MAX {
                    return core::ptr::null_mut();
                }
                self.heap_pos.store(new_chunk + size, Ordering::SeqCst);
                self.heap_end.store(new_chunk + chunk_size, Ordering::SeqCst);
                return new_chunk as *mut u8;
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Memory is automatically reclaimed upon process termination
    }
}
