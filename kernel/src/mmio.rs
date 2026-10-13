//! Volatile MMIO helpers — the Phase 2 seam for device drivers.
//!
//! Phase 1 devices (PIT, 8259 PIC, UART) are *port-I/O* devices, so nothing
//! here is exercised yet. The framebuffer, APIC, and virtio devices in later
//! phases need typed volatile access to physical memory; writing these tiny
//! helpers now keeps every future MMIO `unsafe` in one reviewed place.

#![allow(dead_code)] // Phase 1: seam only — no in-tree callers yet.

use core::mem::size_of;
use core::ptr::{read_volatile, write_volatile};

/// A device register (or register block) at a fixed virtual address.
///
/// Reads/writes are volatile so the compiler cannot elide, fuse, or reorder
/// them — the defining hazard of memory-mapped I/O.
#[repr(transparent)]
pub struct Mmio<T> {
    ptr: *mut T,
}

impl<T> Mmio<T> {
    /// Wraps the device register at `addr`.
    ///
    /// # Safety
    /// - `addr` must be the base of a mapped MMIO region holding at least
    ///   `size_of::<T>()` bytes, aligned for `T`;
    /// - the caller must keep that region mapped and have exclusive access
    ///   for the lifetime of the value (no aliasing with other accessors);
    /// - `T` must be a plain-old-data type (`Copy`, no padding-sensitive
    ///   invariants) — registers are bit patterns, not Rust values.
    pub const unsafe fn at(addr: usize) -> Self {
        Self {
            // SAFETY: integer-to-pointer cast only forms the address; validity
            // is the caller's contract (documented above). No deref happens
            // until `read`/`write`, which are themselves unsafe-by-contract.
            ptr: addr as *mut T,
        }
    }

    /// Volatile read of the register.
    ///
    /// # Safety
    /// Same contract as [`Mmio::at`]: the mapping must be live, aligned, and
    /// exclusively owned for this access.
    pub unsafe fn read(&self) -> T
    where
        T: Copy,
    {
        // SAFETY: forwarded to the caller's contract; volatile so rustc keeps
        // the access even though the result may look unused.
        unsafe { read_volatile(self.ptr) }
    }

    /// Volatile write of the register.
    ///
    /// # Safety
    /// Same contract as [`Mmio::at`], plus `value` must be a meaningful bit
    /// pattern for the device.
    pub unsafe fn write(&self, value: T)
    where
        T: Copy,
    {
        // SAFETY: forwarded to the caller's contract (see `read`).
        unsafe { write_volatile(self.ptr, value) }
    }

    /// Size of the register this handle covers (alignment/size sanity checks
    /// for driver authors).
    pub const fn size() -> usize {
        size_of::<T>()
    }
}
