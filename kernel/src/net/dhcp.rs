//! Dynamic Host Configuration Protocol (DHCP) Client (RFC 2131).
//!
//! Provides bare-metal implementation of:
//! - DHCPDISCOVER broadcast to locate available DHCP servers
//! - DHCPOFFER handling and option parsing (IP, subnet mask, router/gateway, DNS server)
//! - DHCPREQUEST transmission requesting offered lease
//! - DHCPACK processing and automatic network stack configuration
//!
//! Developed by JOJIN JOHN (@jojin1709) for TriOS.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use super::{poll_network, BROADCAST_MAC, ETHERTYPE_IPV4, NET};
use crate::drivers::e1000::E1000;

pub const DHCP_CLIENT_PORT: u16 = 68;
pub const DHCP_SERVER_PORT: u16 = 67;

pub const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99]; // 0x63825363

// DHCP Message Types (Option 53)
pub const DHCPDISCOVER: u8 = 1;
pub const DHCPOFFER: u8 = 2;
pub const DHCPREQUEST: u8 = 3;
pub const DHCPACK: u8 = 5;

// DHCP Option Codes
pub const OPT_PAD: u8 = 0;
pub const OPT_SUBNET_MASK: u8 = 1;
pub const OPT_ROUTER: u8 = 3;
pub const OPT_DNS_SERVER: u8 = 6;
pub const OPT_REQUESTED_IP: u8 = 50;
pub const OPT_LEASE_TIME: u8 = 51;
pub const OPT_DHCP_MSG_TYPE: u8 = 53;
pub const OPT_SERVER_ID: u8 = 54;
pub const OPT_PARAM_REQUEST_LIST: u8 = 55;
pub const OPT_END: u8 = 255;

static TRANSACTION_ID: AtomicU32 = AtomicU32::new(0x3903F326);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpState {
    Idle,
    DiscoverSent,
    OfferReceived,
    RequestSent,
    Bound,
}

#[derive(Debug, Clone)]
pub struct DhcpLease {
    pub state: DhcpState,
    pub xid: u32,
    pub offered_ip: [u8; 4],
    pub server_ip: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns_server: [u8; 4],
    pub lease_time_secs: u32,
}

pub static DHCP_CLIENT: spin::Mutex<DhcpLease> = spin::Mutex::new(DhcpLease {
    state: DhcpState::Idle,
    xid: 0,
    offered_ip: [0; 4],
    server_ip: [0; 4],
    subnet_mask: [255, 255, 255, 0],
    gateway: [0; 4],
    dns_server: [0; 4],
    lease_time_secs: 0,
});

/// Constructs a raw DHCP packet with the provided options.
fn build_dhcp_packet(op: u8, xid: u32, ciaddr: [u8; 4], client_mac: [u8; 6], options: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(240 + options.len());
    packet.push(op); // 0: op (1 = BOOTREQUEST)
    packet.push(1);  // 1: htype (1 = Ethernet 10Mb)
    packet.push(6);  // 2: hlen (Ethernet address length = 6)
    packet.push(0);  // 3: hops (0)

    packet.extend_from_slice(&xid.to_be_bytes()); // 4..8: xid
    packet.extend_from_slice(&0u16.to_be_bytes()); // 8..10: secs (0)
    packet.extend_from_slice(&0x8000u16.to_be_bytes()); // 10..12: flags (Broadcast flag set)

    packet.extend_from_slice(&ciaddr); // 12..16: ciaddr (Client IP)
    packet.extend_from_slice(&[0u8; 4]); // 16..20: yiaddr ('your' IP)
    packet.extend_from_slice(&[0u8; 4]); // 20..24: siaddr (Next server IP)
    packet.extend_from_slice(&[0u8; 4]); // 24..28: giaddr (Relay agent IP)

    // 28..44: chaddr (Client hardware address, 16 bytes)
    packet.extend_from_slice(&client_mac);
    packet.extend_from_slice(&[0u8; 10]);

    // 44..108: sname (Server host name, 64 bytes)
    packet.extend_from_slice(&[0u8; 64]);

    // 108..236: file (Boot file name, 128 bytes)
    packet.extend_from_slice(&[0u8; 128]);

    // 236..240: Magic Cookie
    packet.extend_from_slice(&DHCP_MAGIC_COOKIE);

    // Options
    packet.extend_from_slice(options);

    // End option if not present
    if !options.ends_with(&[OPT_END]) {
        packet.push(OPT_END);
    }

    // BOOTP / DHCP minimum payload is 300 bytes (pad with 0)
    while packet.len() < 300 {
        packet.push(0);
    }

    packet
}

