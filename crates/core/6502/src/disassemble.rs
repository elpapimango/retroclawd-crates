//! 6502 disassembler and single-instruction assembler, for the interactive debugger
//! (`retroclawd --debugger`, the browser debug UI and ZRCP). The counterpart of
//! `z80::disassemble`: no dependency on `Cpu`/`Bus`, pure functions over bytes and text.
//!
//! Every one of the 256 opcode bytes decodes: the 151 documented instructions print as usual,
//! the undocumented ones with a `*` in front of the mnemonic (`*LAX $10`, `*NOP #$00`, `*JAM`) --
//! the NMOS convention of the nestest log and most monitors -- so a viewer can tell the two
//! apart. Addresses use `$hex`; a branch prints its absolute target (`BNE $C012`), which is
//! why `disassemble` needs the address the bytes sit at.
//!
//! `assemble_one` accepts the same syntax (the `*` is optional), plus decimal and `0x` literals.
//! An operand of one or two hex digits (or a value below 256 written without leading zeros)
//! selects the zero-page form when the instruction has one; `$00nn` (three or more digits) forces
//! absolute. Branches take their absolute target and are range-checked.
//!
//! The opcode table is checked against the executor: for every opcode the decoded length equals
//! the number of bytes the core consumes (see the tests).

/// Addressing mode of one opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Imp,
    Acc,
    Imm,
    Zp,
    ZpX,
    ZpY,
    Abs,
    AbsX,
    AbsY,
    Ind,
    IndX,
    IndY,
    Rel,
}

impl Mode {
    /// Instruction length in bytes (opcode included).
    pub const fn byte_len(self) -> usize {
        match self {
            Mode::Imp | Mode::Acc => 1,
            Mode::Imm | Mode::Zp | Mode::ZpX | Mode::ZpY | Mode::IndX | Mode::IndY | Mode::Rel => 2,
            Mode::Abs | Mode::AbsX | Mode::AbsY | Mode::Ind => 3,
        }
    }
}

use Mode::*;

#[derive(Clone, Copy)]
struct Entry {
    mnemonic: &'static str,
    mode: Mode,
    undocumented: bool,
}

const fn e(mnemonic: &'static str, mode: Mode, undocumented: bool) -> Entry {
    Entry {
        mnemonic,
        mode,
        undocumented,
    }
}

