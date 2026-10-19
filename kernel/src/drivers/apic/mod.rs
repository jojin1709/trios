//! Advanced Programmable Interrupt Controller (APIC) subsystem:
//! Local APIC (LAPIC) and I/O APIC (IOAPIC) drivers for TriOS.

pub mod lapic;
pub mod ioapic;
pub mod madt;

pub use lapic::LocalApic;
pub use ioapic::IoApic;
pub use madt::{MadtInfo, parse_madt};