/// Transmits a UDP broadcast packet from 0.0.0.0:68 to 255.255.255.255:67.
fn send_dhcp_udp_broadcast(payload: &[u8]) -> Result<(), &'static str> {
    let mut guard = E1000.lock();
    let nic = guard.as_mut().ok_or("No NIC present")?;
    let src_mac = nic.mac;
    drop(guard);

    let udp_len = (8 + payload.len()) as u16;
    let mut udp = Vec::with_capacity(8 + payload.len());
    udp.extend_from_slice(&DHCP_CLIENT_PORT.to_be_bytes()); // 68
    udp.extend_from_slice(&DHCP_SERVER_PORT.to_be_bytes()); // 67
    udp.extend_from_slice(&udp_len.to_be_bytes());
    udp.extend_from_slice(&0u16.to_be_bytes()); // UDP Checksum optional in IPv4
    udp.extend_from_slice(payload);

    // Build IPv4 broadcast packet: src=0.0.0.0, dest=255.255.255.255
    let total_len = (20 + udp.len()) as u16;
    let mut ip_hdr = Vec::with_capacity(20);
    ip_hdr.push(0x45); // IPv4, IHL=5
    ip_hdr.push(0x00);
    ip_hdr.extend_from_slice(&total_len.to_be_bytes());
    ip_hdr.extend_from_slice(&1u16.to_be_bytes()); // ID
    ip_hdr.extend_from_slice(&0x0000u16.to_be_bytes()); // Flags/Fragment
    ip_hdr.push(64); // TTL
    ip_hdr.push(super::IP_PROTO_UDP);
    ip_hdr.extend_from_slice(&0u16.to_be_bytes()); // Checksum placeholder
    ip_hdr.extend_from_slice(&[0, 0, 0, 0]); // Source IP: 0.0.0.0
    ip_hdr.extend_from_slice(&[255, 255, 255, 255]); // Dest IP: 255.255.255.255

    let csum = super::calculate_checksum(&ip_hdr);
    ip_hdr[10] = (csum >> 8) as u8;
    ip_hdr[11] = (csum & 0xFF) as u8;

    let mut full_packet = Vec::with_capacity(20 + udp.len());
    full_packet.extend_from_slice(&ip_hdr);
    full_packet.extend_from_slice(&udp);

    // Send on raw Ethernet II to broadcast MAC
    let mut frame = Vec::with_capacity(14 + full_packet.len());
    frame.extend_from_slice(&BROADCAST_MAC);
    frame.extend_from_slice(&src_mac);
    frame.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
    frame.extend_from_slice(&full_packet);

    let mut guard = E1000.lock();
    let nic = guard.as_mut().ok_or("No NIC present")?;
    nic.send(&frame)
}

