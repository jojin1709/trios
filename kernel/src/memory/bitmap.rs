//! Bitmap physical frame allocator initialized from the UEFI memory map.
//!
//! Provides fast O(1) allocation and deallocation of 4 KiB physical memory frames.

use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
use spin::Mutex;
use x86_64::{
    PhysAddr,
    structures::paging::{FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB},
};

/// Up to 4 GiB of physical memory (1,048,576 frames = 16,384 u64 words = 128 KiB bitmap).
pub const BITMAP_WORDS: usize = 16384;
pub const MAX_FRAMES: usize = BITMAP_WORDS * 64;

/// Frame allocation statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameStats {
    pub total: usize,
    pub allocated: usize,
    pub free: usize,
}

pub struct BitmapFrameAllocator {
    /// Each bit represents one 4 KiB frame: 0 = Free, 1 = Allocated/Reserved.
    bitmap: [u64; BITMAP_WORDS],
    total_usable_frames: usize,
    allocated_frames: usize,
    last_word_idx: usize,
}

impl BitmapFrameAllocator {
    pub const fn empty() -> Self {
        Self {
            bitmap: [!0u64; BITMAP_WORDS], // Default all frames to allocated/reserved
            total_usable_frames: 0,
            allocated_frames: 0,
            last_word_idx: 0,
        }
    }

    /// Initializes the bitmap from UEFI memory regions, reserving the kernel heap.
    pub fn init(&mut self, regions: &[MemoryRegion], heap_phys_start: PhysAddr, heap_size: u64) {
        // Start with all bits set to 1 (all reserved)
        self.bitmap = [!0u64; BITMAP_WORDS];
        self.total_usable_frames = 0;
        self.allocated_frames = 0;
        self.last_word_idx = 0;

        // Mark all Usable memory frames as 0 (Free)
        for region in regions {
            if region.kind == MemoryRegionKind::Usable {
                let start_frame = (region.start / 4096) as usize;
                let end_frame = (region.end / 4096) as usize;

                for f in start_frame..end_frame {
                    // Frame 0 is preserved as reserved (null pointer guard)
                    if f > 0 && f < MAX_FRAMES {
                        let word = f / 64;
                        let bit = f % 64;
                        self.bitmap[word] &= !(1u64 << bit);
                        self.total_usable_frames += 1;
                    }
                }
            }
        }

        // Now mark the pre-allocated kernel heap region as 1 (Allocated)
        let heap_start_f = (heap_phys_start.as_u64() / 4096) as usize;
        let heap_end_f = ((heap_phys_start.as_u64() + heap_size + 4095) / 4096) as usize;
        for f in heap_start_f..heap_end_f {
            if f < MAX_FRAMES {
                let word = f / 64;
                let bit = f % 64;
                if (self.bitmap[word] & (1u64 << bit)) == 0 {
                    self.bitmap[word] |= 1u64 << bit;
                    self.allocated_frames += 1;
                }
            }
        }
    }

