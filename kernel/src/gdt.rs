//! GDT + TSS: segment descriptors and the double-fault IST stack.
//!
//! Long-mode segmentation is mostly vestigial, but three things still matter:
//! a code selector for `CS`, a DPL-0 data selector for `DS`/`ES`/`SS` (all
//! segment registers must reference *our* GDT once it is loaded), and a TSS
//! whose IST1 stack catches double faults even when the normal stack is gone.

use x86_64::{
    VirtAddr,
    instructions::tables::load_tss,
    registers::segmentation::{CS, DS, ES, SS, Segment},
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        tss::TaskStateSegment,
    },
};

/// IST index (0-based; architectural IST1) wired to double faults.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Double-fault stack size: 20 KiB (five 4 KiB pages) — enough for a deep fault
/// log even if the faulting stack was garbage.
pub const DOUBLE_FAULT_STACK_SIZE: usize = 5 * 4096;

/// The double-fault stack itself (accessed exclusively through
/// [`double_fault_stack_top`]).
static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

/// Virtual address one past the top of the double-fault stack
/// (stacks grow down, so the CPU starts pushing at this address).
pub fn double_fault_stack_top() -> u64 {
    // `addr_of!` (never `&DOUBLE_FAULT_STACK`) so no reference to a
    // `static mut` is materialised — edition 2024 denies `static_mut_refs`.
    let base = core::ptr::addr_of!(DOUBLE_FAULT_STACK) as u64;
    base + DOUBLE_FAULT_STACK_SIZE as u64
}

/// Kernel privilege stack size for Ring 3 interrupts/syscalls (20 KiB).
pub const KERNEL_RSP0_STACK_SIZE: usize = 5 * 4096;

/// Dedicated kernel privilege stack (RSP0).
static mut KERNEL_RSP0_STACK: [u8; KERNEL_RSP0_STACK_SIZE] = [0; KERNEL_RSP0_STACK_SIZE];

pub fn kernel_rsp0_stack_top() -> u64 {
    let base = core::ptr::addr_of!(KERNEL_RSP0_STACK) as u64;
    base + KERNEL_RSP0_STACK_SIZE as u64
}

lazy_static::lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            VirtAddr::new(double_fault_stack_top());
        // RSP0: automatically loaded by CPU when an interrupt arrives while in Ring 3
        tss.privilege_stack_table[0] = VirtAddr::new(kernel_rsp0_stack_top());
        tss
    };
}

lazy_static::lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        // For sysret compatibility in x86_64:
        // user_data followed by user_code
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let tss = gdt.append(Descriptor::tss_segment(&TSS));
        (gdt, Selectors { kernel_code, kernel_data, user_code, user_data, tss })
    };
}

/// Selectors produced when the GDT was built.
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    #[allow(dead_code)]
    pub user_code: SegmentSelector,
    #[allow(dead_code)]
    pub user_data: SegmentSelector,
    pub tss: SegmentSelector,
}

/// Returns the active GDT segment selectors.
#[allow(dead_code)]
pub fn selectors() -> &'static Selectors {
    &GDT.1
}

/// Updates the TSS RSP0 privilege stack pointer for Ring 3 -> Ring 0 transitions.
#[allow(dead_code)]
pub fn set_privilege_stack(stack_top: VirtAddr) {
    let tss_ptr = &*TSS as *const TaskStateSegment as *mut TaskStateSegment;
    unsafe {
        (*tss_ptr).privilege_stack_table[0] = stack_top;
    }
}

/// Loads the GDT, refreshes every segment register from it, loads the TSS.
///
/// Must run before interrupts are enabled: the IDT's double-fault entry
/// points at the TSS's IST1 stack.
pub fn init() {
    let (gdt, selectors) = &*GDT;
    gdt.load();
    unsafe {
        // SAFETY: every selector comes from the GDT just loaded — ring-0
        // code (`CS`) and ring-0 DPL-0 data (`DS`/`ES`/`SS`) are legal for a
        // ring-0 task; reloading them discards whatever stale indices the
        // bootloader's GDT left behind.
        CS::set_reg(selectors.kernel_code);
        DS::set_reg(selectors.kernel_data);
        ES::set_reg(selectors.kernel_data);
        SS::set_reg(selectors.kernel_data);
        // SAFETY: the selector addresses the TSS descriptor in the GDT we
        // just loaded; from now on the CPU may consult its IST stacks.
        load_tss(selectors.tss);
    }
}
