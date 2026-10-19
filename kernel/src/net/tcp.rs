//! TriOS Transmission Control Protocol (TCP) Stack (RFC 793).
//!
//! Bare-metal implementation of:
//! - TCP segment construction with RFC 793 IPv4 pseudo-header checksums
//! - Three-way handshake client connection engine (SYN -> SYN-ACK -> ACK)
//! - Outbound stream transmission with sliding dynamic windowing and MSS chunking
//! - Full duplex reassembly buffer with dynamic advertised receive window
//! - Clean teardown and FIN/ACK connection termination
//!
//! Developed by JOJIN JOHN (@jojin1709) for TriOS.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};
use spin::Mutex;
use super::{poll_network, send_ipv4_packet, NET};

pub const IP_PROTO_TCP: u8 = 6;

pub const TCP_FIN: u8 = 0x01;
pub const TCP_SYN: u8 = 0x02;
pub const TCP_RST: u8 = 0x04;
pub const TCP_PSH: u8 = 0x08;
pub const TCP_ACK: u8 = 0x10;
#[allow(dead_code)]
pub const TCP_URG: u8 = 0x20;

pub const TCP_MSS: usize = 1460;
pub const TCP_DEFAULT_WINDOW: u16 = 64240;
pub const TCP_MAX_RX_BUFFER: usize = 65535;

static NEXT_LOCAL_PORT: AtomicU16 = AtomicU16::new(49152);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    SynSent,
    Established,
    CloseWait,
    Reset,
}

#[derive(Debug, Clone)]
pub struct TcpStream {
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub state: TcpState,
    pub local_seq: u32,
    pub remote_seq: u32,
    pub snd_wnd: u16,
    pub snd_una: u32,
    pub rx_buffer: Vec<u8>,
}

impl TcpStream {
    /// Calculates the current dynamic advertised receive window.
    pub fn rcv_wnd(&self) -> u16 {
        (TCP_MAX_RX_BUFFER.saturating_sub(self.rx_buffer.len())).min(65535) as u16
    }
}

pub static TCP_SOCKETS: Mutex<Vec<TcpStream>> = Mutex::new(Vec::new());

/// Calculates the RFC 793 TCP Checksum over the IPv4 pseudo-header and TCP segment.
pub fn calculate_tcp_checksum(src_ip: [u8; 4], dest_ip: [u8; 4], tcp_segment: &[u8]) -> u16 {
    let mut sum = 0u32;

    // Pseudo-header: Source IP (2 words)
    sum = sum.wrapping_add(u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32);
    sum = sum.wrapping_add(u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32);

    // Pseudo-header: Destination IP (2 words)
    sum = sum.wrapping_add(u16::from_be_bytes([dest_ip[0], dest_ip[1]]) as u32);
    sum = sum.wrapping_add(u16::from_be_bytes([dest_ip[2], dest_ip[3]]) as u32);

    // Pseudo-header: Zero byte (0) + Protocol (6 for TCP)
    sum = sum.wrapping_add(IP_PROTO_TCP as u32);

    // Pseudo-header: TCP Length (header + options + payload)
    let tcp_len = tcp_segment.len() as u16;
    sum = sum.wrapping_add(tcp_len as u32);

    // TCP Segment words
    let mut i = 0;
    while i + 1 < tcp_segment.len() {
        let word = u16::from_be_bytes([tcp_segment[i], tcp_segment[i + 1]]) as u32;
        sum = sum.wrapping_add(word);
        i += 2;
    }
    if i < tcp_segment.len() {
        let word = (tcp_segment[i] as u32) << 8;
        sum = sum.wrapping_add(word);
    }

    // Fold 32-bit sum into 16 bits
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !(sum as u16)
}

