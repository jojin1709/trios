//! TriOS HTTP/1.0 Web Client.
//!
//! Provides bare-metal HTTP GET request handling, URL parsing,
//! DNS resolution integration, header parsing, and response payload decoding.
//!
//! Developed by JOJIN JOHN (@jojin1709) for TriOS.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use super::resolve_dns;
use super::tcp::TcpStream;

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status_code: u16,
    pub status_text: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// Parses a string representation of an IPv4 address into 4 bytes.
fn parse_ipv4_addr(s: &str) -> Option<[u8; 4]> {
    let mut parts = [0u8; 4];
    let mut count = 0;
    for segment in s.split('.') {
        if count >= 4 {
            return None;
        }
        let val: u8 = segment.parse().ok()?;
        parts[count] = val;
        count += 1;
    }
    if count == 4 {
        Some(parts)
    } else {
        None
    }
}

/// Executes an HTTP/1.0 GET request against a target URL or host.
pub fn http_get(url: &str) -> Result<HttpResponse, &'static str> {
    // 1. Strip protocol prefix if present
    let raw_url = if let Some(stripped) = url.strip_prefix("http://") {
        stripped
    } else {
        url
    };

    // 2. Separate host and path
    let (host_port, path) = match raw_url.find('/') {
        Some(idx) => (&raw_url[..idx], &raw_url[idx..]),
        None => (raw_url, "/"),
    };

    // 3. Separate host and port
    let (host, port) = match host_port.find(':') {
        Some(idx) => {
            let h = &host_port[..idx];
            let p: u16 = host_port[idx + 1..].parse().map_err(|_| "Invalid port number")?;
            (h, p)
        }
        None => (host_port, 80),
    };

    if host.is_empty() {
        return Err("Empty hostname");
    }

    // 4. Resolve host to IP address
    let target_ip = if let Some(ip) = parse_ipv4_addr(host) {
        ip
    } else {
        serial_println!("[http   ] resolving domain '{host}' via DNS...");
        resolve_dns(host)?
    };

    serial_println!(
        "[http   ] initiating HTTP GET to {}.{}.{}.{}:{} path='{}'...",
        target_ip[0], target_ip[1], target_ip[2], target_ip[3], port, path
    );

    // 5. Connect via TCP
    let sock = TcpStream::connect(target_ip, port)?;

    // 6. Build HTTP/1.0 GET Request
    let request = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: TriOS-HTTP/1.0 (JOJIN JOHN)\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path, host
    );

    // 7. Transmit Request
    TcpStream::write(sock, request.as_bytes())?;

    // 8. Read complete HTTP response
    let raw_response = TcpStream::read_all(sock, 1_500_000)?;
    TcpStream::close(sock);

    // 9. Parse response header and body
    // Find header boundary (\r\n\r\n or \n\n)
    let body_start = if let Some(pos) = raw_response.windows(4).position(|w| w == b"\r\n\r\n") {
        pos + 4
    } else if let Some(pos) = raw_response.windows(2).position(|w| w == b"\n\n") {
        pos + 2
    } else {
        raw_response.len()
    };

    let header_bytes = &raw_response[..body_start.min(raw_response.len())];
    let body = raw_response[body_start.min(raw_response.len())..].to_vec();

    // Parse status line
    let header_str = core::str::from_utf8(header_bytes).unwrap_or("");
    let mut status_code = 200u16;
    let mut status_text = String::from("OK");
    let mut content_type = String::from("text/html");

    let mut lines = header_str.lines();
    if let Some(status_line) = lines.next() {
        let mut tokens = status_line.split_whitespace();
        let _proto = tokens.next();
        if let Some(code_str) = tokens.next() {
            if let Ok(c) = code_str.parse::<u16>() {
                status_code = c;
            }
        }
        let rest: Vec<&str> = tokens.collect();
        if !rest.is_empty() {
            status_text = rest.join(" ");
        }
    }

    // Parse header fields
    for line in lines {
        if let Some(idx) = line.find(':') {
            let key = line[..idx].trim();
            let val = line[idx + 1..].trim();
            if key.eq_ignore_ascii_case("content-type") {
                content_type = String::from(val);
            }
        }
    }

    serial_println!(
        "[http   ] response: {} {} ({} bytes body, content-type: {})",
        status_code, status_text, body.len(), content_type
    );

    Ok(HttpResponse {
        status_code,
        status_text,
        content_type,
        body,
    })
}
