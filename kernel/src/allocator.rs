//! Kernel heap: hybrid size-class slab allocator backed by a 32 MiB LockedHeap.
//!
//! Provides fast O(1) allocation for common small objects (32B to 2048B)
//! eliminating heap fragmentation, while falling back to the linked-list
//! allocator for large buffers and framebuffers.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use linked_list_allocator::LockedHeap;
use spin::Mutex;
use x86_64::{PhysAddr, VirtAddr};

use crate::memory::{BumpFrameAllocator, PAGE_SIZE};

/// Kernel heap size: 32 MiB (8192 × 4 KiB frames).
pub const HEAP_SIZE: u64 = 32 * 1024 * 1024;

/// Supported slab size classes in bytes.
pub const SIZE_CLASSES: [usize; 7] = [32, 64, 128, 256, 512, 1024, 2048];

struct FreeNode {
    next: Option<&'static mut FreeNode>,
}

struct SizeClassCache {
    free_list: Option<&'static mut FreeNode>,
    allocated_count: usize,
    active_count: usize,
}

impl SizeClassCache {
    const fn new() -> Self {
        Self {
            free_list: None,
            allocated_count: 0,
            active_count: 0,
        }
    }
}

pub struct SlabAllocator {
    slabs: [Mutex<SizeClassCache>; 7],
    pub fallback: LockedHeap,
}

impl SlabAllocator {
    pub const fn new() -> Self {
        Self {
            slabs: [
                Mutex::new(SizeClassCache::new()),
                Mutex::new(SizeClassCache::new()),
                Mutex::new(SizeClassCache::new()),
                Mutex::new(SizeClassCache::new()),
                Mutex::new(SizeClassCache::new()),
                Mutex::new(SizeClassCache::new()),
                Mutex::new(SizeClassCache::new()),
            ],
            fallback: LockedHeap::empty(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct SlabStat {
    pub size: usize,
    pub allocated: usize,
    pub active: usize,
}

#[global_allocator]
static ALLOCATOR: SlabAllocator = SlabAllocator::new();

unsafe impl GlobalAlloc for SlabAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return layout.align() as *mut u8;
        }

        let size = layout.size();
        let align = layout.align();

        // Check if layout can be served by one of the fixed size classes
        let mut class_idx = None;
        for (i, &sc) in SIZE_CLASSES.iter().enumerate() {
            if size <= sc && align <= sc {
                class_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = class_idx {
            let block_size = SIZE_CLASSES[idx];
            let mut cache = self.slabs[idx].lock();

            if let Some(node) = cache.free_list.take() {
                cache.free_list = node.next.take();
                cache.active_count += 1;
                return node as *mut FreeNode as *mut u8;
            }

            // Allocate a new 4096-byte slab from the underlying heap
            let slab_layout = unsafe { Layout::from_size_align_unchecked(4096, 4096) };
            let slab_ptr = unsafe { self.fallback.alloc(slab_layout) };
            if slab_ptr.is_null() {
                return ptr::null_mut();
            }

            let count = 4096 / block_size;
            cache.allocated_count += count;
            cache.active_count += 1;

            // Link blocks (from index 1 to count - 1)
            let mut head: Option<&'static mut FreeNode> = None;
            for i in (1..count).rev() {
                let block = (slab_ptr as usize + i * block_size) as *mut FreeNode;
                unsafe {
                    (*block).next = head;
                    head = Some(&mut *block);
                }
            }
            cache.free_list = head;

            // Return block 0
            slab_ptr
        } else {
            unsafe { self.fallback.alloc(layout) }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() || layout.size() == 0 {
            return;
        }

        let size = layout.size();
        let align = layout.align();

        let mut class_idx = None;
        for (i, &sc) in SIZE_CLASSES.iter().enumerate() {
            if size <= sc && align <= sc {
                class_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = class_idx {
            let mut cache = self.slabs[idx].lock();
            let node = ptr as *mut FreeNode;
            unsafe {
                (*node).next = cache.free_list.take();
                cache.free_list = Some(&mut *node);
            }
            cache.active_count = cache.active_count.saturating_sub(1);
        } else {
            unsafe { self.fallback.dealloc(ptr, layout) };
        }
    }
}

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

    unsafe {
        ALLOCATOR.fallback.lock().init(
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

/// Bytes the fallback heap can still hand out.
pub fn remaining() -> usize {
    ALLOCATOR.fallback.lock().free()
}

/// Returns telemetry for each slab size class (32B to 2048B).
pub fn slab_stats() -> [SlabStat; 7] {
    let mut stats = [SlabStat {
        size: 0,
        allocated: 0,
        active: 0,
    }; 7];
    for i in 0..7 {
        let guard = ALLOCATOR.slabs[i].lock();
        stats[i] = SlabStat {
            size: SIZE_CLASSES[i],
            allocated: guard.allocated_count,
            active: guard.active_count,
        };
    }
    stats
}

/// Called by `alloc` when the global allocator cannot satisfy a layout.
#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    panic!("kernel heap cannot satisfy allocation: {layout:?}");
}