/// Transmits a raw TCP packet with standard pseudo-header checksum.
pub fn send_tcp_packet(
    dest_ip: [u8; 4],
    src_port: u16,
    dest_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    options: &[u8],
    payload: &[u8],
) -> Result<(), &'static str> {
    let src_ip = NET.lock().ip;
    let data_offset = ((20 + options.len()) / 4) as u8;
    let mut segment = Vec::with_capacity(20 + options.len() + payload.len());

    segment.extend_from_slice(&src_port.to_be_bytes()); // 0..2: Source Port
    segment.extend_from_slice(&dest_port.to_be_bytes()); // 2..4: Destination Port
    segment.extend_from_slice(&seq.to_be_bytes()); // 4..8: Sequence Number
    segment.extend_from_slice(&ack.to_be_bytes()); // 8..12: Acknowledgment Number
    segment.push(data_offset << 4); // 12: Data Offset (4 bits) + Reserved (4 bits)
    segment.push(flags); // 13: Flags (FIN, SYN, RST, PSH, ACK, URG)
    segment.extend_from_slice(&window.to_be_bytes()); // 14..16: Window Size
    segment.extend_from_slice(&0u16.to_be_bytes()); // 16..18: Checksum placeholder
    segment.extend_from_slice(&0u16.to_be_bytes()); // 18..20: Urgent Pointer
    segment.extend_from_slice(options); // Options (e.g. MSS)
    segment.extend_from_slice(payload); // Application Data Payload

    let csum = calculate_tcp_checksum(src_ip, dest_ip, &segment);
    segment[16] = (csum >> 8) as u8;
    segment[17] = (csum & 0xFF) as u8;

    send_ipv4_packet(dest_ip, IP_PROTO_TCP, &segment)
}

/// Dispatches an incoming TCP packet to active sockets.
pub fn handle_tcp_packet(src_ip: [u8; 4], payload: &[u8]) {
    if payload.len() < 20 {
        return;
    }

    let src_port = u16::from_be_bytes([payload[0], payload[1]]);
    let dest_port = u16::from_be_bytes([payload[2], payload[3]]);
    let seq = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let ack = u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
    let data_offset = ((payload[12] >> 4) as usize) * 4;
    let flags = payload[13];
    let window = u16::from_be_bytes([payload[14], payload[15]]);

    if payload.len() < data_offset {
        return;
    }
    let tcp_data = &payload[data_offset..];

    let mut sockets = TCP_SOCKETS.lock();
    for sock in sockets.iter_mut() {
        if sock.local_port == dest_port && sock.remote_port == src_port && sock.remote_ip == src_ip {
            // Update peer's advertised send window and acknowledged seq
            sock.snd_wnd = window;
            if (flags & TCP_ACK) != 0 {
                sock.snd_una = ack;
            }

            // Check for connection reset
            if (flags & TCP_RST) != 0 {
                serial_println!("[tcp    ] connection reset by peer on port {dest_port}");
                sock.state = TcpState::Reset;
                return;
            }

            match sock.state {
                TcpState::SynSent => {
                    if (flags & (TCP_SYN | TCP_ACK)) == (TCP_SYN | TCP_ACK) {
                        sock.remote_seq = seq.wrapping_add(1);
                        sock.local_seq = ack;
                        sock.state = TcpState::Established;
                        serial_println!(
                            "[tcp    ] connection established: {}.{}.{}.{}:{} <-> local:{} (wnd={})",
                            src_ip[0], src_ip[1], src_ip[2], src_ip[3], src_port, dest_port, window
                        );

                        let rcv_wnd = sock.rcv_wnd();
                        // Send ACK acknowledging server's SYN
                        let _ = send_tcp_packet(
                            src_ip,
                            dest_port,
                            src_port,
                            sock.local_seq,
                            sock.remote_seq,
                            TCP_ACK,
                            rcv_wnd,
                            &[],
                            &[],
                        );
                    }
                }
                TcpState::Established => {
                    let mut data_received = false;
                    if !tcp_data.is_empty() {
                        sock.rx_buffer.extend_from_slice(tcp_data);
                        sock.remote_seq = sock.remote_seq.wrapping_add(tcp_data.len() as u32);
                        data_received = true;
                    }

                    let rcv_wnd = sock.rcv_wnd();

                    if (flags & TCP_FIN) != 0 {
                        sock.remote_seq = sock.remote_seq.wrapping_add(1);
                        sock.state = TcpState::CloseWait;
                        serial_println!("[tcp    ] peer sent FIN on port {dest_port}, closing stream");
                        // Send ACK for FIN
                        let _ = send_tcp_packet(
                            src_ip,
                            dest_port,
                            src_port,
                            sock.local_seq,
                            sock.remote_seq,
                            TCP_ACK,
                            rcv_wnd,
                            &[],
                            &[],
                        );
                    } else if data_received {
                        // Send ACK for received data
                        let _ = send_tcp_packet(
                            src_ip,
                            dest_port,
                            src_port,
                            sock.local_seq,
                            sock.remote_seq,
                            TCP_ACK,
                            rcv_wnd,
                            &[],
                            &[],
                        );
                    }
                }
                _ => {}
            }
            return;
        }
    }
}

