//! TriOS Network Protocol Stack.
//!
//! Provides bare-metal implementations of:
//! - Ethernet II frame parsing and transmission
//! - ARP (Address Resolution Protocol) cache and request/reply engine
//! - IPv4 packet framing, header validation, and checksum verification
//! - ICMP Echo Request (ping) and Echo Reply processing

use alloc::vec::Vec;
pub mod tcp;
pub mod http;
pub mod dhcp;
pub mod socket;

use spin::Mutex;
use crate::drivers::e1000::E1000;

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

pub const ARP_OP_REQUEST: u16 = 1;
pub const ARP_OP_REPLY: u16 = 2;

pub const IP_PROTO_ICMP: u8 = 1;
pub const IP_PROTO_TCP: u8 = 6;
#[allow(dead_code)]
pub const IP_PROTO_UDP: u8 = 17;

pub const ICMP_TYPE_ECHO_REPLY: u8 = 0;
pub const ICMP_TYPE_ECHO_REQUEST: u8 = 8;

pub const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArpEntry {
    pub ip: [u8; 4],
    pub mac: [u8; 6],
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PingReplyInfo {
    pub from_ip: [u8; 4],
    pub seq: u16,
    pub ttl: u8,
}

#[allow(dead_code)]
pub struct NetworkStack {
    pub ip: [u8; 4],
    pub subnet: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub arp_cache: Vec<ArpEntry>,
    pub ping_replies: Vec<PingReplyInfo>,
    pub dns_replies: Vec<(u16, [u8; 4])>,
}

pub static NET: Mutex<NetworkStack> = Mutex::new(NetworkStack {
    ip: [10, 0, 2, 15],
    subnet: [255, 255, 255, 0],
    gateway: [10, 0, 2, 2],
    dns: [10, 0, 2, 3],
    arp_cache: Vec::new(),
    ping_replies: Vec::new(),
    dns_replies: Vec::new(),
});

/// Calculates the standard RFC 1071 Internet Checksum.
pub fn calculate_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        sum = sum.wrapping_add(word);
        i += 2;
    }
    if i < data.len() {
        let word = (data[i] as u32) << 8;
        sum = sum.wrapping_add(word);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

impl NetworkStack {
    /// Inserts or updates an entry in the ARP cache.
    pub fn update_arp(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        for entry in self.arp_cache.iter_mut() {
            if entry.ip == ip {
                entry.mac = mac;
                return;
            }
        }
        self.arp_cache.push(ArpEntry { ip, mac });
        serial_println!(
            "[net    ] ARP cache learned: {}.{}.{}.{} -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            ip[0], ip[1], ip[2], ip[3],
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    }

    /// Looks up a MAC address in the ARP cache.
    pub fn lookup_arp(&self, ip: [u8; 4]) -> Option<[u8; 6]> {
        for entry in &self.arp_cache {
            if entry.ip == ip {
                return Some(entry.mac);
            }
        }
        None
    }
}

/// Sends a raw Ethernet frame via the E1000 driver.
pub fn send_ethernet_frame(dest_mac: [u8; 6], ethertype: u16, payload: &[u8]) -> Result<(), &'static str> {
    let mut guard = E1000.lock();
    let nic = guard.as_mut().ok_or("No NIC present")?;
    let src_mac = nic.mac;

    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.extend_from_slice(&dest_mac);
    frame.extend_from_slice(&src_mac);
    frame.extend_from_slice(&ethertype.to_be_bytes());
    frame.extend_from_slice(payload);

    nic.send(&frame)
}

/// Broadcasts an ARP Request for the specified target IP address.
pub fn send_arp_request(target_ip: [u8; 4]) -> Result<(), &'static str> {
    let mut guard = E1000.lock();
    let nic = guard.as_mut().ok_or("No NIC present")?;
    let src_mac = nic.mac;
    drop(guard);

    let stack = NET.lock();
    let src_ip = stack.ip;
    drop(stack);

    let mut arp_packet = Vec::with_capacity(28);
    arp_packet.extend_from_slice(&1u16.to_be_bytes()); // Hardware: Ethernet (1)
    arp_packet.extend_from_slice(&0x0800u16.to_be_bytes()); // Protocol: IPv4 (0x0800)
    arp_packet.push(6); // HW Size: 6
    arp_packet.push(4); // Proto Size: 4
    arp_packet.extend_from_slice(&ARP_OP_REQUEST.to_be_bytes()); // Opcode: Request (1)
    arp_packet.extend_from_slice(&src_mac);
    arp_packet.extend_from_slice(&src_ip);
    arp_packet.extend_from_slice(&[0u8; 6]); // Target MAC unknown
    arp_packet.extend_from_slice(&target_ip);

    send_ethernet_frame(BROADCAST_MAC, ETHERTYPE_ARP, &arp_packet)
}

