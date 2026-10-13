//! Physical memory bookkeeping — memory-map summary, frame allocation, and a
//! read-only seam over the live page tables.
//!
//! Phase 1 leans on two bootloader-provided facilities:
//!
//! 1. A firmware **memory map** (`BootInfo::memory_regions`): summarised for
//!    the boot log and drained by [`BumpFrameAllocator`].
//! 2. A **direct map** of all physical memory at `physical_memory_offset`
//!    (enabled by `CONFIG` in `main.rs`): turns any physical address into a
//!    usable virtual one — the kernel heap ([`crate::allocator`]) and future
//!    MMIO reach physical memory through it without touching page tables.
//!
//! [`active_level_4_table`] is the deliberate Phase 2 seam: it hands out the
//! current top-level page table for later mapping work (framebuffer, guard
//! pages, per-process address spaces, ...).

use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{FrameAllocator, PageTable, PhysFrame, Size4KiB},
};

/// x86 page size (4 KiB) — the only frame size Phase 1 allocates.
pub const PAGE_SIZE: u64 = 4096;

/// Aggregated view of the firmware memory map (printed at boot).
#[derive(Debug, Clone, Copy)]
pub struct MemoryMapSummary {
    /// Number of regions the bootloader reported.
    pub region_count: usize,
    /// `Usable` bytes — free RAM the kernel may claim.
    pub usable_bytes: u64,
    /// `Bootloader` bytes — reserved by the bootloader, not ours to touch.
    pub bootloader_bytes: u64,
    /// Everything else (kernel image, BIOS/UEFI reserved ranges, ...).
    pub other_bytes: u64,
    /// Highest physical address mentioned by the map (≈ top of RAM).
    pub top_of_ram: u64,
}

/// Collapses the firmware memory map into [`MemoryMapSummary`].
pub fn summarize(regions: &[MemoryRegion]) -> MemoryMapSummary {
    let mut summary = MemoryMapSummary {
        region_count: regions.len(),
        usable_bytes: 0,
        bootloader_bytes: 0,
        other_bytes: 0,
        top_of_ram: 0,
    };
    for region in regions {
        // `end` is exclusive per the bootloader_api docs.
        let len = region.end.saturating_sub(region.start);
        // Match on a reference so this works whether or not the kind is Copy;
        // the catch-all arm also future-proofs against new variants.
        match &region.kind {
            MemoryRegionKind::Usable => summary.usable_bytes += len,
            MemoryRegionKind::Bootloader => summary.bootloader_bytes += len,
            // Kernel image, UnknownBios/UnknownUefi, reserved ranges: keep
            // them visible in the summary, never hand them out.
            _ => summary.other_bytes += len,
        }
        summary.top_of_ram = summary.top_of_ram.max(region.end);
    }
    summary
}

/// Bump-pointer frame allocator over the `Usable` regions of the memory map.
///
/// Deliberately simple (Phase 1): frames are never freed. Two ways to draw
/// from the same cursor:
/// * [`BumpFrameAllocator::allocate_contiguous`] — a run of frames inside one
///   region (used for the heap, which needs physical contiguity);
/// * the [`FrameAllocator`] impl — one frame at a time (page tables and
///   later phases).
///
/// Interleaving a partial contiguous run with one-frame allocations is fine —
/// both simply advance the same cursor — but a contiguous request fails if
/// the *current* region can't satisfy it, even if free frames exist elsewhere.
pub struct BumpFrameAllocator<'a> {
    regions: &'a [MemoryRegion],
    /// Region currently being drained (`regions.len()` when RAM is exhausted).
    index: usize,
    /// Next candidate physical address inside `regions[index]`.
    next: u64,
}

impl<'a> BumpFrameAllocator<'a> {
    /// Positions the cursor at the first usable byte of the map.
    pub fn new(regions: &'a [MemoryRegion]) -> Self {
        let mut allocator = Self {
            regions,
            index: 0,
            next: 0,
        };
        allocator.skip_to_usable();
        allocator
    }