/// All 256 opcodes, indexed by opcode byte.
#[rustfmt::skip]
const TABLE: [Entry; 256] = [
    e("BRK", Imp, false), e("ORA", IndX, false), e("JAM", Imp, true), e("SLO", IndX, true),
    e("NOP", Zp, true), e("ORA", Zp, false), e("ASL", Zp, false), e("SLO", Zp, true),
    e("PHP", Imp, false), e("ORA", Imm, false), e("ASL", Acc, false), e("ANC", Imm, true),
    e("NOP", Abs, true), e("ORA", Abs, false), e("ASL", Abs, false), e("SLO", Abs, true),
    e("BPL", Rel, false), e("ORA", IndY, false), e("JAM", Imp, true), e("SLO", IndY, true),
    e("NOP", ZpX, true), e("ORA", ZpX, false), e("ASL", ZpX, false), e("SLO", ZpX, true),
    e("CLC", Imp, false), e("ORA", AbsY, false), e("NOP", Imp, true), e("SLO", AbsY, true),
    e("NOP", AbsX, true), e("ORA", AbsX, false), e("ASL", AbsX, false), e("SLO", AbsX, true),
    e("JSR", Abs, false), e("AND", IndX, false), e("JAM", Imp, true), e("RLA", IndX, true),
    e("BIT", Zp, false), e("AND", Zp, false), e("ROL", Zp, false), e("RLA", Zp, true),
    e("PLP", Imp, false), e("AND", Imm, false), e("ROL", Acc, false), e("ANC", Imm, true),
    e("BIT", Abs, false), e("AND", Abs, false), e("ROL", Abs, false), e("RLA", Abs, true),
    e("BMI", Rel, false), e("AND", IndY, false), e("JAM", Imp, true), e("RLA", IndY, true),
    e("NOP", ZpX, true), e("AND", ZpX, false), e("ROL", ZpX, false), e("RLA", ZpX, true),
    e("SEC", Imp, false), e("AND", AbsY, false), e("NOP", Imp, true), e("RLA", AbsY, true),
    e("NOP", AbsX, true), e("AND", AbsX, false), e("ROL", AbsX, false), e("RLA", AbsX, true),
    e("RTI", Imp, false), e("EOR", IndX, false), e("JAM", Imp, true), e("SRE", IndX, true),
    e("NOP", Zp, true), e("EOR", Zp, false), e("LSR", Zp, false), e("SRE", Zp, true),
    e("PHA", Imp, false), e("EOR", Imm, false), e("LSR", Acc, false), e("ALR", Imm, true),
    e("JMP", Abs, false), e("EOR", Abs, false), e("LSR", Abs, false), e("SRE", Abs, true),
    e("BVC", Rel, false), e("EOR", IndY, false), e("JAM", Imp, true), e("SRE", IndY, true),
    e("NOP", ZpX, true), e("EOR", ZpX, false), e("LSR", ZpX, false), e("SRE", ZpX, true),
    e("CLI", Imp, false), e("EOR", AbsY, false), e("NOP", Imp, true), e("SRE", AbsY, true),
    e("NOP", AbsX, true), e("EOR", AbsX, false), e("LSR", AbsX, false), e("SRE", AbsX, true),
    e("RTS", Imp, false), e("ADC", IndX, false), e("JAM", Imp, true), e("RRA", IndX, true),
    e("NOP", Zp, true), e("ADC", Zp, false), e("ROR", Zp, false), e("RRA", Zp, true),
    e("PLA", Imp, false), e("ADC", Imm, false), e("ROR", Acc, false), e("ARR", Imm, true),
    e("JMP", Ind, false), e("ADC", Abs, false), e("ROR", Abs, false), e("RRA", Abs, true),
    e("BVS", Rel, false), e("ADC", IndY, false), e("JAM", Imp, true), e("RRA", IndY, true),
    e("NOP", ZpX, true), e("ADC", ZpX, false), e("ROR", ZpX, false), e("RRA", ZpX, true),
    e("SEI", Imp, false), e("ADC", AbsY, false), e("NOP", Imp, true), e("RRA", AbsY, true),
    e("NOP", AbsX, true), e("ADC", AbsX, false), e("ROR", AbsX, false), e("RRA", AbsX, true),
    e("NOP", Imm, true), e("STA", IndX, false), e("NOP", Imm, true), e("SAX", IndX, true),
    e("STY", Zp, false), e("STA", Zp, false), e("STX", Zp, false), e("SAX", Zp, true),
    e("DEY", Imp, false), e("NOP", Imm, true), e("TXA", Imp, false), e("XAA", Imm, true),
    e("STY", Abs, false), e("STA", Abs, false), e("STX", Abs, false), e("SAX", Abs, true),
    e("BCC", Rel, false), e("STA", IndY, false), e("JAM", Imp, true), e("SHA", IndY, true),
    e("STY", ZpX, false), e("STA", ZpX, false), e("STX", ZpY, false), e("SAX", ZpY, true),
    e("TYA", Imp, false), e("STA", AbsY, false), e("TXS", Imp, false), e("TAS", AbsY, true),
    e("SHY", AbsX, true), e("STA", AbsX, false), e("SHX", AbsY, true), e("SHA", AbsY, true),
    e("LDY", Imm, false), e("LDA", IndX, false), e("LDX", Imm, false), e("LAX", IndX, true),
    e("LDY", Zp, false), e("LDA", Zp, false), e("LDX", Zp, false), e("LAX", Zp, true),
    e("TAY", Imp, false), e("LDA", Imm, false), e("TAX", Imp, false), e("LXA", Imm, true),
    e("LDY", Abs, false), e("LDA", Abs, false), e("LDX", Abs, false), e("LAX", Abs, true),
    e("BCS", Rel, false), e("LDA", IndY, false), e("JAM", Imp, true), e("LAX", IndY, true),
    e("LDY", ZpX, false), e("LDA", ZpX, false), e("LDX", ZpY, false), e("LAX", ZpY, true),
    e("CLV", Imp, false), e("LDA", AbsY, false), e("TSX", Imp, false), e("LAS", AbsY, true),
    e("LDY", AbsX, false), e("LDA", AbsX, false), e("LDX", AbsY, false), e("LAX", AbsY, true),
    e("CPY", Imm, false), e("CMP", IndX, false), e("NOP", Imm, true), e("DCP", IndX, true),
    e("CPY", Zp, false), e("CMP", Zp, false), e("DEC", Zp, false), e("DCP", Zp, true),
    e("INY", Imp, false), e("CMP", Imm, false), e("DEX", Imp, false), e("SBX", Imm, true),
    e("CPY", Abs, false), e("CMP", Abs, false), e("DEC", Abs, false), e("DCP", Abs, true),
    e("BNE", Rel, false), e("CMP", IndY, false), e("JAM", Imp, true), e("DCP", IndY, true),
    e("NOP", ZpX, true), e("CMP", ZpX, false), e("DEC", ZpX, false), e("DCP", ZpX, true),
    e("CLD", Imp, false), e("CMP", AbsY, false), e("NOP", Imp, true), e("DCP", AbsY, true),
    e("NOP", AbsX, true), e("CMP", AbsX, false), e("DEC", AbsX, false), e("DCP", AbsX, true),
    e("CPX", Imm, false), e("SBC", IndX, false), e("NOP", Imm, true), e("ISC", IndX, true),
    e("CPX", Zp, false), e("SBC", Zp, false), e("INC", Zp, false), e("ISC", Zp, true),
    e("INX", Imp, false), e("SBC", Imm, false), e("NOP", Imp, false), e("SBC", Imm, true),
    e("CPX", Abs, false), e("SBC", Abs, false), e("INC", Abs, false), e("ISC", Abs, true),
    e("BEQ", Rel, false), e("SBC", IndY, false), e("JAM", Imp, true), e("ISC", IndY, true),
    e("NOP", ZpX, true), e("SBC", ZpX, false), e("INC", ZpX, false), e("ISC", ZpX, true),
    e("SED", Imp, false), e("SBC", AbsY, false), e("NOP", Imp, true), e("ISC", AbsY, true),
    e("NOP", AbsX, true), e("SBC", AbsX, false), e("INC", AbsX, false), e("ISC", AbsX, true),
];

