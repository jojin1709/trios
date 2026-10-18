//! Hardware device drivers for TriOS.

pub mod ahci;
pub mod pci;

pub use ahci::AhciController;
