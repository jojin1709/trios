//! Kernel Virtual Memory API and Guard Page Stack Allocation.
//!
//! Provides functions to map, unmap, allocate, and free virtual 4 KiB pages,
//! as well as kernel thread stacks equipped with non-present guard pages.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::{
    VirtAddr,
    instructions::tlb,
    structures::paging::{Page, PageTable, PageTableFlags, PhysFrame, Size4KiB},
};

use super::bitmap;

/// Architecture-independent permission flags for virtual memory mappings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PageFlags {
    pub writable: bool,
    pub executable: bool,
    pub user: bool,
}

#[allow(dead_code)]
impl PageFlags {
    pub const KERNEL_DATA: Self = Self {
        writable: true,
        executable: false,
        user: false,
    };
    pub const KERNEL_RO: Self = Self {
        writable: false,
        executable: false,
        user: false,
    };
    pub const KERNEL_CODE: Self = Self {
        writable: false,
        executable: true,
        user: false,
    };
    pub const USER_DATA: Self = Self {
        writable: true,
        executable: false,
        user: true,
    };
    pub const USER_CODE: Self = Self {
        writable: true,
        executable: true,
        user: true,
    };

    pub fn to_x86_flags(&self) -> PageTableFlags {
        let mut flags = PageTableFlags::PRESENT;
        if self.writable {
            flags |= PageTableFlags::WRITABLE;
        }
        if !self.executable {
            flags |= PageTableFlags::NO_EXECUTE;
        }
        if self.user {
            flags |= PageTableFlags::USER_ACCESSIBLE;
        }
        flags
    }
}

