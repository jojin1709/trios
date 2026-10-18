//! Intel 82540EM (e1000) Gigabit Ethernet Network Interface Card Driver.
//!
//! Provides bare-metal Ethernet packet transmission and reception via memory-mapped
//! I/O (MMIO) and DMA descriptor rings.

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use crate::drivers::pci::{self, PciDeviceInfo};

pub const INTEL_VEND: u16 = 0x8086;
pub const E1000_DEV_82540EM: u16 = 0x100E;
pub const E1000_DEV_82545EM: u16 = 0x100F;
pub const E1000_DEV_82543GC: u16 = 0x1004;

const REG_CTRL: u32 = 0x0000;
const REG_STATUS: u32 = 0x0008;
const REG_EERD: u32 = 0x0014;
const REG_IMC: u32 = 0x00D8;
const REG_RCTL: u32 = 0x0100;
const REG_TIPG: u32 = 0x0410;
const REG_RDBAL: u32 = 0x2800;
const REG_RDBAH: u32 = 0x2804;
const REG_RDLEN: u32 = 0x2808;
const REG_RDH: u32 = 0x2810;
const REG_RDT: u32 = 0x2818;
const REG_TCTL: u32 = 0x0400;
const REG_TDBAL: u32 = 0x3800;
const REG_TDBAH: u32 = 0x3804;
const REG_TDLEN: u32 = 0x3808;
const REG_TDH: u32 = 0x3810;
const REG_TDT: u32 = 0x3818;
const REG_MTA: u32 = 0x5200;
const REG_RAL: u32 = 0x5400;
const REG_RAH: u32 = 0x5404;