/// The mnemonic, addressing mode and "undocumented" flag of an opcode byte.
pub fn opcode_info(opcode: u8) -> (&'static str, Mode, bool) {
    let t = TABLE[opcode as usize];
    (t.mnemonic, t.mode, t.undocumented)
}

/// Length in bytes of the instruction starting with `opcode`.
pub fn instruction_length(opcode: u8) -> usize {
    TABLE[opcode as usize].mode.byte_len()
}

/// Decodes the instruction at the start of `bytes`, which sits at address `addr`, into text and its
/// length in bytes (1-3). Missing bytes (a short slice) read as 0; pass up to 3 bytes.
pub fn disassemble(bytes: &[u8], addr: u16) -> (String, usize) {
    let op = bytes.first().copied().unwrap_or(0);
    let entry = TABLE[op as usize];
    let b1 = bytes.get(1).copied().unwrap_or(0);
    let b2 = bytes.get(2).copied().unwrap_or(0);
    let word = u16::from_le_bytes([b1, b2]);
    let star = if entry.undocumented { "*" } else { "" };
    let m = entry.mnemonic;
    let text = match entry.mode {
        Imp => format!("{star}{m}"),
        Acc => format!("{star}{m} A"),
        Imm => format!("{star}{m} #${b1:02X}"),
        Zp => format!("{star}{m} ${b1:02X}"),
        ZpX => format!("{star}{m} ${b1:02X},X"),
        ZpY => format!("{star}{m} ${b1:02X},Y"),
        Abs => format!("{star}{m} ${word:04X}"),
        AbsX => format!("{star}{m} ${word:04X},X"),
        AbsY => format!("{star}{m} ${word:04X},Y"),
        Ind => format!("{star}{m} (${word:04X})"),
        IndX => format!("{star}{m} (${b1:02X},X)"),
        IndY => format!("{star}{m} (${b1:02X}),Y"),
        Rel => {
            let target = addr.wrapping_add(2).wrapping_add(b1 as i8 as u16);
            format!("{star}{m} ${target:04X}")
        }
    };
    (text, entry.mode.byte_len())
}

