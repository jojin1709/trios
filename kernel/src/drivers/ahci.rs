//! Advanced Host Controller Interface (AHCI) SATA disk driver.
//!
//! AHCI PCI class: 0x01 (Mass Storage), subclass: 0x06 (SATA), prog_if: 0x01 (AHCI).
//! Uses Base Address Register 5 (BAR5) for AHCI Memory Mapped I/O (Generic Host Control).
//! Provides DMA sector reading from active SATA hard disks using dedicated heap DMA buffers.

use alloc::{vec, vec::Vec};
use spin::Mutex;
use x86_64::VirtAddr;

use super::pci::{self, PciDeviceInfo};

/// SATA Device Signatures
pub const SATA_SIG_ATA: u32 = 0x00000101;   // SATA drive
pub const SATA_SIG_ATAPI: u32 = 0xEB140101; // SATAPI drive

/// ATA Commands
pub const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
pub const FIS_TYPE_REG_H2D: u8 = 0x27;

/// Global AHCI controller instance.
pub static AHCI_CONTROLLER: Mutex<Option<AhciController>> = Mutex::new(None);

/// Memory-allocated DMA buffers for an active AHCI port.
#[allow(dead_code)]
pub struct PortDmaMemory {
    pub clb_phys: u32,
    pub fb_phys: u32,
    pub ct_phys: u32,
    pub dma_buf_phys: u32,
    pub clb_virt: *mut u8,
    pub ct_virt: *mut u8,
    pub dma_buf_virt: *mut u8,
    page_buf: Vec<u8>,
}

// Safety: PortDmaMemory manages raw pointers to dedicated heap memory
unsafe impl Send for PortDmaMemory {}
unsafe impl Sync for PortDmaMemory {}

/// AHCI Controller information and active port DMA handles.
#[allow(dead_code)]
pub struct AhciController {
    pub bar5_phys: u32,
    pub bar5_virt: VirtAddr,
    pub phys_offset: VirtAddr,
    pub ports_implemented: u32,
    pub active_drives: Vec<u8>,
    pub port_mem: Vec<Option<PortDmaMemory>>,
}