    /// Advances (region, cursor) until the cursor sits inside a `Usable`
    /// region at or after its `start` address. Leaves `index ==
    /// regions.len()` when no usable RAM is left.
    fn skip_to_usable(&mut self) {
        while self.index < self.regions.len() {
            let region = &self.regions[self.index];
            if region.kind == MemoryRegionKind::Usable && self.next < region.end {
                if self.next < region.start {
                    self.next = region.start;
                }
                return;
            }
            self.index += 1;
            self.next = 0;
        }
    }

    /// Allocates `count` contiguous, page-aligned frames **inside a single
    /// usable region** (never straddling a region boundary).
    ///
    /// Returns `None` when no single region has that much space left.
    pub fn allocate_contiguous(&mut self, count: u64) -> Option<PhysAddr> {
        let bytes = count.checked_mul(PAGE_SIZE)?;
        loop {
            self.skip_to_usable();
            if self.index >= self.regions.len() {
                return None;
            }
            let region = &self.regions[self.index];
            // Region starts are page-aligned per UEFI/bootloader guarantees;
            // align the cursor anyway so a stray hole can't yield a
            // misaligned frame (checked arithmetic throughout).
            let start = self.next.checked_add(PAGE_SIZE - 1)? & !(PAGE_SIZE - 1);
            let end = start.checked_add(bytes)?;
            if end <= region.end {
                self.next = end;
                return Some(PhysAddr::new(start));
            }
            // Too big for this region: jump to the next one and retry.
            self.index += 1;
            self.next = 0;
        }
    }
}

/// One frame at a time — for page-table frames and Phase 2+ mapping work.
///
/// # Safety
/// Implementing `FrameAllocator` promises that returned frames are not
/// currently in use. That holds because only `Usable` (free, bootloader-
/// untouched) memory is ever returned, and each byte at most once — the bump
/// cursor never revisits addresses.
unsafe impl FrameAllocator<Size4KiB> for BumpFrameAllocator<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.allocate_contiguous(1)
            .map(PhysFrame::containing_address)
    }
}

/// Returns the active level-4 page table (as `&'static mut`) plus its frame.
///
/// Phase 2 seam: framebuffer mapping, guard pages, and per-process address
/// spaces all start from this reference. The same bytes are also reachable
/// through the direct map — an alias the caller must discipline.
///
/// # Safety
/// The caller must treat the returned table as read-only unless it owns the
/// CPU and follows the x86 paging rules (proper synchronization, TLB
/// invalidation after changes, ...).
pub unsafe fn active_level_4_table(
    physical_memory_offset: VirtAddr,
) -> (&'static mut PageTable, PhysFrame<Size4KiB>) {
    let (frame, _) = Cr3::read();
    let phys = frame.start_address().as_u64();
    // SAFETY:
    //  * `frame` is the top-level table the CPU is walking right now, and no
    //    Phase 1 code frees or replaces it, so it lives for `'static`;
    //  * the direct map covers all physical memory, so offsetting the
    //    frame's start address yields a valid virtual address for it;
    //  * aliasing is documented on this function: the direct-map alias must
    //    not be mutated behind the table's back.
    unsafe {
        let virt = physical_memory_offset + phys;
        (&mut *(virt.as_u64() as *mut PageTable), frame)
    }
}

/// Boot log: which table is active, how many level-4 entries are live.
pub fn log_page_table(physical_memory_offset: VirtAddr) {
    // SAFETY: read-only inspection of the live table (contract above).
    let (level_4, frame) = unsafe { active_level_4_table(physical_memory_offset) };
    let used = level_4.iter().filter(|entry| !entry.is_unused()).count();
    serial_println!(
        "[memory ] page tables: active L4 at phys {:#x} ({used}/512 entries used)",
        frame.start_address().as_u64()
    );
}