/// Parses one instruction (`LDA #$10`, `STA $1234,X`, `JMP ($FFFC)`, `BNE $C012`, `ASL A`, `*LAX $10`)
/// that will be placed at `addr` and returns its bytes.
pub fn assemble_one(text: &str, addr: u16) -> Result<Vec<u8>, String> {
    let text = text.trim();
    let text = text.strip_prefix('*').unwrap_or(text).trim_start();
    let (mnemonic, operand) = match text.split_once(char::is_whitespace) {
        Some((m, rest)) => (m, rest.trim()),
        None => (text, ""),
    };
    let mnemonic = mnemonic.to_ascii_uppercase();
    if mnemonic.is_empty() {
        return Err("empty instruction".to_string());
    }
    if !TABLE.iter().any(|t| t.mnemonic == mnemonic) {
        return Err(format!("unknown mnemonic \"{mnemonic}\""));
    }
    let parsed = parse_operand(operand)?;
    // Candidate modes in order of preference.
    let candidates: Vec<Mode> = match parsed {
        Operand::None => vec![Imp, Acc],
        Operand::Acc => vec![Acc],
        Operand::Imm(_) => vec![Imm],
        Operand::Addr {
            value,
            wide,
            index: Index::None,
        } => {
            let rel = TABLE
                .iter()
                .any(|t| t.mnemonic == mnemonic && t.mode == Rel);
            if rel {
                vec![Rel]
            } else if value <= 0xFF && !wide {
                vec![Zp, Abs]
            } else {
                vec![Abs]
            }
        }
        Operand::Addr {
            value,
            wide,
            index: Index::X,
        } => {
            if value <= 0xFF && !wide {
                vec![ZpX, AbsX]
            } else {
                vec![AbsX]
            }
        }
        Operand::Addr {
            value,
            wide,
            index: Index::Y,
        } => {
            if value <= 0xFF && !wide {
                vec![ZpY, AbsY]
            } else {
                vec![AbsY]
            }
        }
        Operand::Indirect(_) => vec![Ind],
        Operand::IndirectX(_) => vec![IndX],
        Operand::IndirectY(_) => vec![IndY],
    };
    for mode in candidates {
        // A documented opcode wins over an undocumented twin.
        let found = (0..=255u8)
            .filter(|&op| {
                let t = TABLE[op as usize];
                t.mnemonic == mnemonic && t.mode == mode
            })
            .min_by_key(|&op| (TABLE[op as usize].undocumented, op));
        let Some(op) = found else { continue };
        return encode(op, mode, parsed, addr);
    }
    Err(format!(
        "{mnemonic} has no addressing mode for \"{operand}\""
    ))
}

#[derive(Clone, Copy, Debug)]
enum Index {
    None,
    X,
    Y,
}

#[derive(Clone, Copy, Debug)]
enum Operand {
    None,
    Acc,
    Imm(u16),
    Addr {
        value: u16,
        wide: bool,
        index: Index,
    },
    Indirect(u16),
    IndirectX(u16),
    IndirectY(u16),
}

/// `(value, written with three or more hex digits / leading zeros)`.
fn parse_number(s: &str) -> Result<(u16, bool), String> {
    let s = s.trim();
    let (digits, radix, hex_digits) = if let Some(h) = s.strip_prefix('$') {
        (h, 16, h.len())
    } else if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (h, 16, h.len())
    } else {
        (s, 10, 0)
    };
    if digits.is_empty() {
        return Err("missing number".to_string());
    }
    let value = u32::from_str_radix(digits, radix).map_err(|_| format!("bad number \"{s}\""))?;
    if value > 0xFFFF {
        return Err(format!("{s} does not fit in 16 bits"));
    }
    Ok((value as u16, radix == 16 && hex_digits >= 3))
}