impl AhciController {
    /// Probes, initializes, and arms an AHCI SATA controller found on the PCI bus.
    pub fn init(phys_offset: VirtAddr) -> bool {
        serial_println!("[ahci   ] scanning PCI bus for AHCI SATA storage controller...");
        let devices = pci::scan_bus();

        let mut ahci_dev: Option<PciDeviceInfo> = None;
        for dev in devices {
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
                return false;
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

        let bar5_phys = dev.bar5 & 0xFFFF_FFF0;
        let bar5_virt = phys_offset + bar5_phys as u64;

        serial_println!(
            "[ahci   ] mapped AHCI Generic Host Control: phys {bar5_phys:#x} -> virt {:#x}",
            bar5_virt.as_u64()
        );

        // Enable AHCI Mode in Global Host Control (GHC, offset 0x04, bit 31 = AE)
        let ghc_reg = (bar5_virt.as_u64() + 0x04) as *mut u32;
        let ghc_val = unsafe { core::ptr::read_volatile(ghc_reg) };
        unsafe { core::ptr::write_volatile(ghc_reg, ghc_val | (1 << 31)); }

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
        let mut port_mem = Vec::new();
        for _ in 0..32 {
            port_mem.push(None);
        }

        for port in 0..32 {
            if (ports_implemented & (1 << port)) != 0 {
                let port_offset = 0x100 + (port * 0x80);
                let port_ptr = (bar5_virt.as_u64() + port_offset as u64) as *mut u32;

                let ssts = unsafe { core::ptr::read_volatile(port_ptr.add(10)) }; // offset 0x28 / 4
                let ipm = (ssts >> 8) & 0x0F;
                let det = ssts & 0x0F;

                if det == 3 && ipm == 1 {
                    let sig = unsafe { core::ptr::read_volatile(port_ptr.add(9)) }; // offset 0x24 / 4
                    if sig == SATA_SIG_ATA {
                        serial_println!("[ahci   ] port #{port}: active SATA Hard Disk drive detected!");
                        active_drives.push(port as u8);

                        // Allocate 8 KiB buffer on kernel heap to guarantee 1 KiB alignment
                        let mut page_buf = vec![0u8; 8192];
                        let raw_virt = page_buf.as_mut_ptr() as usize;
                        let aligned_virt = (raw_virt + 1023) & !1023;
                        let aligned_phys = (aligned_virt - phys_offset.as_u64() as usize) as u32;

                        // Layout inside aligned buffer:
                        // 0..1024:    Command List (1 KiB aligned)
                        // 1024..1280: Received FIS (256 B aligned)
                        // 1280..1536: Command Table (128 B aligned)
                        // 2048..4096: DMA Sector Buffer (2048 B aligned)
                        let clb_phys = aligned_phys;
                        let fb_phys = aligned_phys + 1024;
                        let ct_phys = aligned_phys + 1280;
                        let dma_buf_phys = aligned_phys + 2048;

                        let clb_virt = aligned_virt as *mut u8;
                        let ct_virt = (aligned_virt + 1280) as *mut u8;
                        let dma_buf_virt = (aligned_virt + 2048) as *mut u8;

                        // Stop port command engine
                        unsafe {
                            let cmd_reg = port_ptr.add(6); // PxCMD offset 0x18 / 4
                            let val = core::ptr::read_volatile(cmd_reg);
                            core::ptr::write_volatile(cmd_reg, val & !0x11); // Clear ST (bit 0) and FRE (bit 4)

                            // Wait until CR and FR are 0
                            let mut spins = 0;
                            while (core::ptr::read_volatile(cmd_reg) & 0xC000) != 0 && spins < 100_000 {
                                spins += 1;
                            }

                            // Program Base Addresses
                            core::ptr::write_volatile(port_ptr.add(0), clb_phys); // PxCLB
                            core::ptr::write_volatile(port_ptr.add(1), 0);        // PxCLBU
                            core::ptr::write_volatile(port_ptr.add(2), fb_phys);  // PxFB
                            core::ptr::write_volatile(port_ptr.add(3), 0);        // PxFBU

                            // Clear Interrupt Status & Errors
                            core::ptr::write_volatile(port_ptr.add(4), 0xFFFF_FFFF); // PxIS
                            core::ptr::write_volatile(port_ptr.add(12), 0xFFFF_FFFF); // PxSERR

                            // Start port command engine (set FRE then ST)
                            core::ptr::write_volatile(cmd_reg, val | 0x10); // Set FRE
                            core::ptr::write_volatile(cmd_reg, val | 0x11); // Set ST and FRE
                        }

                        port_mem[port] = Some(PortDmaMemory {
                            clb_phys,
                            fb_phys,
                            ct_phys,
                            dma_buf_phys,
                            clb_virt,
                            ct_virt,
                            dma_buf_virt,
                            page_buf,
                        });
                    } else if sig == SATA_SIG_ATAPI {
                        serial_println!("[ahci   ] port #{port}: SATAPI CD/DVD drive detected");
                    } else {
                        serial_println!("[ahci   ] port #{port}: connected (sig={sig:#x})");
                    }
                }
            }
        }

        let ctrl = Self {
            bar5_phys,
            bar5_virt,
            phys_offset,
            ports_implemented,
            active_drives,
            port_mem,
        };

        let mut lock = AHCI_CONTROLLER.lock();
        *lock = Some(ctrl);
        drop(lock);

        true
    }

    /// Reads `count` 512-byte sectors starting at `lba` from the specified SATA drive port.
    pub fn read_sectors(
        &self,
        port: u8,
        lba: u64,
        count: u16,
        dest_buf: &mut [u8],
    ) -> Result<(), &'static str> {
        let expected_bytes = count as usize * 512;
        if dest_buf.len() < expected_bytes {
            return Err("destination buffer too small");
        }

        let mem = match self.port_mem.get(port as usize).and_then(|m| m.as_ref()) {
            Some(m) => m,
            None => return Err("port memory not initialized"),
        };

        let port_offset = 0x100 + (port as usize * 0x80);
        let port_ptr = (self.bar5_virt.as_u64() + port_offset as u64) as *mut u32;

        unsafe {
            // Clear Interrupt Status and SERR
            core::ptr::write_volatile(port_ptr.add(4), 0xFFFF_FFFF);
            core::ptr::write_volatile(port_ptr.add(12), 0xFFFF_FFFF);

            // Wait until port is not busy
            let tfd_reg = port_ptr.add(8); // PxTFD offset 0x20 / 4
            let mut spins = 0;
            while (core::ptr::read_volatile(tfd_reg) & 0x88) != 0 {
                spins += 1;
                if spins > 100_000 {
                    return Err("port busy timeout");
                }
            }

            // Command Header 0 (32 bytes at clb_virt)
            core::ptr::write_bytes(mem.clb_virt, 0, 32);
            let clb_u32 = mem.clb_virt as *mut u32;
            core::ptr::write_volatile(clb_u32.add(0), 5 | (1 << 16)); // CFL=5, PRDTL=1
            core::ptr::write_volatile(clb_u32.add(1), 0);             // PRDBC=0
            core::ptr::write_volatile(clb_u32.add(2), mem.ct_phys);  // CTBA
            core::ptr::write_volatile(clb_u32.add(3), 0);             // CTBAU

            // Command Table: Command FIS (64 bytes) + PRDT (at offset 128)
            core::ptr::write_bytes(mem.ct_virt, 0, 144);
            let cfis = mem.ct_virt;
            *cfis.add(0) = FIS_TYPE_REG_H2D;
            *cfis.add(1) = 0x80; // Command bit
            *cfis.add(2) = ATA_CMD_READ_DMA_EXT;
            *cfis.add(4) = (lba & 0xFF) as u8;
            *cfis.add(5) = ((lba >> 8) & 0xFF) as u8;
            *cfis.add(6) = ((lba >> 16) & 0xFF) as u8;
            *cfis.add(7) = 1 << 6; // LBA mode
            *cfis.add(8) = ((lba >> 24) & 0xFF) as u8;
            *cfis.add(9) = ((lba >> 32) & 0xFF) as u8;
            *cfis.add(10) = ((lba >> 40) & 0xFF) as u8;
            *cfis.add(12) = (count & 0xFF) as u8;
            *cfis.add(13) = ((count >> 8) & 0xFF) as u8;

            // PRDT Entry 0 (16 bytes at offset 128) pointing to our guaranteed heap DMA buffer
            let prdt_u32 = mem.ct_virt.add(128) as *mut u32;
            core::ptr::write_volatile(prdt_u32.add(0), mem.dma_buf_phys);
            core::ptr::write_volatile(prdt_u32.add(1), 0);
            core::ptr::write_volatile(prdt_u32.add(2), 0);
            core::ptr::write_volatile(prdt_u32.add(3), ((expected_bytes as u32 - 1) & 0x003F_FFFF) | (1 << 31));

            // Issue command: slot 0 (bit 0 of PxCI, offset 0x38 / 4 = 14)
            let ci_reg = port_ptr.add(14);
            core::ptr::write_volatile(ci_reg, 1);

            // Wait for completion (PxCI bit 0 clears)
            let mut timeout = 0;
            while (core::ptr::read_volatile(ci_reg) & 1) != 0 {
                timeout += 1;
                if timeout > 2_000_000 {
                    let is = core::ptr::read_volatile(port_ptr.add(4));
                    let tfd = core::ptr::read_volatile(tfd_reg);
                    let serr = core::ptr::read_volatile(port_ptr.add(12));
                    serial_println!("[ahci   ] timeout debug: IS={is:#x} TFD={tfd:#x} SERR={serr:#x}");
                    return Err("AHCI command timeout");
                }
            }

            // Copy from heap DMA buffer to caller's destination buffer
            core::ptr::copy_nonoverlapping(mem.dma_buf_virt, dest_buf.as_mut_ptr(), expected_bytes);
        }

        Ok(())
    }
}

/// Helper function to read sectors from disk port 0.
pub fn read_sector(lba: u64, buf: &mut [u8; 512]) -> Result<(), &'static str> {
    let lock = AHCI_CONTROLLER.lock();
    let ctrl = match lock.as_ref() {
        Some(c) => c,
        None => return Err("AHCI controller not initialized"),
    };

    if ctrl.active_drives.is_empty() {
        return Err("no active SATA drives found");
    }

    let port = ctrl.active_drives[0];
    ctrl.read_sectors(port, lba, 1, buf)
}