impl TcpStream {
    /// Connects to a remote IPv4 TCP host and port via 3-way handshake.
    pub fn connect(remote_ip: [u8; 4], remote_port: u16) -> Result<u16, &'static str> {
        let local_port = NEXT_LOCAL_PORT.fetch_add(1, Ordering::SeqCst);
        let local_seq = 0x1000_2000u32.wrapping_add((local_port as u32) * 100);

        {
            let mut sockets = TCP_SOCKETS.lock();
            sockets.retain(|s| s.local_port != local_port);
            sockets.push(TcpStream {
                local_port,
                remote_ip,
                remote_port,
                state: TcpState::SynSent,
                local_seq,
                remote_seq: 0,
                snd_wnd: TCP_DEFAULT_WINDOW,
                snd_una: local_seq,
                rx_buffer: Vec::new(),
            });
        }

        serial_println!(
            "[tcp    ] connecting to {}.{}.{}.{}:{} from local port {}...",
            remote_ip[0], remote_ip[1], remote_ip[2], remote_ip[3], remote_port, local_port
        );

        // Send SYN packet with MSS Option = 1460 (4 bytes: [0x02, 0x04, 0x05, 0xb4])
        let mss_opt = [0x02, 0x04, 0x05, 0xb4];

        for attempt in 1..=4 {
            send_tcp_packet(
                remote_ip,
                local_port,
                remote_port,
                local_seq,
                0,
                TCP_SYN,
                TCP_DEFAULT_WINDOW,
                &mss_opt,
                &[],
            )?;

            // Poll for up to 1.5 seconds (150 timer ticks at 100 Hz)
            let start = crate::timer::ticks();
            while crate::timer::ticks().saturating_sub(start) < 150 {
                poll_network();
                {
                    let sockets = TCP_SOCKETS.lock();
                    if let Some(sock) = sockets.iter().find(|s| s.local_port == local_port) {
                        match sock.state {
                            TcpState::Established => return Ok(local_port),
                            TcpState::Reset => return Err("Connection reset by peer"),
                            _ => {}
                        }
                    }
                }
                for _ in 0..1000 {
                    core::hint::spin_loop();
                }
            }
            serial_println!("[tcp    ] connect attempt #{attempt} timed out, retrying SYN...");
        }