fn parse_operand(operand: &str) -> Result<Operand, String> {
    let op = operand.trim();
    if op.is_empty() {
        return Ok(Operand::None);
    }
    if op.eq_ignore_ascii_case("A") {
        return Ok(Operand::Acc);
    }
    if let Some(rest) = op.strip_prefix('#') {
        let (v, _) = parse_number(rest)?;
        return Ok(Operand::Imm(v));
    }
    let upper = op.to_ascii_uppercase();
    if let Some(inner) = upper.strip_prefix('(') {
        let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
        if let Some(n) = compact.strip_suffix(",X)") {
            return Ok(Operand::IndirectX(parse_number(n)?.0));
        }
        if let Some(n) = compact.strip_suffix("),Y") {
            return Ok(Operand::IndirectY(parse_number(n)?.0));
        }
        if let Some(n) = compact.strip_suffix(')') {
            return Ok(Operand::Indirect(parse_number(n)?.0));
        }
        return Err(format!("bad indirect operand \"{operand}\""));
    }
    let compact: String = upper.chars().filter(|c| !c.is_whitespace()).collect();
    let (num, index) = if let Some(n) = compact.strip_suffix(",X") {
        (n.to_string(), Index::X)
    } else if let Some(n) = compact.strip_suffix(",Y") {
        (n.to_string(), Index::Y)
    } else {
        (compact, Index::None)
    };
    let (value, wide) = parse_number(&num)?;
    Ok(Operand::Addr { value, wide, index })
}

