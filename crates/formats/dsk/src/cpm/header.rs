//! The 128-byte headers AMSDOS (Amstrad CPC) and +3DOS (ZX Spectrum +3) put in front of a file's data.
//! CP/M itself stores only whole 128-byte records; the header carries the exact length, the type
//! (BASIC / binary) and the load and entry addresses.

pub const HEADER_SIZE: usize = 128;

/// An AMSDOS header.
///
/// Layout: `00` user, `01..0B` name, `10` block number, `11` last-block flag, `12` type (0 BASIC, 1
/// protected BASIC, 2 binary), `13..14` length, `15..16` load address, `17` first-block flag, `18..19`
/// logical length, `1A..1B` entry address, `40..42` real file length (24 bits), `43..44` the sum of
/// bytes `00..42`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Amsdos {
    pub user: u8,
    /// The 8 + 3 name characters.
    pub name: [u8; 11],
    pub kind: u8,
    pub load: u16,
    pub entry: u16,
    /// Length of the data after the header.
    pub length: u32,
}

pub const AMSDOS_BASIC: u8 = 0;
pub const AMSDOS_BINARY: u8 = 2;

fn amsdos_sum(header: &[u8]) -> u16 {
    header[..0x43]
        .iter()
        .fold(0u16, |a, &b| a.wrapping_add(b as u16))
}

/// The AMSDOS header at the start of `data`, if there is a valid one (the checksum matches).
pub fn parse_amsdos(data: &[u8]) -> Option<Amsdos> {
    if data.len() < HEADER_SIZE {
        return None;
    }
    let stored = u16::from_le_bytes([data[0x43], data[0x44]]);
    // An all-zero start would "match" a zero checksum; a real header has a name and a type.
    if stored != amsdos_sum(data) || stored == 0 {
        return None;
    }
    let mut name = [0u8; 11];
    name.copy_from_slice(&data[1..12]);
    Some(Amsdos {
        user: data[0],
        name,
        kind: data[0x12],
        load: u16::from_le_bytes([data[0x15], data[0x16]]),
        entry: u16::from_le_bytes([data[0x1A], data[0x1B]]),
        length: u32::from_le_bytes([data[0x40], data[0x41], data[0x42], 0]),
    })
}

pub fn amsdos(h: &Amsdos) -> [u8; HEADER_SIZE] {
    let mut out = [0u8; HEADER_SIZE];
    out[0] = h.user;
    out[1..12].copy_from_slice(&h.name);
    out[0x12] = h.kind;
    out[0x13..0x15].copy_from_slice(&(h.length as u16).to_le_bytes());
    out[0x15..0x17].copy_from_slice(&h.load.to_le_bytes());
    out[0x17] = 0xFF;
    out[0x18..0x1A].copy_from_slice(&(h.length as u16).to_le_bytes());
    out[0x1A..0x1C].copy_from_slice(&h.entry.to_le_bytes());
    out[0x40..0x43].copy_from_slice(&h.length.to_le_bytes()[..3]);
    let sum = amsdos_sum(&out);
    out[0x43..0x45].copy_from_slice(&sum.to_le_bytes());
    out
}

/// A +3DOS header.
///
/// Layout: `00..07` `PLUS3DOS`, `08` `$1A`, `09` issue, `0A` version, `0B..0E` file length including the
/// header (32 bits), `0F` type (0 program, 1 number array, 2 character array, 3 code), `10..11` length,
/// `12..13` and `14..15` the two BASIC parameters (autostart line and program length, or load address),
/// `7F` the sum of bytes `00..7E` modulo 256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plus3Dos {
    pub kind: u8,
    pub length: u16,
    pub param1: u16,
    pub param2: u16,
    /// The size of the whole file, header included.
    pub total: u32,
}

pub const PLUS3_PROGRAM: u8 = 0;
pub const PLUS3_CODE: u8 = 3;

fn plus3_sum(header: &[u8]) -> u8 {
    header[..127].iter().fold(0u8, |a, &b| a.wrapping_add(b))
}

pub fn parse_plus3dos(data: &[u8]) -> Option<Plus3Dos> {
    if data.len() < HEADER_SIZE || &data[..8] != b"PLUS3DOS" || data[8] != 0x1A {
        return None;
    }
    if data[127] != plus3_sum(data) {
        return None;
    }
    Some(Plus3Dos {
        total: u32::from_le_bytes([data[11], data[12], data[13], data[14]]),
        kind: data[15],
        length: u16::from_le_bytes([data[16], data[17]]),
        param1: u16::from_le_bytes([data[18], data[19]]),
        param2: u16::from_le_bytes([data[20], data[21]]),
    })
}

pub fn plus3dos(h: &Plus3Dos) -> [u8; HEADER_SIZE] {
    let mut out = [0u8; HEADER_SIZE];
    out[..8].copy_from_slice(b"PLUS3DOS");
    out[8] = 0x1A;
    out[9] = 1;
    out[10] = 0;
    out[11..15].copy_from_slice(&h.total.to_le_bytes());
    out[15] = h.kind;
    out[16..18].copy_from_slice(&h.length.to_le_bytes());
    out[18..20].copy_from_slice(&h.param1.to_le_bytes());
    out[20..22].copy_from_slice(&h.param2.to_le_bytes());
    out[127] = plus3_sum(&out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amsdos_headers_round_trip_and_check_their_sum() {
        let h = Amsdos {
            user: 0,
            name: *b"GAME    BIN",
            kind: AMSDOS_BINARY,
            load: 0x4000,
            entry: 0x4010,
            length: 0x1234,
        };
        let mut file = amsdos(&h).to_vec();
        file.extend([1, 2, 3]);
        assert_eq!(parse_amsdos(&file), Some(h.clone()));
        // The checksum is the 16-bit sum of the first 67 bytes.
        let sum: u32 = file[..67].iter().map(|&b| b as u32).sum();
        assert_eq!(
            u16::from_le_bytes([file[67], file[68]]) as u32,
            sum & 0xFFFF
        );
        file[0x15] ^= 1; // damage the load address
        assert_eq!(parse_amsdos(&file), None);
        assert_eq!(parse_amsdos(&[0u8; 200]), None);
        assert_eq!(parse_amsdos(&[1, 2, 3]), None);
    }

    #[test]
    fn plus3dos_headers_round_trip_and_check_their_sum() {
        let h = Plus3Dos {
            kind: PLUS3_CODE,
            length: 6912,
            param1: 0x4000,
            param2: 0x8000,
            total: 128 + 6912,
        };
        let mut file = plus3dos(&h).to_vec();
        file.extend(vec![0u8; 6912]);
        assert_eq!(parse_plus3dos(&file), Some(h));
        assert_eq!(&file[..9], b"PLUS3DOS\x1A");
        file[20] ^= 1;
        assert_eq!(parse_plus3dos(&file), None);
        assert_eq!(parse_plus3dos(&[0u8; 200]), None);
    }
}
