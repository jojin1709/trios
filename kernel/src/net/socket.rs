//! BSD-style Socket Abstraction Layer for TriOS.
//!
//! Provides kernel-level socket management:
//! - Domain: AF_INET (2)
//! - Types: SOCK_STREAM (1) [TCP], SOCK_DGRAM (2) [UDP]
//! - Operations: socket, connect, send, recv, close
//!
//! Developed by JOJIN JOHN (@jojin1709) for TriOS.

use alloc::vec::Vec;
use spin::Mutex;
use super::tcp::TcpStream;

pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Socket {
    pub id: usize,
    pub domain: i32,
    pub sock_type: i32,
    pub protocol: i32,
    pub tcp_port: Option<u16>,
    pub udp_port: Option<u16>,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub connected: bool,
}

pub static SOCKET_TABLE: Mutex<Vec<Option<Socket>>> = Mutex::new(Vec::new());

/// Creates a new socket descriptor. Returns socket ID or an error string.
pub fn socket_create(domain: i32, sock_type: i32, protocol: i32) -> Result<usize, &'static str> {
    if domain != AF_INET {
        return Err("Only AF_INET (IPv4) domain is supported");
    }
    if sock_type != SOCK_STREAM && sock_type != SOCK_DGRAM {
        return Err("Only SOCK_STREAM and SOCK_DGRAM socket types are supported");
    }

    let mut table = SOCKET_TABLE.lock();
    let id = match table.iter().position(|s| s.is_none()) {
        Some(idx) => idx,
        None => {
            let idx = table.len();
            table.push(None);
            idx
        }
    };

    table[id] = Some(Socket {
        id,
        domain,
        sock_type,
        protocol,
        tcp_port: None,
        udp_port: None,
        remote_ip: [0; 4],
        remote_port: 0,
        connected: false,
    });

    serial_println!("[socket ] created socket #{} (domain={}, type={}, proto={})", id, domain, sock_type, protocol);
    Ok(id)
}

/// Initiates a connection on a socket to `dest_ip`:`dest_port`.
pub fn socket_connect(sock_id: usize, dest_ip: [u8; 4], dest_port: u16) -> Result<(), &'static str> {
    let sock_type = {
        let table = SOCKET_TABLE.lock();
        let sock = table.get(sock_id).and_then(|s| s.as_ref()).ok_or("Invalid socket ID")?;
        sock.sock_type
    };

    if sock_type == SOCK_STREAM {
        let local_port = TcpStream::connect(dest_ip, dest_port)?;
        let mut table = SOCKET_TABLE.lock();
        if let Some(Some(sock)) = table.get_mut(sock_id) {
            sock.tcp_port = Some(local_port);
            sock.remote_ip = dest_ip;
            sock.remote_port = dest_port;
            sock.connected = true;
            serial_println!(
                "[socket ] socket #{} connected to {}.{}.{}.{}:{} (local port {})",
                sock_id, dest_ip[0], dest_ip[1], dest_ip[2], dest_ip[3], dest_port, local_port
            );
            return Ok(());
        }
        TcpStream::close(local_port);
        Err("Socket disappeared during connect")
    } else if sock_type == SOCK_DGRAM {
        let mut table = SOCKET_TABLE.lock();
        if let Some(Some(sock)) = table.get_mut(sock_id) {
            sock.remote_ip = dest_ip;
            sock.remote_port = dest_port;
            sock.connected = true;
            return Ok(());
        }
        Err("Invalid socket")
    } else {
        Err("Unsupported socket type")
    }
}

/// Transmits data on an active socket.
pub fn socket_send(sock_id: usize, data: &[u8]) -> Result<usize, &'static str> {
    let (sock_type, tcp_port, remote_ip, remote_port) = {
        let table = SOCKET_TABLE.lock();
        let sock = table.get(sock_id).and_then(|s| s.as_ref()).ok_or("Invalid socket ID")?;
        if !sock.connected {
            return Err("Socket is not connected");
        }
        (sock.sock_type, sock.tcp_port, sock.remote_ip, sock.remote_port)
    };

    if sock_type == SOCK_STREAM {
        let port = tcp_port.ok_or("No TCP port bound")?;
        TcpStream::write(port, data)
    } else if sock_type == SOCK_DGRAM {
        super::send_udp_packet(remote_ip, 49150, remote_port, data)?;
        Ok(data.len())
    } else {
        Err("Unsupported socket type")
    }
}

/// Receives available data from a socket.
pub fn socket_recv(sock_id: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    let (sock_type, tcp_port) = {
        let table = SOCKET_TABLE.lock();
        let sock = table.get(sock_id).and_then(|s| s.as_ref()).ok_or("Invalid socket ID")?;
        (sock.sock_type, sock.tcp_port)
    };

    if sock_type == SOCK_STREAM {
        let port = tcp_port.ok_or("No TCP port bound")?;
        TcpStream::read_stream(port, buf)
    } else {
        Err("UDP recv not implemented yet")
    }
}

/// Closes an active socket and releases its resources.
pub fn socket_close(sock_id: usize) {
    let mut table = SOCKET_TABLE.lock();
    if sock_id < table.len() {
        if let Some(sock) = table[sock_id].take() {
            if let Some(port) = sock.tcp_port {
                serial_println!("[socket ] closing socket #{} (port {})", sock_id, port);
                TcpStream::close(port);
            }
        }
    }
}
