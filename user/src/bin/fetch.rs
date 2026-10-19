#![no_std]
#![no_main]

extern crate alloc;
use alloc::format;
use libtrios::net::TcpStream;
use libtrios::*;

trios_main!(main);

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut idx = 0;
    let mut current = 0u32;
    let mut has_digit = false;

    for &b in s.as_bytes() {
        if b == b'.' {
            if !has_digit || idx >= 3 || current > 255 {
                return None;
            }
            octets[idx] = current as u8;
            idx += 1;
            current = 0;
            has_digit = false;
        } else if b.is_ascii_digit() {
            current = current * 10 + (b - b'0') as u32;
            has_digit = true;
            if current > 255 {
                return None;
            }
        } else {
            return None;
        }
    }

    if !has_digit || idx != 3 || current > 255 {
        return None;
    }
    octets[3] = current as u8;
    Some(octets)
}

fn parse_port(s: &str) -> Option<u16> {
    let mut port = 0u32;
    for &b in s.as_bytes() {
        if b.is_ascii_digit() {
            port = port * 10 + (b - b'0') as u32;
            if port > 65535 {
                return None;
            }
        } else {
            return None;
        }
    }
    if port == 0 {
        None
    } else {
        Some(port as u16)
    }
}

fn main(args: &[&str]) -> i32 {
    let ip = if args.len() > 1 {
        match parse_ipv4(args[1]) {
            Some(addr) => addr,
            None => {
                eprintln!("fetch: invalid IPv4 address: '{}'", args[1]);
                return 1;
            }
        }
    } else {
        [1, 1, 1, 1] // Default to 1.1.1.1 (Cloudflare Internet HTTP)
    };

    let port = if args.len() > 2 {
        match parse_port(args[2]) {
            Some(p) => p,
            None => {
                eprintln!("fetch: invalid port: '{}'", args[2]);
                return 1;
            }
        }
    } else {
        80 // Default HTTP port 80
    };

    let path = if args.len() > 3 {
        args[3]
    } else {
        "/"
    };

    println!("[fetch] connecting to {}.{}.{}.{}:{}...", ip[0], ip[1], ip[2], ip[3], port);

    let mut stream = match TcpStream::connect(ip, port) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("fetch: connection failed: {err}");
            return 1;
        }
    };

    println!("[fetch] connected! Sending HTTP GET {}...", path);

    let req = format!("GET {} HTTP/1.0\r\nHost: {}.{}.{}.{}\r\nUser-Agent: TriOS-Fetch/2.0\r\nConnection: close\r\n\r\n", path, ip[0], ip[1], ip[2], ip[3]);
    if let Err(err) = stream.write(req.as_bytes()) {
        eprintln!("fetch: write error: {err}");
        return 1;
    }

    println!("[fetch] receiving response:\n---");

    let mut buf = [0u8; 512];
    let mut total_bytes = 0;

    loop {
        match stream.read(&mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => {
                write(1, &buf[..n]);
                total_bytes += n;
            }
            Err(err) => {
                eprintln!("\nfetch: read error: {err}");
                break;
            }
        }
    }

    println!("\n---\n[fetch] done ({} bytes received).", total_bytes);
    0
}
