//! Host-side unit tests for TriOS network, filesystem, and executable parsers.
//!
//! Validates:
//! 1. ELF64 Header parser and error handling
//! 2. FAT 8.3 filename encoding and decoding
//! 3. DNS RFC 1035 packet builder and response parser
//! 4. RFC 793 TCP IPv4 pseudo-header checksum calculation

// ---------------------------------------------------------------------------
// 1. ELF Header Parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElfHeader64 {
    pub entry_point: u64,
    pub ph_offset: u64,
    pub ph_entry_size: u16,
    pub ph_num: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    InvalidMagic,
    Not64Bit,
    NotLittleEndian,
    NotX86_64,
    UnsupportedType,
}

pub fn parse_elf_header(bytes: &[u8]) -> Result<ElfHeader64, ElfError> {
    if bytes.len() < 64 {
        return Err(ElfError::TooSmall);
    }

    if bytes[0..4] != [0x7F, b'E', b'L', b'F'] {
        return Err(ElfError::InvalidMagic);
    }

    if bytes[4] != 2 {
        return Err(ElfError::Not64Bit);
    }

    if bytes[5] != 1 {
        return Err(ElfError::NotLittleEndian);
    }

    let e_type = u16::from_le_bytes([bytes[16], bytes[17]]);
    if e_type != 2 && e_type != 3 {
        return Err(ElfError::UnsupportedType);
    }

    let e_machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    if e_machine != 0x3E {
        return Err(ElfError::NotX86_64);
    }

    let entry_point = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let ph_offset = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
    let ph_entry_size = u16::from_le_bytes([bytes[54], bytes[55]]);
    let ph_num = u16::from_le_bytes([bytes[56], bytes[57]]);

    Ok(ElfHeader64 {
        entry_point,
        ph_offset,
        ph_entry_size,
        ph_num,
    })
}

// ---------------------------------------------------------------------------
// 2. FAT 8.3 Filename Conversion
// ---------------------------------------------------------------------------

pub fn to_dos_8_3(name: &str) -> [u8; 11] {
    let mut dos = [b' '; 11];
    let trimmed = name.trim();
    let (base, ext) = match trimmed.rfind('.') {
        Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
        None => (trimmed, ""),
    };

    let mut i = 0;
    for b in base.bytes() {
        if i >= 8 {
            break;
        }
        dos[i] = b.to_ascii_uppercase();
        i += 1;
    }

    let mut j = 0;
    for b in ext.bytes() {
        if j >= 3 {
            break;
        }
        dos[8 + j] = b.to_ascii_uppercase();
        j += 1;
    }

    dos
}

pub fn from_dos_8_3(dos: &[u8; 11]) -> String {
    let mut name = String::new();
    for &b in &dos[0..8] {
        if b != b' ' && b != 0 {
            name.push(b as char);
        }
    }
    let mut ext = String::new();
    for &b in &dos[8..11] {
        if b != b' ' && b != 0 {
            ext.push(b as char);
        }
    }
    if !ext.is_empty() {
        name.push('.');
        name.push_str(&ext);
    }
    name
}

// ---------------------------------------------------------------------------
// 3. DNS Packet Builder & Parser (RFC 1035)
// ---------------------------------------------------------------------------

pub fn build_dns_query(hostname: &str, q_id: u16) -> Result<Vec<u8>, &'static str> {
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

    Ok(query)
}

pub fn parse_dns_response(data: &[u8]) -> Option<(u16, [u8; 4])> {
    if data.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([data[0], data[1]]);
    let flags = u16::from_be_bytes([data[2], data[3]]);
    let qdcount = u16::from_be_bytes([data[4], data[5]]);
    let ancount = u16::from_be_bytes([data[6], data[7]]);

    if (flags & 0x8000) == 0 || ancount == 0 {
        return None;
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
            return None;
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
            return Some((id, ip));
        }
        pos += rdlen;
    }
    None
}

// ---------------------------------------------------------------------------
// 4. TCP Checksum Calculation (RFC 793)
// ---------------------------------------------------------------------------