/// Broadcasts a DHCPDISCOVER message across the local network.
pub fn send_dhcp_discover() -> Result<u32, &'static str> {
    let mac = {
        let guard = E1000.lock();
        let nic = guard.as_ref().ok_or("No NIC present")?;
        nic.mac
    };

    let xid = TRANSACTION_ID.fetch_add(1, Ordering::SeqCst);

    let mut options = Vec::with_capacity(32);
    // Option 53: DHCP Message Type = DHCPDISCOVER (1)
    options.push(OPT_DHCP_MSG_TYPE);
    options.push(1);
    options.push(DHCPDISCOVER);

    // Option 55: Parameter Request List (Subnet Mask, Router, DNS Server)
    options.push(OPT_PARAM_REQUEST_LIST);
    options.push(3);
    options.push(OPT_SUBNET_MASK);
    options.push(OPT_ROUTER);
    options.push(OPT_DNS_SERVER);

    // Option 255: End
    options.push(OPT_END);

    let packet = build_dhcp_packet(1, xid, [0, 0, 0, 0], mac, &options);

    {
        let mut client = DHCP_CLIENT.lock();
        client.state = DhcpState::DiscoverSent;
        client.xid = xid;
    }

    serial_println!("[dhcp   ] broadcasting DHCPDISCOVER (xid={:#010x})...", xid);
    send_dhcp_udp_broadcast(&packet)?;

    Ok(xid)
}

/// Broadcasts a DHCPREQUEST message for the offered IP address.
pub fn send_dhcp_request(xid: u32, requested_ip: [u8; 4], server_ip: [u8; 4]) -> Result<(), &'static str> {
    let mac = {
        let guard = E1000.lock();
        let nic = guard.as_ref().ok_or("No NIC present")?;
        nic.mac
    };

    let mut options = Vec::with_capacity(32);
    // Option 53: DHCP Message Type = DHCPREQUEST (3)
    options.push(OPT_DHCP_MSG_TYPE);
    options.push(1);
    options.push(DHCPREQUEST);

    // Option 50: Requested IP Address
    options.push(OPT_REQUESTED_IP);
    options.push(4);
    options.extend_from_slice(&requested_ip);

    // Option 54: Server Identifier
    options.push(OPT_SERVER_ID);
    options.push(4);
    options.extend_from_slice(&server_ip);

    // Option 55: Parameter Request List
    options.push(OPT_PARAM_REQUEST_LIST);
    options.push(3);
    options.push(OPT_SUBNET_MASK);
    options.push(OPT_ROUTER);
    options.push(OPT_DNS_SERVER);

    options.push(OPT_END);

    let packet = build_dhcp_packet(1, xid, [0, 0, 0, 0], mac, &options);

    {
        let mut client = DHCP_CLIENT.lock();
        client.state = DhcpState::RequestSent;
    }

    serial_println!(
        "[dhcp   ] sending DHCPREQUEST for {}.{}.{}.{} to server {}.{}.{}.{}...",
        requested_ip[0], requested_ip[1], requested_ip[2], requested_ip[3],
        server_ip[0], server_ip[1], server_ip[2], server_ip[3]
    );

    send_dhcp_udp_broadcast(&packet)
}