/// Sends an ARP Reply to the requested target.
pub fn send_arp_reply(target_mac: [u8; 6], target_ip: [u8; 4]) -> Result<(), &'static str> {
    let mut guard = E1000.lock();
    let nic = guard.as_mut().ok_or("No NIC present")?;
    let src_mac = nic.mac;
    drop(guard);

    let stack = NET.lock();
    let src_ip = stack.ip;
    drop(stack);

    let mut arp_packet = Vec::with_capacity(28);
    arp_packet.extend_from_slice(&1u16.to_be_bytes());
    arp_packet.extend_from_slice(&0x0800u16.to_be_bytes());
    arp_packet.push(6);
    arp_packet.push(4);
    arp_packet.extend_from_slice(&ARP_OP_REPLY.to_be_bytes()); // Opcode: Reply (2)
    arp_packet.extend_from_slice(&src_mac);
    arp_packet.extend_from_slice(&src_ip);
    arp_packet.extend_from_slice(&target_mac);
    arp_packet.extend_from_slice(&target_ip);

    send_ethernet_frame(target_mac, ETHERTYPE_ARP, &arp_packet)
}

/// Builds and transmits an IPv4 packet encapsulating a higher-level protocol.
pub fn send_ipv4_packet(dest_ip: [u8; 4], protocol: u8, payload: &[u8]) -> Result<(), &'static str> {
    let (src_ip, dest_mac) = {
        let stack = NET.lock();
        let src_ip = stack.ip;

        // Determine destination MAC: if broadcast, send to BROADCAST_MAC
        let mac = if dest_ip == [255, 255, 255, 255] {
            BROADCAST_MAC
        } else {
            let same_subnet = (dest_ip[0] & stack.subnet[0] == src_ip[0] & stack.subnet[0])
                && (dest_ip[1] & stack.subnet[1] == src_ip[1] & stack.subnet[1])
                && (dest_ip[2] & stack.subnet[2] == src_ip[2] & stack.subnet[2]);

            let target_for_mac = if same_subnet { dest_ip } else { stack.gateway };
            stack.lookup_arp(target_for_mac).unwrap_or_else(|| {
                // Default gateway MAC for VirtualBox NAT is 52:54:00:12:35:02
                [0x52, 0x54, 0x00, 0x12, 0x35, 0x02]
            })
        };
        (src_ip, mac)
    };

    let total_len = (20 + payload.len()) as u16;
    let mut ip_hdr = Vec::with_capacity(20);
    ip_hdr.push(0x45); // Version 4, IHL 5 (20 bytes)
    ip_hdr.push(0x00); // DSCP/ECN
    ip_hdr.extend_from_slice(&total_len.to_be_bytes());
    ip_hdr.extend_from_slice(&1234u16.to_be_bytes()); // ID
    ip_hdr.extend_from_slice(&0x4000u16.to_be_bytes()); // Don't fragment
    ip_hdr.push(64); // TTL
    ip_hdr.push(protocol);
    ip_hdr.extend_from_slice(&0u16.to_be_bytes()); // Checksum placeholder
    ip_hdr.extend_from_slice(&src_ip);
    ip_hdr.extend_from_slice(&dest_ip);

    let csum = calculate_checksum(&ip_hdr);
    ip_hdr[10] = (csum >> 8) as u8;
    ip_hdr[11] = (csum & 0xFF) as u8;

    let mut full_packet = Vec::with_capacity(20 + payload.len());
    full_packet.extend_from_slice(&ip_hdr);
    full_packet.extend_from_slice(payload);

    send_ethernet_frame(dest_mac, ETHERTYPE_IPV4, &full_packet)
}