pub const NUM_RX_DESCS: usize = 32;
pub const NUM_TX_DESCS: usize = 16;
pub const BUFFER_SIZE: usize = 2048;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct RxDesc {
    pub addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct TxDesc {
    pub addr: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}

pub struct E1000Driver {
    pub mmio_base: u64,
    pub mac: [u8; 6],
    rx_descs_ptr: *mut RxDesc,
    tx_descs_ptr: *mut TxDesc,
    _rx_descs_buf: Vec<u8>,
    _tx_descs_buf: Vec<u8>,
    rx_buffers: Vec<*mut u8>,
    tx_buffers: Vec<*mut u8>,
    _rx_backing_pages: Vec<Vec<u8>>,
    _tx_backing_pages: Vec<Vec<u8>>,
    rx_cur: usize,
    tx_cur: usize,
    pub packets_rx: usize,
    pub packets_tx: usize,
    pub bytes_rx: usize,
    pub bytes_tx: usize,
}

unsafe impl Send for E1000Driver {}
unsafe impl Sync for E1000Driver {}

pub static E1000: Mutex<Option<E1000Driver>> = Mutex::new(None);

#[inline]
unsafe fn read_reg(mmio_base: u64, offset: u32) -> u32 {
    let ptr = (mmio_base + offset as u64) as *const u32;
    unsafe { core::ptr::read_volatile(ptr) }
}

#[inline]
unsafe fn write_reg(mmio_base: u64, offset: u32, val: u32) {
    let ptr = (mmio_base + offset as u64) as *mut u32;
    unsafe { core::ptr::write_volatile(ptr, val); }
}

fn read_eeprom(mmio_base: u64, addr: u8) -> u16 {
    unsafe {
        write_reg(mmio_base, REG_EERD, 1 | ((addr as u32) << 8));
        let mut timeout = 10000;
        loop {
            let val = read_reg(mmio_base, REG_EERD);
            if (val & (1 << 4)) != 0 {
                return (val >> 16) as u16;
            }
            timeout -= 1;
            if timeout == 0 {
                return 0;
            }
            core::hint::spin_loop();
        }
    }
}

impl E1000Driver {
    pub fn init(pci_dev: &PciDeviceInfo, phys_offset: u64) -> Result<Self, &'static str> {
        pci::pci_enable_device(
            pci_dev.location.bus,
            pci_dev.location.device,
            pci_dev.location.function,
        );

        let bar0 = pci_dev.bar0;
        if bar0 == 0 || (bar0 & 1) != 0 {
            return Err("E1000 invalid MMIO BAR0");
        }
        let bar0_phys = (bar0 & 0xFFFF_FFF0) as u64;
        let mmio_base = phys_offset + bar0_phys;

        serial_println!(
            "[e1000   ] initializing Intel Gigabit NIC at MMIO phys={:#x} virt={:#x}",
            bar0_phys,
            mmio_base
        );

        unsafe {
            // Reset device
            write_reg(mmio_base, REG_CTRL, read_reg(mmio_base, REG_CTRL) | (1 << 26));
            // Small delay for reset
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }

            // Set Link Up (SLU bit 6) and auto-speed detection (ASDE bit 5)
            let ctrl = read_reg(mmio_base, REG_CTRL);
            write_reg(mmio_base, REG_CTRL, ctrl | (1 << 6) | (1 << 5));

            // Disable interrupts
            write_reg(mmio_base, REG_IMC, 0xFFFF_FFFF);
            let _ = read_reg(mmio_base, 0x00C0); // Clear ICR

            // Clear multicast table array
            for i in 0..128 {
                write_reg(mmio_base, REG_MTA + (i * 4), 0);
            }
        }

        // Determine MAC Address
        let mut mac = [0u8; 6];
        let w0 = read_eeprom(mmio_base, 0);
        let w1 = read_eeprom(mmio_base, 1);
        let w2 = read_eeprom(mmio_base, 2);

        if w0 != 0 || w1 != 0 || w2 != 0 {
            mac[0] = (w0 & 0xFF) as u8;
            mac[1] = (w0 >> 8) as u8;
            mac[2] = (w1 & 0xFF) as u8;
            mac[3] = (w1 >> 8) as u8;
            mac[4] = (w2 & 0xFF) as u8;
            mac[5] = (w2 >> 8) as u8;
        } else {
            // Fallback: read from Receive Address Registers
            unsafe {
                let ral = read_reg(mmio_base, REG_RAL);
                let rah = read_reg(mmio_base, REG_RAH);
                mac[0] = (ral & 0xFF) as u8;
                mac[1] = ((ral >> 8) & 0xFF) as u8;
                mac[2] = ((ral >> 16) & 0xFF) as u8;
                mac[3] = ((ral >> 24) & 0xFF) as u8;
                mac[4] = (rah & 0xFF) as u8;
                mac[5] = ((rah >> 8) & 0xFF) as u8;
            }
        }

        // Program Receive Address Filter 0 to match our MAC
        unsafe {
            let ral = (mac[0] as u32)
                | ((mac[1] as u32) << 8)
                | ((mac[2] as u32) << 16)
                | ((mac[3] as u32) << 24);
            let rah = (mac[4] as u32) | ((mac[5] as u32) << 8) | (1 << 31); // Bit 31: Address Valid
            write_reg(mmio_base, REG_RAL, ral);
            write_reg(mmio_base, REG_RAH, rah);
        }

        serial_println!(
            "[e1000   ] MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );

        let status = unsafe { read_reg(mmio_base, REG_STATUS) };
        let link_up = (status & (1 << 1)) != 0;
        serial_println!(
            "[e1000   ] hardware status: {status:#010x} | Link {}",
            if link_up { "UP" } else { "DOWN" }
        );

        // Allocate RX Descriptors buffer (aligned to 128 bytes)
        let rx_descs_buf = vec![0u8; NUM_RX_DESCS * 16 + 128];
        let rx_raw_virt = rx_descs_buf.as_ptr() as usize;
        let rx_aligned_virt = (rx_raw_virt + 127) & !127;
        let rx_descs_phys = (rx_aligned_virt as u64) - phys_offset;
        let rx_descs_ptr = rx_aligned_virt as *mut RxDesc;

        // Allocate RX Buffers
        let mut rx_backing_pages = Vec::with_capacity(NUM_RX_DESCS);
        let mut rx_buffers = Vec::with_capacity(NUM_RX_DESCS);

        for i in 0..NUM_RX_DESCS {
            let mut page = vec![0u8; BUFFER_SIZE];
            let buf_virt = page.as_mut_ptr();
            let buf_phys = (buf_virt as u64) - phys_offset;
            rx_buffers.push(buf_virt);
            rx_backing_pages.push(page);

            unsafe {
                let desc = &mut *rx_descs_ptr.add(i);
                desc.addr = buf_phys;
                desc.length = 0;
                desc.checksum = 0;
                desc.status = 0;
                desc.errors = 0;
                desc.special = 0;
            }
        }

        // Program RX registers
        unsafe {
            write_reg(mmio_base, REG_RDBAL, rx_descs_phys as u32);
            write_reg(mmio_base, REG_RDBAH, (rx_descs_phys >> 32) as u32);
            write_reg(mmio_base, REG_RDLEN, (NUM_RX_DESCS * 16) as u32);
            write_reg(mmio_base, REG_RDH, 0);
            write_reg(mmio_base, REG_RDT, (NUM_RX_DESCS - 1) as u32);

            // RCTL: EN (1<<1), SBP (1<<2), UPE (1<<3), MPE (1<<4), BAM (1<<15), SECRC (1<<26)
            let rctl = (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 15) | (1 << 26);
            write_reg(mmio_base, REG_RCTL, rctl);
        }

        // Allocate TX Descriptors buffer (aligned to 128 bytes)
        let tx_descs_buf = vec![0u8; NUM_TX_DESCS * 16 + 128];
        let tx_raw_virt = tx_descs_buf.as_ptr() as usize;
        let tx_aligned_virt = (tx_raw_virt + 127) & !127;
        let tx_descs_phys = (tx_aligned_virt as u64) - phys_offset;
        let tx_descs_ptr = tx_aligned_virt as *mut TxDesc;

        // Allocate TX Buffers
        let mut tx_backing_pages = Vec::with_capacity(NUM_TX_DESCS);
        let mut tx_buffers = Vec::with_capacity(NUM_TX_DESCS);

        for i in 0..NUM_TX_DESCS {
            let mut page = vec![0u8; BUFFER_SIZE];
            let buf_virt = page.as_mut_ptr();
            let buf_phys = (buf_virt as u64) - phys_offset;
            tx_buffers.push(buf_virt);
            tx_backing_pages.push(page);

            unsafe {
                let desc = &mut *tx_descs_ptr.add(i);
                desc.addr = buf_phys;
                desc.length = 0;
                desc.cso = 0;
                desc.cmd = 0;
                desc.status = 1; // Mark initially done
                desc.css = 0;
                desc.special = 0;
            }
        }

        // Program TX registers
        unsafe {
            write_reg(mmio_base, REG_TDBAL, tx_descs_phys as u32);
            write_reg(mmio_base, REG_TDBAH, (tx_descs_phys >> 32) as u32);
            write_reg(mmio_base, REG_TDLEN, (NUM_TX_DESCS * 16) as u32);
            write_reg(mmio_base, REG_TDH, 0);
            write_reg(mmio_base, REG_TDT, 0);

            // Standard IPG values: IPGT=10, IPGR1=8, IPGR2=6
            write_reg(mmio_base, REG_TIPG, 10 | (8 << 10) | (6 << 20));

            // TCTL: EN (1<<1), PSP (1<<3), CT=0x0F (4..11), COLD=0x040 (12..21)
            let tctl = (1 << 1) | (1 << 3) | (0x0F << 4) | (0x040 << 12);
            write_reg(mmio_base, REG_TCTL, tctl);
        }

        serial_println!("[e1000   ] RX/TX DMA rings active and configured");

        Ok(Self {
            mmio_base,
            mac,
            rx_descs_ptr,
            tx_descs_ptr,
            _rx_descs_buf: rx_descs_buf,
            _tx_descs_buf: tx_descs_buf,
            rx_buffers,
            tx_buffers,
            _rx_backing_pages: rx_backing_pages,
            _tx_backing_pages: tx_backing_pages,
            rx_cur: NUM_RX_DESCS - 1,
            tx_cur: 0,
            packets_rx: 0,
            packets_tx: 0,
            bytes_rx: 0,
            bytes_tx: 0,
        })
    }

    /// Transmits an Ethernet packet over the wire.
    pub fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() > BUFFER_SIZE {
            return Err("Packet exceeds max frame size");
        }

        let cur = self.tx_cur;
        let buf = self.tx_buffers[cur];

        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
            let desc = &mut *self.tx_descs_ptr.add(cur);
            desc.length = data.len() as u16;
            desc.cso = 0;
            // CMD: EOP (1<<0) | IFCS (1<<1) | RS (1<<3)
            desc.cmd = (1 << 0) | (1 << 1) | (1 << 3);
            desc.status = 0;
            desc.css = 0;
            desc.special = 0;

            self.tx_cur = (cur + 1) % NUM_TX_DESCS;
            write_reg(self.mmio_base, REG_TDT, self.tx_cur as u32);

            // Wait for completion (Descriptor Done bit 0)
            let mut spin = 50_000;
            while (desc.status & 1) == 0 && spin > 0 {
                core::hint::spin_loop();
                spin -= 1;
            }

            if spin == 0 {
                return Err("TX timed out");
            }
        }

        self.packets_tx += 1;
        self.bytes_tx += data.len();
        Ok(())
    }

    /// Checks the receive ring for incoming packets.
    pub fn receive(&mut self) -> Option<Vec<u8>> {
        let next = (self.rx_cur + 1) % NUM_RX_DESCS;
        let desc = unsafe { &mut *self.rx_descs_ptr.add(next) };

        if (desc.status & 1) != 0 {
            let len = desc.length as usize;
            let mut packet = vec![0u8; len];
            unsafe {
                core::ptr::copy_nonoverlapping(self.rx_buffers[next], packet.as_mut_ptr(), len);
                desc.status = 0;
                write_reg(self.mmio_base, REG_RDT, next as u32);
            }

            self.rx_cur = next;
            self.packets_rx += 1;
            self.bytes_rx += len;
            Some(packet)
        } else {
            None
        }
    }

    /// Returns the link state: true if UP, false if DOWN.
    pub fn is_link_up(&self) -> bool {
        let status = unsafe { read_reg(self.mmio_base, REG_STATUS) };
        (status & (1 << 1)) != 0 || self.packets_rx > 0 || self.packets_tx > 0
    }
}
