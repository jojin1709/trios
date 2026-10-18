//! Kernel heap: frames from [`crate::memory::BumpFrameAllocator`], reached
//! through the bootloader's direct physical-memory map.
//!
//! Why the direct map? `CONFIG` in `main.rs` asks the bootloader to map *all*
//! physical memory at `physical_memory_offset`. Taking contiguous usable
//! frames for the heap and adding the offset yields a contiguous, already
//! mapped, writable virtual range — no page-table surgery needed in Phase 1.
//!
//! Phase 2 seam: [`init`] returns [`HeapInfo`] so later phases can carve more
//! regions (per-process heaps, slab caches) with the same machinery.

use core::alloc::Layout;
use core::ptr;

use linked_list_allocator::LockedHeap;
use x86_64::{PhysAddr, VirtAddr};

use crate::memory::{BumpFrameAllocator, PAGE_SIZE};

/// Kernel heap size: 32 MiB (8192 × 4 KiB frames).
///
/// Expanded for 1920×1080 double-buffered graphics backbuffer (~8.3 MiB)
/// plus the cached background wallpaper buffer (~8.3 MiB).
pub const HEAP_SIZE: u64 = 32 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Where the heap landed — logged at boot, reused by later phases.
#[derive(Debug, Clone, Copy)]
pub struct HeapInfo {
    pub virt_start: VirtAddr,
    pub phys_start: PhysAddr,
    pub size: u64,
}

/// Carves `size` bytes of contiguous usable physical memory, exposes them at
/// the direct-map virtual address, and installs that range as the global
/// allocator's heap.
///
/// # Panics
/// - if the memory map has no contiguous `size`-byte usable run, or
/// - if the resulting range would exceed what the direct map covers.
pub fn init(
    frame_allocator: &mut BumpFrameAllocator<'_>,
    physical_memory_offset: VirtAddr,
    size: u64,
) -> HeapInfo {
    let frames = size.div_ceil(PAGE_SIZE);
    let phys_start = frame_allocator
        .allocate_contiguous(frames)
        .expect("not enough contiguous usable RAM for the kernel heap");
    let virt_start = physical_memory_offset + phys_start.as_u64();

    // SAFETY:
    //  * `phys_start..phys_start + size` lies inside a `Usable` memory-map
    //    region: real, free RAM the bootloader never touched, and handed out
    //    only here (no aliasing with other allocations);
    //  * the direct map covers all physical memory as writable, so the virtual
    //    range is valid and writable for the `'static` lifetime the allocator
    //    requires;
    //  * `init` runs exactly once, on an empty heap (`LockedHeap::empty()`),
    //    and the whole range stays mapped for the rest of the program.
    unsafe {
        ALLOCATOR.lock().init(
            ptr::with_exposed_provenance_mut::<u8>(virt_start.as_u64() as usize),
            size as usize,
        );
    }

    HeapInfo {
        virt_start,
        phys_start,
        size,
    }
}

/// Bytes the heap can still hand out (diagnostic for the boot log).
pub fn remaining() -> usize {
    ALLOCATOR.lock().free()
}

/// Called by `alloc` when the global allocator cannot satisfy a layout.
#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    panic!("kernel heap cannot satisfy allocation: {layout:?}");
}