/// Sends an ICMP Echo Request (Ping) to the target IP address.
pub fn send_icmp_ping(target_ip: [u8; 4], seq: u16) -> Result<(), &'static str> {
    let payload = b"TriOS Ping Packet v1.0 JOJIN";
    let mut icmp = Vec::with_capacity(8 + payload.len());
    icmp.push(ICMP_TYPE_ECHO_REQUEST); // Type 8
    icmp.push(0); // Code 0
    icmp.extend_from_slice(&0u16.to_be_bytes()); // Checksum placeholder
    icmp.extend_from_slice(&0x1337u16.to_be_bytes()); // Identifier
    icmp.extend_from_slice(&seq.to_be_bytes()); // Sequence number
    icmp.extend_from_slice(payload);

    let csum = calculate_checksum(&icmp);
    icmp[2] = (csum >> 8) as u8;
    icmp[3] = (csum & 0xFF) as u8;

    send_ipv4_packet(target_ip, IP_PROTO_ICMP, &icmp)
}

/// Builds and transmits a UDP datagram over IPv4.
pub fn send_udp_packet(dest_ip: [u8; 4], src_port: u16, dest_port: u16, payload: &[u8]) -> Result<(), &'static str> {
    let length = (8 + payload.len()) as u16;
    let mut udp = Vec::with_capacity(8 + payload.len());
    udp.extend_from_slice(&src_port.to_be_bytes());
    udp.extend_from_slice(&dest_port.to_be_bytes());
    udp.extend_from_slice(&length.to_be_bytes());
    udp.extend_from_slice(&0u16.to_be_bytes()); // Checksum optional in IPv4 UDP
    udp.extend_from_slice(payload);
    send_ipv4_packet(dest_ip, IP_PROTO_UDP, &udp)
}

/// Resolves a domain name into an IPv4 address using DNS over UDP (RFC 1035).
pub fn resolve_dns(hostname: &str) -> Result<[u8; 4], &'static str> {
    let dns_server = NET.lock().dns;
    let q_id = 0x5432u16;

    let mut query = Vec::with_capacity(64);
    // Header (12 bytes)
    query.extend_from_slice(&q_id.to_be_bytes());
    query.extend_from_slice(&0x0100u16.to_be_bytes()); // Standard query, recursion desired
    query.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT: 1
    query.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT: 0
    query.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT: 0
    query.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT: 0

    // Question: QNAME
    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err("Invalid domain label");
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0); // Root label
    query.extend_from_slice(&1u16.to_be_bytes()); // Type A (1)
    query.extend_from_slice(&1u16.to_be_bytes()); // Class IN (1)

    // Clear previous responses for this ID
    {
        let mut net = NET.lock();
        net.dns_replies.retain(|(id, _)| *id != q_id);
    }

    // Send query from UDP port 53535 to DNS server port 53
    send_udp_packet(dns_server, 53535, 53, &query)?;

    // Poll network for answer
    for _ in 0..600_000 {
        poll_network();
        let net = NET.lock();
        for (id, ip) in &net.dns_replies {
            if *id == q_id {
                return Ok(*ip);
            }
        }
        core::hint::spin_loop();
    }

    Err("DNS resolution timed out")
}