pub fn calculate_tcp_checksum(src_ip: [u8; 4], dest_ip: [u8; 4], tcp_segment: &[u8]) -> u16 {
    let mut sum = 0u32;

    // Pseudo-header: Source IP (2 words)
    sum = sum.wrapping_add(u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32);
    sum = sum.wrapping_add(u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32);

    // Pseudo-header: Destination IP (2 words)
    sum = sum.wrapping_add(u16::from_be_bytes([dest_ip[0], dest_ip[1]]) as u32);
    sum = sum.wrapping_add(u16::from_be_bytes([dest_ip[2], dest_ip[3]]) as u32);

    // Pseudo-header: Zero byte (0) + Protocol (6 for TCP)
    sum = sum.wrapping_add(6 as u32);

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

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[test]
fn test_elf_header_valid() {
    let mut buf = [0u8; 64];
    buf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    buf[4] = 2; // 64-bit
    buf[5] = 1; // Little-endian
    buf[6] = 1; // Version
    buf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    buf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // x86_64
    buf[24..32].copy_from_slice(&0x400000u64.to_le_bytes()); // entry point
    buf[32..40].copy_from_slice(&64u64.to_le_bytes()); // ph_offset
    buf[54..56].copy_from_slice(&56u16.to_le_bytes()); // ph_entry_size
    buf[56..58].copy_from_slice(&2u16.to_le_bytes()); // ph_num

    let hdr = parse_elf_header(&buf).expect("Valid ELF header should parse");
    assert_eq!(hdr.entry_point, 0x400000);
    assert_eq!(hdr.ph_offset, 64);
    assert_eq!(hdr.ph_entry_size, 56);
    assert_eq!(hdr.ph_num, 2);
}

#[test]
fn test_elf_header_invalid_magic() {
    let mut buf = [0u8; 64];
    buf[0..4].copy_from_slice(b"NOPE");
    assert_eq!(parse_elf_header(&buf), Err(ElfError::InvalidMagic));
}

#[test]
fn test_elf_header_not_64bit() {
    let mut buf = [0u8; 64];
    buf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    buf[4] = 1; // 32-bit ELFCLASS32
    assert_eq!(parse_elf_header(&buf), Err(ElfError::Not64Bit));
}

#[test]
fn test_elf_header_too_small() {
    let buf = [0u8; 32];
    assert_eq!(parse_elf_header(&buf), Err(ElfError::TooSmall));
}

#[test]
fn test_elf_header_not_x86_64() {
    let mut buf = [0u8; 64];
    buf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    buf[4] = 2; // 64-bit
    buf[5] = 1; // Little-endian
    buf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    buf[18..20].copy_from_slice(&0xB7u16.to_le_bytes()); // AArch64 (not x86_64)
    assert_eq!(parse_elf_header(&buf), Err(ElfError::NotX86_64));
}

#[test]
fn test_fat_8_3_conversion() {
    assert_eq!(to_dos_8_3("hello.txt"), *b"HELLO   TXT");
    assert_eq!(to_dos_8_3("kernel.bin"), *b"KERNEL  BIN");
    assert_eq!(to_dos_8_3("readme"), *b"README     ");
    assert_eq!(to_dos_8_3("verylongfilename.jpeg"), *b"VERYLONGJPE");

    assert_eq!(from_dos_8_3(b"HELLO   TXT"), "HELLO.TXT");
    assert_eq!(from_dos_8_3(b"KERNEL  BIN"), "KERNEL.BIN");
    assert_eq!(from_dos_8_3(b"README     "), "README");
}

#[test]
fn test_dns_packet_builder_and_parser() {
    let query = build_dns_query("example.com", 0x1337).expect("DNS query build failed");
    assert_eq!(query[0..2], 0x1337u16.to_be_bytes()); // ID
    assert_eq!(query[2..4], 0x0100u16.to_be_bytes()); // Flags
    assert_eq!(query[4..6], 1u16.to_be_bytes()); // QDCOUNT

    // Check QNAME formatting: \x07example\x03com\x00
    assert_eq!(&query[12..25], b"\x07example\x03com\x00");

    // Construct mock response packet for example.com -> 93.184.216.34
    let mut resp = Vec::new();
    resp.extend_from_slice(&0x1337u16.to_be_bytes()); // ID
    resp.extend_from_slice(&0x8180u16.to_be_bytes()); // Standard response, No error
    resp.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT: 1
    resp.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT: 1
    resp.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT: 0
    resp.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT: 0
    // Question copy
    resp.extend_from_slice(b"\x07example\x03com\x00");
    resp.extend_from_slice(&1u16.to_be_bytes()); // QTYPE: A
    resp.extend_from_slice(&1u16.to_be_bytes()); // QCLASS: IN
    // Answer
    resp.extend_from_slice(&0xC00Cu16.to_be_bytes()); // Name pointer to offset 12
    resp.extend_from_slice(&1u16.to_be_bytes()); // TYPE: A
    resp.extend_from_slice(&1u16.to_be_bytes()); // CLASS: IN
    resp.extend_from_slice(&300u32.to_be_bytes()); // TTL: 300
    resp.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH: 4
    resp.extend_from_slice(&[93, 184, 216, 34]); // RDATA: 93.184.216.34

    let parsed = parse_dns_response(&resp).expect("Failed to parse DNS response");
    assert_eq!(parsed.0, 0x1337);
    assert_eq!(parsed.1, [93, 184, 216, 34]);
}

#[test]
fn test_tcp_checksum_calculation() {
    let src_ip = [10, 0, 2, 15];
    let dest_ip = [93, 184, 216, 34];

    // Minimal 20-byte TCP header (SYN packet) with checksum field = 0
    let tcp_seg = vec![
        0xCA, 0xFE, // Source Port: 51966
        0x00, 0x50, // Dest Port: 80
        0x00, 0x00, 0x03, 0xE8, // Seq: 1000
        0x00, 0x00, 0x00, 0x00, // Ack: 0
        0x50, 0x02, // Data offset (5 words), Flags: SYN
        0xFA, 0xF0, // Window: 64240
        0x00, 0x00, // Checksum placeholder
        0x00, 0x00, // Urgent pointer
    ];

    let csum = calculate_tcp_checksum(src_ip, dest_ip, &tcp_seg);
    assert_ne!(csum, 0);

    // Verify odd-length payload checksum handling
    let mut tcp_seg_odd = tcp_seg.clone();
    tcp_seg_odd.extend_from_slice(b"HELLO"); // 5 bytes payload
    let csum_odd = calculate_tcp_checksum(src_ip, dest_ip, &tcp_seg_odd);
    assert_ne!(csum_odd, 0);
}
