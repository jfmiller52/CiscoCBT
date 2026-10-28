#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRequest {
    pub filename: String,
    pub blksize: Option<u16>,
}

fn next_c_string(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = bytes.iter().position(|&b| b == 0)?;
    Some((&bytes[..end], &bytes[end + 1..]))
}

fn parse_blksize_value(value: &[u8]) -> Option<u16> {
    let s = std::str::from_utf8(value).ok()?;
    let n: u64 = s.parse().ok()?;
    Some(if n > u16::MAX as u64 {
        u16::MAX
    } else {
        n as u16
    })
}

pub fn parse_rrq(packet: &[u8]) -> Option<ReadRequest> {
    if packet.len() < 2 {
        return None;
    }
    let opcode = u16::from_be_bytes([packet[0], packet[1]]);
    if opcode != 1 {
        return None;
    }
    let (filename_bytes, rest) = next_c_string(&packet[2..])?;
    if filename_bytes.is_empty() {
        return None;
    }
    let filename = String::from_utf8_lossy(filename_bytes).into_owned();
    let (mode_bytes, mut rest) = next_c_string(rest)?;
    let mode = std::str::from_utf8(mode_bytes).ok()?;
    if !mode.eq_ignore_ascii_case("octet") {
        return None;
    }
    let mut blksize = None;
    while !rest.is_empty() {
        let (name_bytes, after_name) = next_c_string(rest)?;
        let (value_bytes, after_value) = next_c_string(after_name)?;
        rest = after_value;
        let name = std::str::from_utf8(name_bytes).ok()?;
        if name.eq_ignore_ascii_case("blksize") {
            if let Some(n) = parse_blksize_value(value_bytes) {
                blksize = Some(n);
            }
        }
    }
    Some(ReadRequest { filename, blksize })
}

pub fn serves(request: &ReadRequest, chosen: &str) -> bool {
    request.filename == chosen
}

pub fn negotiated_blksize(requested: Option<u16>) -> u16 {
    match requested {
        None => 512,
        Some(n) => n.clamp(8, 8192),
    }
}

pub fn oack_packet(blksize: u16) -> Vec<u8> {
    let mut packet = vec![0, 6];
    packet.extend_from_slice(b"blksize\0");
    packet.extend_from_slice(blksize.to_string().as_bytes());
    packet.push(0);
    packet
}

pub fn data_packet(block: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0, 3, (block >> 8) as u8, (block & 0xff) as u8];
    packet.extend_from_slice(payload);
    packet
}

pub fn error_packet(code: u16, message: &str) -> Vec<u8> {
    let mut packet = vec![0, 5, (code >> 8) as u8, (code & 0xff) as u8];
    packet.extend_from_slice(message.as_bytes());
    packet.push(0);
    packet
}

pub fn parse_ack(packet: &[u8]) -> Option<u16> {
    if packet.len() < 4 {
        return None;
    }
    let opcode = u16::from_be_bytes([packet[0], packet[1]]);
    if opcode != 4 {
        return None;
    }
    Some(u16::from_be_bytes([packet[2], packet[3]]))
}

pub fn block_count(size: u64, blksize: u16) -> u64 {
    let bs = if blksize == 0 { 512 } else { blksize as u64 };
    size / bs + 1
}

pub fn wire_block(index: u64) -> u16 {
    (index % 65536) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrq_parses_blksize_and_serves_only_the_chosen_image() {
        let request =
            parse_rrq(b"\x00\x01cat9k_iosxe.17.12.05.SPA.bin\x00octet\x00blksize\x008192\x00")
                .unwrap();
        assert_eq!(request.blksize, Some(8192));
        assert!(serves(&request, "cat9k_iosxe.17.12.05.SPA.bin"));
        assert!(!serves(&request, "other.bin"));
        assert!(parse_rrq(b"\x00\x01x.bin\x00netascii\x00").is_none());
    }

    #[test]
    fn blocks_wrap_and_always_end_short() {
        assert_eq!(block_count(1024, 512), 3);
        assert_eq!(block_count(1000, 512), 2);
        assert_eq!(wire_block(65535), 65535);
        assert_eq!(wire_block(65536), 0);
        assert_eq!(wire_block(65537), 1);
        assert_eq!(data_packet(1, b"ab"), vec![0, 3, 0, 1, b'a', b'b']);
        assert_eq!(parse_ack(&[0, 4, 0, 7]), Some(7));
        assert_eq!(negotiated_blksize(Some(65000)), 8192);
    }
}