/// Processes an incoming Ethernet frame and dispatches to ARP or IPv4 handlers.
pub fn handle_packet(frame: &[u8]) {
    if frame.len() < 14 {
        return;
    }

    let _dest_mac: [u8; 6] = frame[0..6].try_into().unwrap_or([0; 6]);
    let src_mac: [u8; 6] = frame[6..12].try_into().unwrap_or([0; 6]);
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    let payload = &frame[14..];

    match ethertype {
        ETHERTYPE_ARP => {
            handle_arp_packet(src_mac, payload);
        }
        ETHERTYPE_IPV4 => {
            handle_ipv4_packet(src_mac, payload);
        }
        _ => {}
    }
}

fn handle_arp_packet(src_mac: [u8; 6], payload: &[u8]) {
    if payload.len() < 28 {
        return;
    }

    let hw_type = u16::from_be_bytes([payload[0], payload[1]]);
    let proto_type = u16::from_be_bytes([payload[2], payload[3]]);
    let opcode = u16::from_be_bytes([payload[6], payload[7]]);

    if hw_type != 1 || proto_type != 0x0800 {
        return;
    }

    let sender_mac: [u8; 6] = payload[8..14].try_into().unwrap_or(src_mac);
    let sender_ip: [u8; 4] = payload[14..18].try_into().unwrap_or([0; 4]);
    let _target_mac: [u8; 6] = payload[18..24].try_into().unwrap_or([0; 6]);
    let target_ip: [u8; 4] = payload[24..28].try_into().unwrap_or([0; 4]);

    NET.lock().update_arp(sender_ip, sender_mac);

    if opcode == ARP_OP_REQUEST {
        let our_ip = NET.lock().ip;
        if target_ip == our_ip {
            serial_println!(
                "[net    ] responding to ARP request from {}.{}.{}.{}",
                sender_ip[0], sender_ip[1], sender_ip[2], sender_ip[3]
            );
            let _ = send_arp_reply(sender_mac, sender_ip);
        }
    } else if opcode == ARP_OP_REPLY {
        serial_println!(
            "[net    ] received ARP reply from {}.{}.{}.{}",
            sender_ip[0], sender_ip[1], sender_ip[2], sender_ip[3]
        );
    }
}

fn handle_ipv4_packet(src_mac: [u8; 6], payload: &[u8]) {
    if payload.len() < 20 {
        return;
    }

    let version_ihl = payload[0];
    let version = version_ihl >> 4;
    let ihl = (version_ihl & 0x0F) as usize * 4;

    if version != 4 || payload.len() < ihl {
        return;
    }

    let total_len = u16::from_be_bytes([payload[2], payload[3]]) as usize;
    let ttl = payload[8];
    let protocol = payload[9];
    let src_ip: [u8; 4] = payload[12..16].try_into().unwrap_or([0; 4]);
    let dest_ip: [u8; 4] = payload[16..20].try_into().unwrap_or([0; 4]);

    // Automatically learn ARP entry from any incoming IPv4 packet
    if src_ip != [0, 0, 0, 0] && src_mac != [0; 6] {
        NET.lock().update_arp(src_ip, src_mac);
    }

    let our_ip = NET.lock().ip;
    if dest_ip != our_ip && dest_ip != [255, 255, 255, 255] {
        return;
    }

    let ip_payload_len = if total_len > ihl && total_len <= payload.len() {
        total_len - ihl
    } else {
        payload.len() - ihl
    };
    let ip_payload = &payload[ihl..ihl + ip_payload_len];

    match protocol {
        IP_PROTO_ICMP => {
            handle_icmp_packet(src_ip, ttl, ip_payload);
        }
        IP_PROTO_UDP => {
            handle_udp_packet(src_ip, ip_payload);
        }
        IP_PROTO_TCP => {
            tcp::handle_tcp_packet(src_ip, ip_payload);
        }
        _ => {}
    }
}