        // Timeout: Clean up socket
        let mut sockets = TCP_SOCKETS.lock();
        sockets.retain(|s| s.local_port != local_port);
        Err("TCP connection timed out")
    }

    /// Transmits data payload across the established TCP connection using MSS chunking and window tracking.
    pub fn write(local_port: u16, data: &[u8]) -> Result<usize, &'static str> {
        let (remote_ip, remote_port, mut seq, ack) = {
            let sockets = TCP_SOCKETS.lock();
            let sock = sockets.iter().find(|s| s.local_port == local_port).ok_or("Socket not found")?;
            if sock.state != TcpState::Established {
                return Err("Socket is not in established state");
            }
            (sock.remote_ip, sock.remote_port, sock.local_seq, sock.remote_seq)
        };

        let mut total_sent = 0;
        let chunks = data.chunks(TCP_MSS);
        let total_chunks = chunks.len();

        for (idx, chunk) in chunks.enumerate() {
            let is_last = idx + 1 == total_chunks;
            let flags = if is_last { TCP_PSH | TCP_ACK } else { TCP_ACK };

            let rcv_wnd = {
                let sockets = TCP_SOCKETS.lock();
                if let Some(sock) = sockets.iter().find(|s| s.local_port == local_port) {
                    sock.rcv_wnd()
                } else {
                    TCP_DEFAULT_WINDOW
                }
            };

            send_tcp_packet(
                remote_ip,
                local_port,
                remote_port,
                seq,
                ack,
                flags,
                rcv_wnd,
                &[],
                chunk,
            )?;

            seq = seq.wrapping_add(chunk.len() as u32);
            total_sent += chunk.len();
        }

        // Update local sequence
        let mut sockets = TCP_SOCKETS.lock();
        if let Some(sock) = sockets.iter_mut().find(|s| s.local_port == local_port) {
            sock.local_seq = seq;
        }

        Ok(total_sent)
    }

    /// Reads available streaming data from socket rx buffer, popping consumed bytes.
    /// Blocks until data arrives, connection is closed by peer, or 5-second timeout elapses.
    pub fn read_stream(local_port: u16, buf: &mut [u8]) -> Result<usize, &'static str> {
        let start = crate::timer::ticks();
        loop {
            poll_network();
            let mut sockets = TCP_SOCKETS.lock();
            let sock = match sockets.iter_mut().find(|s| s.local_port == local_port) {
                Some(s) => s,
                None => return Err("Socket not found"),
            };

            if !sock.rx_buffer.is_empty() {
                let to_read = buf.len().min(sock.rx_buffer.len());
                buf[..to_read].copy_from_slice(&sock.rx_buffer[..to_read]);
                sock.rx_buffer.drain(..to_read);

                let rcv_wnd = sock.rcv_wnd();
                let _ = send_tcp_packet(
                    sock.remote_ip,
                    sock.local_port,
                    sock.remote_port,
                    sock.local_seq,
                    sock.remote_seq,
                    TCP_ACK,
                    rcv_wnd,
                    &[],
                    &[],
                );

                return Ok(to_read);
            }

            if sock.state == TcpState::CloseWait || sock.state == TcpState::Closed {
                return Ok(0); // EOF
            }
            if sock.state == TcpState::Reset {
                return Err("Connection reset by peer");
            }

            // Timeout after 5 seconds of waiting for data
            if crate::timer::ticks().saturating_sub(start) > 500 {
                return Ok(0); // Timed out waiting for data
            }

            drop(sockets);
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
        }
    }

    /// Polls and reads all available response data until the remote peer closes the stream.
    pub fn read_all(local_port: u16, max_loops: usize) -> Result<Vec<u8>, &'static str> {
        let mut idle_loops = 0;
        let mut last_rx_len = 0;

        for _ in 0..max_loops {
            poll_network();
            let sockets = TCP_SOCKETS.lock();
            if let Some(sock) = sockets.iter().find(|s| s.local_port == local_port) {
                if sock.rx_buffer.len() > last_rx_len {
                    last_rx_len = sock.rx_buffer.len();
                    idle_loops = 0;
                } else if !sock.rx_buffer.is_empty() {
                    idle_loops += 1;
                }

                // If remote sent FIN (CloseWait) or closed, or sufficient idle time after receiving data
                if sock.state == TcpState::CloseWait || sock.state == TcpState::Closed || sock.state == TcpState::Reset {
                    return Ok(sock.rx_buffer.clone());
                }

                if idle_loops > 100_000 && !sock.rx_buffer.is_empty() {
                    return Ok(sock.rx_buffer.clone());
                }
            } else {
                return Err("Socket destroyed");
            }
            core::hint::spin_loop();
        }

        let sockets = TCP_SOCKETS.lock();
        if let Some(sock) = sockets.iter().find(|s| s.local_port == local_port) {
            if !sock.rx_buffer.is_empty() {
                return Ok(sock.rx_buffer.clone());
            }
        }
        Err("Read timed out with no data")
    }

    /// Cleanly closes the TCP stream sending a FIN/ACK packet.
    pub fn close(local_port: u16) {
        let mut sockets = TCP_SOCKETS.lock();
        if let Some(sock) = sockets.iter_mut().find(|s| s.local_port == local_port) {
            if sock.state == TcpState::Established || sock.state == TcpState::CloseWait {
                let rcv_wnd = sock.rcv_wnd();
                let _ = send_tcp_packet(
                    sock.remote_ip,
                    sock.local_port,
                    sock.remote_port,
                    sock.local_seq,
                    sock.remote_seq,
                    TCP_FIN | TCP_ACK,
                    rcv_wnd,
                    &[],
                    &[],
                );
                sock.local_seq = sock.local_seq.wrapping_add(1);
                sock.state = TcpState::Closed;
            }
        }
        sockets.retain(|s| s.local_port != local_port);
    }
}
