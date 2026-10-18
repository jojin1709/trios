//! Advanced Host Controller Interface (AHCI) SATA disk driver.
//!
//! AHCI PCI class: 0x01 (Mass Storage), subclass: 0x06 (SATA), prog_if: 0x01 (AHCI).
//! Uses Base Address Register 5 (BAR5) for AHCI Memory Mapped I/O (Generic Host Control).

use alloc::vec::Vec;
use x86_64::VirtAddr;

use super::pci::{self, PciDeviceInfo};

/// SATA Device Signatures
pub const SATA_SIG_ATA: u32 = 0x00000101;   // SATA drive
pub const SATA_SIG_ATAPI: u32 = 0xEB140101; // SATAPI drive

/// AHCI Controller information.
#[allow(dead_code)]
#[derive(Debug)]
pub struct AhciController {
    pub bar5_phys: u32,
    pub bar5_virt: VirtAddr,
    pub ports_implemented: u32,
    pub active_drives: Vec<u8>,
}

impl AhciController {
    /// Probes and initializes an AHCI SATA controller found on the PCI bus.
    pub fn init(phys_offset: VirtAddr) -> Option<Self> {
        serial_println!("[ahci   ] scanning PCI bus for AHCI SATA storage controller...");
        let devices = pci::scan_bus();

        let mut ahci_dev: Option<PciDeviceInfo> = None;
        for dev in devices {
            // Class 0x01 (Mass Storage), Subclass 0x06 (SATA)
            if dev.class_code == 0x01 && dev.subclass == 0x06 {
                serial_println!(
                    "[ahci   ] found SATA controller at {}:{}.0 (vendor={:#06x}, dev={:#06x}, BAR5={:#x})",
                    dev.location.bus,
                    dev.location.device,
                    dev.vendor_id,
                    dev.device_id,
                    dev.bar5
                );
                ahci_dev = Some(dev);
                break;
            }
        }

        let dev = match ahci_dev {
            Some(d) => d,
            None => {
                serial_println!("[ahci   ] no SATA controller found on PCI bus");
                return None;
            }
        };

        // Enable Bus Mastering (bit 2) and Memory Space (bit 1) in PCI Command Register
        let cmd = pci::pci_read_u16(dev.location.bus, dev.location.device, 0, 0x04);
        pci::pci_write_u32(
            dev.location.bus,
            dev.location.device,
            0,
            0x04,
            (cmd as u32) | 0x06,
        );

        let bar5_phys = dev.bar5 & 0xFFFF_FFF0; // Mask out flags
        let bar5_virt = phys_offset + bar5_phys as u64;

        serial_println!(
            "[ahci   ] mapped AHCI Generic Host Control: phys {bar5_phys:#x} -> virt {:#x}",
            bar5_virt.as_u64()
        );

        // Read AHCI CAP (offset 0x00) and PI (Ports Implemented, offset 0x0C)
        let ghc_ptr = bar5_virt.as_ptr::<u32>();
        let cap = unsafe { core::ptr::read_volatile(ghc_ptr.add(0)) };
        let ports_implemented = unsafe { core::ptr::read_volatile(ghc_ptr.add(3)) }; // offset 0x0C

        let port_count = (cap & 0x1F) + 1;
        serial_println!(
            "[ahci   ] AHCI 1.{}.{} | ports implemented: {ports_implemented:#010b} (max ports: {port_count})",
            (cap >> 20) & 0xF,
            (cap >> 16) & 0xF
        );

        let mut active_drives = Vec::new();

        // Check ports 0..32
        for port in 0..32 {
            if (ports_implemented & (1 << port)) != 0 {
                // Port base = GHC base (0x100) + (port * 0x80)
                let port_offset = 0x100 + (port * 0x80);
                let port_ptr = (bar5_virt.as_u64() + port_offset as u64) as *const u32;

                // Read SSTS (Serial ATA Status, port offset 0x28 -> byte offset 0x28 / 4 = 10)
                let ssts = unsafe { core::ptr::read_volatile(port_ptr.add(10)) };
                let ipm = (ssts >> 8) & 0x0F;
                let det = ssts & 0x0F;

                // DET = 3: device detected and physical communication established
                // IPM = 1: device in active state
                if det == 3 && ipm == 1 {
                    // Read SIG (Signature, port offset 0x24 / 4 = 9)
                    let sig = unsafe { core::ptr::read_volatile(port_ptr.add(9)) };
                    if sig == SATA_SIG_ATA {
                        serial_println!("[ahci   ] port #{port}: active SATA Hard Disk drive detected!");
                        active_drives.push(port as u8);
                    } else if sig == SATA_SIG_ATAPI {
                        serial_println!("[ahci   ] port #{port}: SATAPI CD/DVD drive detected");
                    } else {
                        serial_println!("[ahci   ] port #{port}: connected (sig={sig:#x})");
                        active_drives.push(port as u8);
                    }
                }
            }
        }

        Some(Self {
            bar5_phys,
            bar5_virt,
            ports_implemented,
            active_drives,
        })
    }
}