fn handle_icmp_packet(src_ip: [u8; 4], ttl: u8, payload: &[u8]) {
    if payload.len() < 8 {
        return;
    }

    let icmp_type = payload[0];
    let _code = payload[1];
    let seq = u16::from_be_bytes([payload[6], payload[7]]);

    if icmp_type == ICMP_TYPE_ECHO_REQUEST {
        // Echo Request received: send Echo Reply
        serial_println!(
            "[net    ] ICMP Echo Request from {}.{}.{}.{} seq={seq}, sending Reply",
            src_ip[0], src_ip[1], src_ip[2], src_ip[3]
        );
        let mut reply = payload.to_vec();
        reply[0] = ICMP_TYPE_ECHO_REPLY;
        reply[2] = 0;
        reply[3] = 0;
        let csum = calculate_checksum(&reply);
        reply[2] = (csum >> 8) as u8;
        reply[3] = (csum & 0xFF) as u8;

        let _ = send_ipv4_packet(src_ip, IP_PROTO_ICMP, &reply);
    } else if icmp_type == ICMP_TYPE_ECHO_REPLY {
        serial_println!(
            "[net    ] ICMP Echo Reply from {}.{}.{}.{} seq={seq} ttl={ttl}",
            src_ip[0], src_ip[1], src_ip[2], src_ip[3]
        );
        let mut stack = NET.lock();
        stack.ping_replies.push(PingReplyInfo {
            from_ip: src_ip,
            seq,
            ttl,
        });
    }
}

fn handle_udp_packet(_src_ip: [u8; 4], payload: &[u8]) {
    if payload.len() < 8 {
        return;
    }
    let src_port = u16::from_be_bytes([payload[0], payload[1]]);
    let dest_port = u16::from_be_bytes([payload[2], payload[3]]);
    let data = &payload[8..];

    if src_port == 53 || dest_port == 53535 {
        handle_dns_response(data);
    } else if src_port == 67 || dest_port == 68 {
        dhcp::handle_dhcp_packet(data);
    }
}

fn handle_dns_response(data: &[u8]) {
    if data.len() < 12 {
        return;
    }
    let id = u16::from_be_bytes([data[0], data[1]]);
    let flags = u16::from_be_bytes([data[2], data[3]]);
    let qdcount = u16::from_be_bytes([data[4], data[5]]);
    let ancount = u16::from_be_bytes([data[6], data[7]]);

    if (flags & 0x8000) == 0 || ancount == 0 {
        return;
    }

    let mut pos = 12;
    // Skip Question section
    for _ in 0..qdcount {
        while pos < data.len() {
            let len = data[pos] as usize;
            if len == 0 {
                pos += 1;
                break;
            }
            if (len & 0xC0) == 0xC0 {
                pos += 2;
                break;
            }
            pos += 1 + len;
        }
        pos += 4; // QTYPE + QCLASS
        if pos >= data.len() {
            return;
        }
    }

    // Parse Answers
    for _ in 0..ancount {
        if pos >= data.len() {
            break;
        }
        if (data[pos] & 0xC0) == 0xC0 {
            pos += 2;
        } else {
            while pos < data.len() {
                let len = data[pos] as usize;
                if len == 0 {
                    pos += 1;
                    break;
                }
                pos += 1 + len;
            }
        }
        if pos + 10 > data.len() {
            break;
        }
        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let rdlen = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;

        if rtype == 1 && rdlen == 4 && pos + 4 <= data.len() {
            let ip = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
            crate::serial_println!(
                "[dns    ] resolved ID={:#06x} -> {}.{}.{}.{}",
                id, ip[0], ip[1], ip[2], ip[3]
            );
            let mut net = NET.lock();
            net.dns_replies.push((id, ip));
            return;
        }
        pos += rdlen;
    }
}

/// Polls the network interface for pending packets and handles them.
pub fn poll_network() {
    loop {
        let packet = {
            let mut guard = E1000.lock();
            match guard.as_mut() {
                Some(nic) => nic.receive(),
                None => None,
            }
        };

        match packet {
            Some(pkt) => handle_packet(&pkt),
            None => break,
        }
    }
}