/// Dispatches an incoming DHCP packet received on UDP port 68.
pub fn handle_dhcp_packet(payload: &[u8]) {
    if payload.len() < 240 {
        return;
    }

    let op = payload[0];
    if op != 2 {
        // Only accept BOOTREPLY (2)
        return;
    }

    let xid = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let yiaddr = [payload[16], payload[17], payload[18], payload[19]];

    // Verify magic cookie
    if &payload[236..240] != DHCP_MAGIC_COOKIE {
        return;
    }

    // Parse options
    let mut msg_type = None;
    let mut server_id = None;
    let mut subnet_mask = None;
    let mut router = None;
    let mut dns = None;
    let mut lease_time = 0u32;

    let mut i = 240;
    while i < payload.len() {
        let opt = payload[i];
        if opt == OPT_PAD {
            i += 1;
            continue;
        }
        if opt == OPT_END {
            break;
        }
        if i + 1 >= payload.len() {
            break;
        }
        let len = payload[i + 1] as usize;
        let data_start = i + 2;
        let data_end = data_start + len;
        if data_end > payload.len() {
            break;
        }
        let opt_data = &payload[data_start..data_end];

        match opt {
            OPT_DHCP_MSG_TYPE => {
                if !opt_data.is_empty() {
                    msg_type = Some(opt_data[0]);
                }
            }
            OPT_SERVER_ID => {
                if opt_data.len() == 4 {
                    server_id = Some([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]);
                }
            }
            OPT_SUBNET_MASK => {
                if opt_data.len() == 4 {
                    subnet_mask = Some([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]);
                }
            }
            OPT_ROUTER => {
                if opt_data.len() >= 4 {
                    router = Some([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]);
                }
            }
            OPT_DNS_SERVER => {
                if opt_data.len() >= 4 {
                    dns = Some([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]);
                }
            }
            OPT_LEASE_TIME => {
                if opt_data.len() == 4 {
                    lease_time = u32::from_be_bytes([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]);
                }
            }
            _ => {}
        }

        i = data_end;
    }

    let mut client = DHCP_CLIENT.lock();
    if client.xid != xid && client.xid != 0 {
        return;
    }

    match msg_type {
        Some(DHCPOFFER) => {
            serial_println!(
                "[dhcp   ] DHCPOFFER received: offered IP={}.{}.{}.{}, server={:?}",
                yiaddr[0], yiaddr[1], yiaddr[2], yiaddr[3], server_id
            );
            client.state = DhcpState::OfferReceived;
            client.offered_ip = yiaddr;
            if let Some(s) = server_id {
                client.server_ip = s;
            }
            if let Some(mask) = subnet_mask {
                client.subnet_mask = mask;
            }
            if let Some(gw) = router {
                client.gateway = gw;
            }
            if let Some(d) = dns {
                client.dns_server = d;
            }
            client.lease_time_secs = lease_time;

            // Automatically proceed to DHCPREQUEST
            let s_ip = client.server_ip;
            drop(client);
            let _ = send_dhcp_request(xid, yiaddr, s_ip);
        }
        Some(DHCPACK) => {
            serial_println!(
                "[dhcp   ] DHCPACK received! Bound IP={}.{}.{}.{} Subnet={:?} Gateway={:?} DNS={:?}",
                yiaddr[0], yiaddr[1], yiaddr[2], yiaddr[3],
                client.subnet_mask, client.gateway, client.dns_server
            );
            client.state = DhcpState::Bound;
            client.offered_ip = yiaddr;
            if let Some(mask) = subnet_mask {
                client.subnet_mask = mask;
            }
            if let Some(gw) = router {
                client.gateway = gw;
            }
            if let Some(d) = dns {
                client.dns_server = d;
            }
            if lease_time > 0 {
                client.lease_time_secs = lease_time;
            }

            // Update main network stack configuration dynamically!
            let (gw, s_ip) = {
                let mut net = NET.lock();
                net.ip = yiaddr;
                net.subnet = client.subnet_mask;
                if client.gateway != [0, 0, 0, 0] {
                    net.gateway = client.gateway;
                }
                if client.dns_server != [0, 0, 0, 0] {
                    net.dns = client.dns_server;
                }
                (client.gateway, client.server_ip)
            };

            // Request ARP resolution for gateway and DHCP server to prime ARP cache
            if gw != [0, 0, 0, 0] {
                let _ = super::send_arp_request(gw);
            }
            if s_ip != [0, 0, 0, 0] && s_ip != gw {
                let _ = super::send_arp_request(s_ip);
            }
        }
        _ => {}
    }
}

/// Runs full DHCP acquisition loop with bounded timeout and retries.
/// Returns true if DHCP configuration was successfully acquired.
pub fn acquire_lease() -> bool {
    for attempt in 1..=3 {
        let _ = send_dhcp_discover();

        for _ in 0..1_500_000 {
            poll_network();
            let client = DHCP_CLIENT.lock();
            if client.state == DhcpState::Bound {
                return true;
            }
            core::hint::spin_loop();
        }
        serial_println!("[dhcp   ] discover attempt #{attempt} timed out, retrying...");
    }

    let client = DHCP_CLIENT.lock();
    client.state == DhcpState::Bound
}