/// Maps a single 4 KiB virtual page to a physical frame in the active page table.
pub fn map_page(
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    flags: PageFlags,
) -> Result<(), &'static str> {
    let phys_offset = super::get_phys_offset().ok_or("phys offset uninitialized")?;
    let (l4, _) = unsafe { super::active_level_4_table(phys_offset) };

    let p4_idx = page.start_address().p4_index();
    let l4_entry = &mut l4[p4_idx];

    if l4_entry.is_unused() {
        let new_frame = bitmap::alloc_frame().ok_or("out of physical frames for L3 table")?;
        unsafe {
            let ptr = (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
            core::ptr::write_bytes(ptr, 0, 1);
        }
        l4_entry.set_addr(
            new_frame.start_address(),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
    }

    let l3_addr = phys_offset + l4_entry.addr().as_u64();
    let l3 = unsafe { &mut *l3_addr.as_mut_ptr::<PageTable>() };
    let p3_idx = page.start_address().p3_index();
    let l3_entry = &mut l3[p3_idx];

    if l3_entry.is_unused() {
        let new_frame = bitmap::alloc_frame().ok_or("out of physical frames for L2 table")?;
        unsafe {
            let ptr = (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
            core::ptr::write_bytes(ptr, 0, 1);
        }
        l3_entry.set_addr(
            new_frame.start_address(),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
    }

    let l2_addr = phys_offset + l3_entry.addr().as_u64();
    let l2 = unsafe { &mut *l2_addr.as_mut_ptr::<PageTable>() };
    let p2_idx = page.start_address().p2_index();
    let l2_entry = &mut l2[p2_idx];

    if l2_entry.is_unused() {
        let new_frame = bitmap::alloc_frame().ok_or("out of physical frames for L1 table")?;
        unsafe {
            let ptr = (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
            core::ptr::write_bytes(ptr, 0, 1);
        }
        l2_entry.set_addr(
            new_frame.start_address(),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
    }

    let l1_addr = phys_offset + l2_entry.addr().as_u64();
    let l1 = unsafe { &mut *l1_addr.as_mut_ptr::<PageTable>() };
    let p1_idx = page.start_address().p1_index();
    let l1_entry = &mut l1[p1_idx];

    l1_entry.set_addr(frame.start_address(), flags.to_x86_flags());
    tlb::flush(page.start_address());

    Ok(())
}

/// Unmaps a virtual page from the active page table, returning its previously mapped physical frame.
pub fn unmap_page(page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, &'static str> {
    let phys_offset = super::get_phys_offset().ok_or("phys offset uninitialized")?;
    let (l4, _) = unsafe { super::active_level_4_table(phys_offset) };

    let l4_entry = &l4[page.start_address().p4_index()];
    if l4_entry.is_unused() {
        return Err("page table L4 entry not present");
    }

    let l3_addr = phys_offset + l4_entry.addr().as_u64();
    let l3 = unsafe { &*l3_addr.as_ptr::<PageTable>() };
    let l3_entry = &l3[page.start_address().p3_index()];
    if l3_entry.is_unused() {
        return Err("page table L3 entry not present");
    }

    let l2_addr = phys_offset + l3_entry.addr().as_u64();
    let l2 = unsafe { &*l2_addr.as_ptr::<PageTable>() };
    let l2_entry = &l2[page.start_address().p2_index()];
    if l2_entry.is_unused() {
        return Err("page table L2 entry not present");
    }

    let l1_addr = phys_offset + l2_entry.addr().as_u64();
    let l1 = unsafe { &mut *l1_addr.as_mut_ptr::<PageTable>() };
    let l1_entry = &mut l1[page.start_address().p1_index()];

    if l1_entry.is_unused() {
        return Err("page is not mapped in L1 table");
    }

    let frame = PhysFrame::containing_address(l1_entry.addr());
    l1_entry.set_unused();
    tlb::flush(page.start_address());

    Ok(frame)
}

/// Allocates and maps `count` consecutive virtual pages starting at `start_page`.
pub fn alloc_pages(
    start_page: Page<Size4KiB>,
    count: usize,
    flags: PageFlags,
) -> Result<(), &'static str> {
    for i in 0..count {
        let page = Page::containing_address(start_page.start_address() + (i as u64 * 4096));
        let frame = bitmap::alloc_frame().ok_or("out of physical memory frames")?;
        if let Err(err) = map_page(page, frame, flags) {
            // Rollback already mapped pages
            bitmap::free_frame(frame);
            for j in 0..i {
                let rollback_page =
                    Page::containing_address(start_page.start_address() + (j as u64 * 4096));
                if let Ok(f) = unmap_page(rollback_page) {
                    bitmap::free_frame(f);
                }
            }
            return Err(err);
        }
    }
    Ok(())
}

/// Unmaps and frees `count` consecutive virtual pages starting at `start_page`.
pub fn free_pages(start_page: Page<Size4KiB>, count: usize) -> Result<(), &'static str> {
    for i in 0..count {
        let page = Page::containing_address(start_page.start_address() + (i as u64 * 4096));
        let frame = unmap_page(page)?;
        bitmap::free_frame(frame);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Guard Page Kernel Thread Stacks
// ---------------------------------------------------------------------------

pub const STACK_PAGES: usize = 8; // 32 KiB stack
pub const STACK_SLOT_PAGES: usize = STACK_PAGES + 2; // Guard page (1) + Stack (8) + Gap (1) = 10 pages (40 KiB)
pub const STACK_VIRT_BASE: u64 = 0xFFFF_9000_0000_0000;
pub const STACK_VIRT_LIMIT: u64 = 0xFFFF_9000_4000_0000;

static NEXT_STACK_ADDR: AtomicU64 = AtomicU64::new(STACK_VIRT_BASE);

/// A dynamically allocated kernel thread stack with an unmapped guard page at the bottom.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct KernelStack {
    /// Unmapped guard page immediately below the stack bottom.
    pub guard_page: VirtAddr,
    /// Lowest mapped byte of the stack.
    pub stack_bottom: VirtAddr,
    /// Top of the stack (initial RSP target).
    pub stack_top: VirtAddr,
    /// Number of mapped 4 KiB pages (excluding guard page).
    pub page_count: usize,
}

/// Allocates a new kernel stack of `page_count` pages preceded by an unmapped guard page.
pub fn alloc_kernel_stack(page_count: usize) -> Result<KernelStack, &'static str> {
    let slot_pages = page_count + 2;
    let slot_bytes = (slot_pages as u64) * 4096;
    let base = NEXT_STACK_ADDR.fetch_add(slot_bytes, Ordering::Relaxed);

    if base >= STACK_VIRT_LIMIT {
        return Err("exhausted kernel thread stack virtual address space");
    }

    let guard_page = VirtAddr::new(base);
    let stack_bottom = VirtAddr::new(base + 4096);
    let stack_top = VirtAddr::new(base + 4096 + ((page_count as u64) * 4096));

    // The guard page at `guard_page` is deliberately never mapped.
    // Map the actual stack pages:
    let start_page = Page::containing_address(stack_bottom);
    alloc_pages(start_page, page_count, PageFlags::KERNEL_DATA)?;

    Ok(KernelStack {
        guard_page,
        stack_bottom,
        stack_top,
        page_count,
    })
}

/// Frees the mapped pages of a kernel stack.
pub fn free_kernel_stack(stack: &KernelStack) {
    let start_page = Page::containing_address(stack.stack_bottom);
    let _ = free_pages(start_page, stack.page_count);
}

/// Checks if a faulting virtual address falls within a kernel thread stack guard page.
pub fn is_guard_page(fault_addr: u64) -> bool {
    if fault_addr < STACK_VIRT_BASE || fault_addr >= STACK_VIRT_LIMIT {
        return false;
    }
    let slot_bytes = (STACK_SLOT_PAGES as u64) * 4096;
    let offset_in_slot = (fault_addr - STACK_VIRT_BASE) % slot_bytes;
    // The first page in the slot (offset 0..4096) is the unmapped guard page
    offset_in_slot < 4096
}