    /// Allocates a single 4 KiB physical frame.
    pub fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        for offset in 0..BITMAP_WORDS {
            let idx = (self.last_word_idx + offset) % BITMAP_WORDS;
            let word = self.bitmap[idx];
            if word != !0u64 {
                let free_bit = (!word).trailing_zeros() as usize;
                let frame_idx = idx * 64 + free_bit;
                if frame_idx >= MAX_FRAMES {
                    return None;
                }
                self.bitmap[idx] |= 1u64 << free_bit;
                self.allocated_frames += 1;
                self.last_word_idx = idx;
                let phys = PhysAddr::new((frame_idx as u64) * 4096);
                return Some(PhysFrame::containing_address(phys));
            }
        }
        None
    }

    /// Frees a previously allocated 4 KiB physical frame.
    pub fn free_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        let phys = frame.start_address().as_u64();
        let frame_idx = (phys / 4096) as usize;
        if frame_idx < MAX_FRAMES && frame_idx > 0 {
            let word = frame_idx / 64;
            let bit = frame_idx % 64;
            if (self.bitmap[word] & (1u64 << bit)) != 0 {
                self.bitmap[word] &= !(1u64 << bit);
                self.allocated_frames = self.allocated_frames.saturating_sub(1);
                if word < self.last_word_idx {
                    self.last_word_idx = word;
                }
            }
        }
    }

    /// Allocates `count` contiguous 4 KiB physical frames.
    #[allow(dead_code)]
    pub fn allocate_contiguous(&mut self, count: usize) -> Option<PhysFrame<Size4KiB>> {
        if count == 0 {
            return None;
        }
        if count == 1 {
            return self.allocate_frame();
        }

        let mut run_start = 0;
        let mut run_len = 0;

        for frame_idx in 1..MAX_FRAMES {
            let word = frame_idx / 64;
            let bit = frame_idx % 64;
            let is_allocated = (self.bitmap[word] & (1u64 << bit)) != 0;

            if !is_allocated {
                if run_len == 0 {
                    run_start = frame_idx;
                }
                run_len += 1;
                if run_len == count {
                    // Mark all frames in the run as allocated
                    for f in run_start..(run_start + count) {
                        self.bitmap[f / 64] |= 1u64 << (f % 64);
                    }
                    self.allocated_frames += count;
                    let phys = PhysAddr::new((run_start as u64) * 4096);
                    return Some(PhysFrame::containing_address(phys));
                }
            } else {
                run_len = 0;
            }
        }
        None
    }

    /// Frees `count` contiguous 4 KiB physical frames.
    #[allow(dead_code)]
    pub fn free_contiguous(&mut self, frame: PhysFrame<Size4KiB>, count: usize) {
        let start_f = (frame.start_address().as_u64() / 4096) as usize;
        for f in start_f..(start_f + count) {
            if f < MAX_FRAMES && f > 0 {
                let word = f / 64;
                let bit = f % 64;
                if (self.bitmap[word] & (1u64 << bit)) != 0 {
                    self.bitmap[word] &= !(1u64 << bit);
                    self.allocated_frames = self.allocated_frames.saturating_sub(1);
                }
            }
        }
    }

    /// Returns current frame allocation metrics.
    pub fn stats(&self) -> FrameStats {
        FrameStats {
            total: self.total_usable_frames,
            allocated: self.allocated_frames,
            free: self.total_usable_frames.saturating_sub(self.allocated_frames),
        }
    }
}

unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.allocate_frame()
    }
}

impl FrameDeallocator<Size4KiB> for BitmapFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        self.free_frame(frame);
    }
}

static BITMAP_ALLOCATOR: Mutex<BitmapFrameAllocator> = Mutex::new(BitmapFrameAllocator::empty());

/// Initializes the global bitmap frame allocator.
pub fn init(regions: &[MemoryRegion], heap_phys_start: PhysAddr, heap_size: u64) {
    BITMAP_ALLOCATOR.lock().init(regions, heap_phys_start, heap_size);
}

/// Allocates a single physical frame from the global bitmap allocator.
pub fn alloc_frame() -> Option<PhysFrame<Size4KiB>> {
    BITMAP_ALLOCATOR.lock().allocate_frame()
}

/// Frees a single physical frame back to the global bitmap allocator.
pub fn free_frame(frame: PhysFrame<Size4KiB>) {
    BITMAP_ALLOCATOR.lock().free_frame(frame);
}

/// Allocates `count` contiguous physical frames.
#[allow(dead_code)]
pub fn alloc_contiguous(count: usize) -> Option<PhysFrame<Size4KiB>> {
    BITMAP_ALLOCATOR.lock().allocate_contiguous(count)
}

/// Frees `count` contiguous physical frames.
#[allow(dead_code)]
pub fn free_contiguous(frame: PhysFrame<Size4KiB>, count: usize) {
    BITMAP_ALLOCATOR.lock().free_contiguous(frame, count);
}

/// Returns snapshot of physical frame statistics.
pub fn stats() -> FrameStats {
    BITMAP_ALLOCATOR.lock().stats()
}