fn encode(op: u8, mode: Mode, operand: Operand, addr: u16) -> Result<Vec<u8>, String> {
    let value = match operand {
        Operand::None | Operand::Acc => 0,
        Operand::Imm(v) | Operand::Indirect(v) | Operand::IndirectX(v) | Operand::IndirectY(v) => v,
        Operand::Addr { value, .. } => value,
    };
    match mode {
        Imp | Acc => Ok(vec![op]),
        Imm | Zp | ZpX | ZpY | IndX | IndY => {
            if value > 0xFF {
                return Err(format!("operand ${value:04X} does not fit in one byte"));
            }
            Ok(vec![op, value as u8])
        }
        Abs | AbsX | AbsY | Ind => Ok(vec![op, value as u8, (value >> 8) as u8]),
        Rel => {
            let offset = value as i32 - (addr as i32 + 2);
            if !(-128..=127).contains(&offset) {
                return Err(format!(
                    "branch target ${value:04X} is out of range from ${addr:04X} (offset {offset})"
                ));
            }
            Ok(vec![op, offset as i8 as u8])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cpu, test_support::FlatBus};

    #[test]
    fn table_has_the_151_documented_opcodes_and_every_byte_decodes() {
        let documented = (0..=255u8).filter(|&o| !opcode_info(o).2).count();
        assert_eq!(documented, 151);
        for op in 0..=255u8 {
            let (text, len) = disassemble(&[op, 0x34, 0x12], 0x1000);
            assert!((1..=3).contains(&len), "{op:02X} {text}");
        }
    }

    #[test]
    fn known_instructions_print_in_the_standard_syntax() {
        let cases: &[(&[u8], &str)] = &[
            (&[0xA9, 0x10], "LDA #$10"),
            (&[0xA5, 0x20], "LDA $20"),
            (&[0xB5, 0x20], "LDA $20,X"),
            (&[0xAD, 0x34, 0x12], "LDA $1234"),
            (&[0xBD, 0x34, 0x12], "LDA $1234,X"),
            (&[0xB9, 0x34, 0x12], "LDA $1234,Y"),
            (&[0xA1, 0x20], "LDA ($20,X)"),
            (&[0xB1, 0x20], "LDA ($20),Y"),
            (&[0x6C, 0xFC, 0xFF], "JMP ($FFFC)"),
            (&[0x4C, 0x00, 0xC0], "JMP $C000"),
            (&[0x20, 0xD2, 0xFF], "JSR $FFD2"),
            (&[0x0A], "ASL A"),
            (&[0x60], "RTS"),
            (&[0x00], "BRK"),
            (&[0x96, 0x10], "STX $10,Y"),
            (&[0xBE, 0x00, 0x20], "LDX $2000,Y"),
            (&[0xA7, 0x10], "*LAX $10"),
            (&[0x80, 0x00], "*NOP #$00"),
            (&[0x02], "*JAM"),
            (&[0xEB, 0x01], "*SBC #$01"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(disassemble(bytes, 0x1000).0, *expected, "{bytes:02X?}");
        }
    }

    /// The first instructions of the C64 KERNAL's reset routine at $FCE2 (bytes as in the ROM).
    #[test]
    fn a_real_routine_reads_as_expected() {
        let rom = [0xA2u8, 0xFF, 0x78, 0x9A, 0xD8, 0x20, 0x02, 0xFD, 0xD0, 0x03];
        let mut addr = 0xFCE2u16;
        let mut out = Vec::new();
        let mut i = 0;
        while i < rom.len() {
            let (text, len) = disassemble(&rom[i..], addr);
            out.push(text);
            i += len;
            addr = addr.wrapping_add(len as u16);
        }
        assert_eq!(
            out,
            ["LDX #$FF", "SEI", "TXS", "CLD", "JSR $FD02", "BNE $FCEF"]
        );
    }

    #[test]
    fn branches_print_their_absolute_target() {
        assert_eq!(disassemble(&[0xD0, 0x10], 0xC000).0, "BNE $C012");
        assert_eq!(disassemble(&[0xD0, 0xFE], 0xC000).0, "BNE $C000"); // a loop onto itself
        assert_eq!(disassemble(&[0xF0, 0x80], 0xC000).0, "BEQ $BF82");
        assert_eq!(disassemble(&[0x10, 0x7F], 0xFFF0).0, "BPL $0071"); // wraps at 64K
    }

    #[test]
    fn a_short_slice_reads_missing_bytes_as_zero() {
        assert_eq!(disassemble(&[0xAD], 0).0, "LDA $0000");
        assert_eq!(disassemble(&[], 0).0, "BRK");
    }

    /// Assembling what was disassembled gives the same instruction back, for every opcode and a spread
    /// of operands. Where several opcode bytes print identically (the twelve `*JAM`s, the `*NOP`s,
    /// the two `*ANC`s, `*SBC #`) the assembler picks the documented one, else the lowest, so the
    /// bytes equal the original only for opcodes that are the sole holder of their text.
    #[test]
    fn assembling_a_disassembly_round_trips_every_opcode() {
        for op in 0..=255u8 {
            let (name, mode, _) = opcode_info(op);
            let unique = (0..=255u8)
                .filter(|&o| opcode_info(o).0 == name && opcode_info(o).1 == mode)
                .count()
                == 1;
            for (b1, b2) in [(0x00u8, 0x00u8), (0x7F, 0x12), (0xFE, 0x80), (0x10, 0xC0)] {
                let addr = 0x2000u16;
                let (text, len) = disassemble(&[op, b1, b2], addr);
                let bytes =
                    assemble_one(&text, addr).unwrap_or_else(|e| panic!("{op:02X}: {text}: {e}"));
                assert_eq!(bytes.len(), len, "{op:02X} {text}");
                let again = disassemble(&bytes, addr).0;
                assert_eq!(
                    again.trim_start_matches('*'),
                    text.trim_start_matches('*'),
                    "{op:02X} {text} -> {bytes:02X?}"
                );
                if unique {
                    let mut expected = vec![op, b1, b2];
                    expected.truncate(len);
                    assert_eq!(bytes, expected, "{op:02X} {text}");
                }
            }
        }
    }

    #[test]
    fn documented_opcodes_win_over_undocumented_twins() {
        assert_eq!(assemble_one("NOP", 0).unwrap(), vec![0xEA]);
        assert_eq!(assemble_one("SBC #$01", 0).unwrap(), vec![0xE9, 0x01]);
        assert_eq!(assemble_one("*SBC #$01", 0).unwrap(), vec![0xE9, 0x01]);
        assert_eq!(assemble_one("NOP #$00", 0).unwrap(), vec![0x80, 0x00]);
    }

    fn assembler_syntax_variants() {
        let a = |s: &str| assemble_one(s, 0xC000);
        assert_eq!(a("lda #$10").unwrap(), vec![0xA9, 0x10]);
        assert_eq!(a("LDA #16").unwrap(), vec![0xA9, 0x10]);
        assert_eq!(a("LDA #0x10").unwrap(), vec![0xA9, 0x10]);
        assert_eq!(a("LDA $10").unwrap(), vec![0xA5, 0x10]); // zero page
        assert_eq!(a("LDA $0010").unwrap(), vec![0xAD, 0x10, 0x00]); // forced absolute
        assert_eq!(a("LDA $1234,X").unwrap(), vec![0xBD, 0x34, 0x12]);
        assert_eq!(a("LDA ( $20 ) , Y").unwrap(), vec![0xB1, 0x20]);
        assert_eq!(a("LDX $10,Y").unwrap(), vec![0xB6, 0x10]);
        assert_eq!(a("LDX $1234,Y").unwrap(), vec![0xBE, 0x34, 0x12]);
        assert_eq!(a("STA $1234,Y").unwrap(), vec![0x99, 0x34, 0x12]);
        assert_eq!(a("LDA $10,Y").unwrap(), vec![0xB9, 0x10, 0x00]); // no zp,Y for LDA: absolute
        assert_eq!(a("ASL").unwrap(), vec![0x0A]);
        assert_eq!(a("ASL A").unwrap(), vec![0x0A]);
        assert_eq!(a("JMP ($FFFC)").unwrap(), vec![0x6C, 0xFC, 0xFF]);
        assert_eq!(a("BNE $C012").unwrap(), vec![0xD0, 0x10]);
        assert_eq!(a("BNE $BF82").unwrap(), vec![0xD0, 0x80]);
    }

    fn assembler_rejects_nonsense_clearly() {
        let a = |s: &str| assemble_one(s, 0xC000);
        assert!(a("").unwrap_err().contains("empty"));
        assert!(a("FOO $10").unwrap_err().contains("unknown mnemonic"));
        assert!(a("LDA #$100").unwrap_err().contains("one byte"));
        assert!(a("STA #$10").is_err()); // no immediate store
        assert!(a("BNE $D000").unwrap_err().contains("out of range"));
        assert!(a("LDA $12345").unwrap_err().contains("16 bits"));
        assert!(a("LDA ($10").unwrap_err().contains("indirect"));
        assert!(a("JMP $10,Y").is_err());
    }
    /// Merged (every check kept): `assembler_syntax_variants`, `assembler_rejects_nonsense_clearly`.
    #[test]
    fn assembler_syntax() {
        assembler_syntax_variants();
        assembler_rejects_nonsense_clearly();
    }

    /// The table's lengths are the executor's: run every non-control-flow opcode and compare how far
    /// the program counter moved.
    fn decoded_lengths_match_what_the_core_consumes() {
        let control = |m: &str| matches!(m, "JMP" | "JSR" | "RTS" | "RTI" | "BRK" | "JAM");
        for op in 0..=255u8 {
            let (m, mode, _) = opcode_info(op);
            if control(m) || mode == Rel {
                continue;
            }
            let mut bus = FlatBus::new();
            bus.load(0x0200, &[op, 0x40, 0x03, 0xEA, 0xEA]);
            let mut cpu = Cpu::new();
            cpu.pc = 0x0200;
            cpu.step(&mut bus);
            assert_eq!(
                cpu.pc as usize,
                0x0200 + mode.byte_len(),
                "{op:02X} {m} {mode:?}"
            );
        }
    }

    fn control_flow_lengths() {
        assert_eq!(instruction_length(0x4C), 3); // JMP abs
        assert_eq!(instruction_length(0x6C), 3); // JMP (ind)
        assert_eq!(instruction_length(0x20), 3); // JSR
        assert_eq!(instruction_length(0x60), 1); // RTS
        assert_eq!(instruction_length(0x40), 1); // RTI
        assert_eq!(instruction_length(0x00), 1); // BRK (listed as one byte; the core skips a padding byte)
        for op in [0x10u8, 0x30, 0x50, 0x70, 0x90, 0xB0, 0xD0, 0xF0] {
            assert_eq!(instruction_length(op), 2);
        }
    }
    /// Merged (every check kept): `decoded_lengths_match_what_the_core_consumes`, `control_flow_lengths`.
    #[test]
    fn decoded_lengths() {
        decoded_lengths_match_what_the_core_consumes();
        control_flow_lengths();
    }
}
