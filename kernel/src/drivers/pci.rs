//! PCI configuration space access via I/O ports 0xCF8 (CONFIG_ADDRESS) and 0xCFC (CONFIG_DATA).
//!
//! Enables scanning the PCI bus to discover devices (e.g., AHCI SATA storage controllers,
//! network cards, graphics adapters).

use x86_64::instructions::port::Port;

const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// Represents a PCI device location on the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDeviceLocation {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

/// Generic PCI Device Header information read from configuration space.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PciDeviceInfo {
    pub location: PciDeviceLocation,
    pub vendor_id: u16,
    pub device_id: u16,
    pub command: u16,
    pub status: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub bar5: u32,
}

/// Reads a 32-bit dword from PCI configuration space.
pub fn pci_read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = (1u32 << 31) // Enable bit
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);
        addr_port.write(address);
        data_port.read()
    }
}

/// Reads a 16-bit word from PCI configuration space.
pub fn pci_read_u16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let dword = pci_read_u32(bus, device, function, offset);
    let shift = (offset & 2) * 8;
    ((dword >> shift) & 0xFFFF) as u16
}

/// Writes a 32-bit dword to PCI configuration space.
pub fn pci_write_u32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address = (1u32 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);
        addr_port.write(address);
        data_port.write(value);
    }
}

/// Scans PCI buses 0..8 for attached hardware devices.
pub fn scan_bus() -> alloc::vec::Vec<PciDeviceInfo> {
    let mut devices = alloc::vec::Vec::new();

    for bus in 0..=8 {
        for device in 0..32 {
            // Read vendor ID of function 0
            let vendor_id = pci_read_u16(bus, device, 0, 0x00);
            if vendor_id == 0xFFFF {
                // Device does not exist
                continue;
            }

            let device_id = pci_read_u16(bus, device, 0, 0x02);
            let command = pci_read_u16(bus, device, 0, 0x04);
            let status = pci_read_u16(bus, device, 0, 0x06);
            let class_rev = pci_read_u32(bus, device, 0, 0x08);
            let class_code = ((class_rev >> 24) & 0xFF) as u8;
            let subclass = ((class_rev >> 16) & 0xFF) as u8;
            let prog_if = ((class_rev >> 8) & 0xFF) as u8;
            let bar5 = pci_read_u32(bus, device, 0, 0x24);

            let loc = PciDeviceLocation {
                bus,
                device,
                function: 0,
            };

            let dev = PciDeviceInfo {
                location: loc,
                vendor_id,
                device_id,
                command,
                status,
                class_code,
                subclass,
                prog_if,
                bar5,
            };

            serial_println!(
                "[pci    ] {bus:02x}:{device:02x}.0 vendor={vendor_id:#06x} dev={device_id:#06x} class={class_code:#04x}/{subclass:#04x} (prog_if {prog_if:#04x})",
            );

            devices.push(dev);
        }
    }

    devices
}
