//! Z80 disassembler + single-instruction assembler, for the interactive
//! debugger (`retroclawd --debugger`) -- see `apps/retroclawd/src/
//! debugger.rs`. No dependency on `Machine`/`Bus`: pure functions over raw
//! bytes/text, unit-tested directly like this crate's other pure logic.
//!
//! `disassemble`'s field-decode order deliberately mirrors `opcodes/mod.rs`'s
//! own `(opcode>>3)&7`/`opcode&7` groupings and `reg_get`/`reg_set`'s
//! IXH/IXL/IYH/IYL substitution rules exactly, so a printed mnemonic always
//! names what this crate's own executor would actually read/write for that
//! byte pattern -- enforced by this module's own round-trip tests, not by
//! sharing code with the execution handlers (which execute, not format
//! text -- there is no function to literally share).
//!
//! `assemble_one` is deliberately scoped to a single instruction: no labels,
//! no macros, no multi-line programs -- an address/immediate operand is a
//! decimal or `0x`-prefixed hex literal only, matching `main.rs`'s own
//! hex-parsing convention. It covers every documented mnemonic plus the
//! common undocumented register forms (IXH/IXL/IYH/IYL, SLL), but does NOT
//! assemble the DD/FD CB indexed-bit-op "undocumented copy into a register"
//! forms (e.g. "RLC (IX+5),B") -- `disassemble` still displays these
//! correctly if they're already in memory, but writing new ones back via the
//! single-instruction editor isn't supported, a deliberate scope cut (this
//! form is rare to hand-edit and would meaningfully expand the parser for a
//! niche case).

use crate::opcodes::IndexReg;

/// Decodes the Z80 instruction at the start of `bytes` into a human-readable
/// mnemonic and its length in bytes (1-4 -- the longest real encoding is a
/// DD/FD CB four-byte form). `bytes` should have at least 1 byte; a caller
/// reading from live memory (e.g. `Machine::read_memory`) should pass up to
/// 4 consecutive bytes starting at the instruction's own address --
/// over-reading past the true instruction length is harmless, this never
/// reads more of `bytes` than the decoded instruction actually needs.
///
/// Every unrecognized byte pattern returns a safe `"DB 0x.."`-style
/// placeholder rather than panicking -- unlike this crate's executor (whose
/// `unimplemented!()` is a deliberate loud failure, since reaching it during
/// real execution is a genuine bug), disassembly runs continuously against
/// whatever bytes happen to sit at PC plus/minus a viewer's scroll window,
/// including misaligned or garbage memory, and must degrade gracefully.
pub fn disassemble(bytes: &[u8]) -> (String, usize) {
    let op0 = *bytes.first().unwrap_or(&0);
    match op0 {
        0xCB => {
            let (text, len) = decode_cb(&bytes[1.min(bytes.len())..]);
            (text, len + 1)
        }
        0xED => {
            let (text, len) = decode_ed(&bytes[1.min(bytes.len())..]);
            (text, len + 1)
        }
        0xDD | 0xFD => {
            let ix = if op0 == 0xDD {
                IndexReg::Ix
            } else {
                IndexReg::Iy
            };
            let rest = &bytes[1.min(bytes.len())..];
            if rest.first() == Some(&0xCB) {
                // DD/FD CB d op -- 4 bytes total, prefix + CB + displacement +
                // sub-opcode. Displacement is fetched BEFORE the sub-opcode on
                // real hardware (see `Cpu::step`'s own DD/FD CB branch).
                let d = *rest.get(1).unwrap_or(&0) as i8;
                let sub = *rest.get(2).unwrap_or(&0);
                (decode_indexed_cb(ix, d, sub), 4)
            } else {
                let (text, len) = decode_base(rest, Some(ix));
                (text, len + 1)
            }
        }
        _ => decode_base(bytes, None),
    }
}

fn get(bytes: &[u8], pos: usize) -> u8 {
    *bytes.get(pos).unwrap_or(&0)
}

fn imm16_at(bytes: &[u8], pos: usize) -> u16 {
    u16::from_le_bytes([get(bytes, pos), get(bytes, pos + 1)])
}

fn hl_or_index_name(active_index: Option<IndexReg>) -> &'static str {
    match active_index {
        None => "HL",
        Some(IndexReg::Ix) => "IX",
        Some(IndexReg::Iy) => "IY",
    }
}

/// B,C,D,E,H,L,A only (never `(HL)`/IXH-style substitution) -- for register
/// fields that this crate's own executor always resolves to a *real*
/// register regardless of any active DD/FD prefix (ED instructions' `r`
/// fields, `LD r,r'`'s "other operand" when the other side is indirect, the
/// DD/FD-CB copy-target field's B/C/D/E/A slots).
fn real_reg_name(idx: u8) -> &'static str {
    match idx {
        0 => "B",
        1 => "C",
        2 => "D",
        3 => "E",
        4 => "H",
        5 => "L",
        7 => "A",
        _ => "?",
    }
}

fn indexed_operand(ix: IndexReg, d: i8) -> String {
    let reg = match ix {
        IndexReg::Ix => "IX",
        IndexReg::Iy => "IY",
    };
    if d >= 0 {
        format!("({reg}+0x{d:02X})")
    } else {
        format!("({reg}-0x{:02X})", -(d as i16))
    }
}

/// One 3-bit register-field operand (`reg_get`/`reg_set`'s own idx 0-7
/// meaning: B,C,D,E,H,L,(HL),A), substituting IXH/IXL/IYH/IYL for H/L and
/// `(IX+d)`/`(IY+d)` for `(HL)` when `active_index` is set -- exactly
/// `reg_get`/`reg_set`'s own substitution rule. `pos` is where a
/// displacement byte (if idx==6 under an active index) would be found in
/// `bytes`; returns the operand text plus how many extra bytes (0 or 1) it
/// consumed from `bytes` at `pos`.
fn reg_operand(
    idx: u8,
    active_index: Option<IndexReg>,
    bytes: &[u8],
    pos: usize,
) -> (String, usize) {
    match idx {
        0..=3 | 7 => (real_reg_name(idx).to_string(), 0),
        4 => match active_index {
            None => ("H".to_string(), 0),
            Some(IndexReg::Ix) => ("IXH".to_string(), 0),
            Some(IndexReg::Iy) => ("IYH".to_string(), 0),
        },
        5 => match active_index {
            None => ("L".to_string(), 0),
            Some(IndexReg::Ix) => ("IXL".to_string(), 0),
            Some(IndexReg::Iy) => ("IYL".to_string(), 0),
        },
        6 => match active_index {
            None => ("(HL)".to_string(), 0),
            Some(ix) => {
                let d = get(bytes, pos) as i8;
                (indexed_operand(ix, d), 1)
            }
        },
        _ => ("?".to_string(), 0),
    }
}

/// Decodes one base-table (unprefixed, or DD/FD-prefixed-and-re-dispatched)
/// instruction starting at `bytes[0]` (the opcode byte itself -- any DD/FD
/// prefix byte has already been consumed by the caller). Returns mnemonic
/// text and length INCLUDING the opcode byte but NOT the prefix byte.
fn decode_base(bytes: &[u8], active_index: Option<IndexReg>) -> (String, usize) {
    let op = get(bytes, 0);
    let hli = hl_or_index_name(active_index);
    match op {
        0x00 => ("NOP".to_string(), 1),
        0x76 => ("HALT".to_string(), 1),

        0x40..=0x7F => ld_r_r_text(op, active_index, bytes),
        0x80..=0xBF => alu_text(op, active_index, bytes),

        0x01 => (format!("LD BC,0x{:04X}", imm16_at(bytes, 1)), 3),
        0x11 => (format!("LD DE,0x{:04X}", imm16_at(bytes, 1)), 3),
        0x21 => (format!("LD {hli},0x{:04X}", imm16_at(bytes, 1)), 3),
        0x31 => (format!("LD SP,0x{:04X}", imm16_at(bytes, 1)), 3),

        0x02 => ("LD (BC),A".to_string(), 1),
        0x12 => ("LD (DE),A".to_string(), 1),
        0x32 => (format!("LD (0x{:04X}),A", imm16_at(bytes, 1)), 3),

        0x0A => ("LD A,(BC)".to_string(), 1),
        0x1A => ("LD A,(DE)".to_string(), 1),
        0x3A => (format!("LD A,(0x{:04X})", imm16_at(bytes, 1)), 3),

        0x22 => (format!("LD (0x{:04X}),{hli}", imm16_at(bytes, 1)), 3),
        0x2A => (format!("LD {hli},(0x{:04X})", imm16_at(bytes, 1)), 3),

        0x06 => ld_r_n_text(0, active_index, bytes),
        0x0E => ld_r_n_text(1, active_index, bytes),
        0x16 => ld_r_n_text(2, active_index, bytes),
        0x1E => ld_r_n_text(3, active_index, bytes),
        0x26 => ld_r_n_text(4, active_index, bytes),
        0x2E => ld_r_n_text(5, active_index, bytes),
        0x36 => ld_hl_ind_n_text(active_index, bytes),
        0x3E => ld_r_n_text(7, active_index, bytes),

        0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => {
            incdec_text("INC", op, active_index, bytes)
        }
        0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => {
            incdec_text("DEC", op, active_index, bytes)
        }

        0x03 | 0x13 | 0x23 | 0x33 => ss_text("INC", op, active_index),
        0x0B | 0x1B | 0x2B | 0x3B => ss_text("DEC", op, active_index),
        0x09 | 0x19 | 0x29 | 0x39 => add_hl_ss_text(op, active_index),

        0x10 => (format!("DJNZ {}", signed_hex(get(bytes, 1) as i8)), 2),
        0x18 => (format!("JR {}", signed_hex(get(bytes, 1) as i8)), 2),
        0x20 => (format!("JR NZ,{}", signed_hex(get(bytes, 1) as i8)), 2),
        0x28 => (format!("JR Z,{}", signed_hex(get(bytes, 1) as i8)), 2),
        0x30 => (format!("JR NC,{}", signed_hex(get(bytes, 1) as i8)), 2),
        0x38 => (format!("JR C,{}", signed_hex(get(bytes, 1) as i8)), 2),

        0xC3 => (format!("JP 0x{:04X}", imm16_at(bytes, 1)), 3),
        0xC2 | 0xCA | 0xD2 | 0xDA | 0xE2 | 0xEA | 0xF2 | 0xFA => {
            let cc = cc_name((op >> 3) & 0x07);
            (format!("JP {cc},0x{:04X}", imm16_at(bytes, 1)), 3)
        }
        0xE9 => (format!("JP ({hli})"), 1),

        0xCD => (format!("CALL 0x{:04X}", imm16_at(bytes, 1)), 3),
        0xC4 | 0xCC | 0xD4 | 0xDC | 0xE4 | 0xEC | 0xF4 | 0xFC => {
            let cc = cc_name((op >> 3) & 0x07);
            (format!("CALL {cc},0x{:04X}", imm16_at(bytes, 1)), 3)
        }

        0xC9 => ("RET".to_string(), 1),
        0xC0 | 0xC8 | 0xD0 | 0xD8 | 0xE0 | 0xE8 | 0xF0 | 0xF8 => {
            (format!("RET {}", cc_name((op >> 3) & 0x07)), 1)
        }

        0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
            (format!("RST 0x{:02X}", op & 0x38), 1)
        }

        0x08 => ("EX AF,AF'".to_string(), 1),
        0xD9 => ("EXX".to_string(), 1),
        0xEB => ("EX DE,HL".to_string(), 1),
        0xE3 => (format!("EX (SP),{hli}"), 1),

        0xC5 => ("PUSH BC".to_string(), 1),
        0xD5 => ("PUSH DE".to_string(), 1),
        0xE5 => (format!("PUSH {hli}"), 1),
        0xF5 => ("PUSH AF".to_string(), 1),

        0xC1 => ("POP BC".to_string(), 1),
        0xD1 => ("POP DE".to_string(), 1),
        0xE1 => (format!("POP {hli}"), 1),
        0xF1 => ("POP AF".to_string(), 1),

        0x07 => ("RLCA".to_string(), 1),
        0x0F => ("RRCA".to_string(), 1),
        0x17 => ("RLA".to_string(), 1),
        0x1F => ("RRA".to_string(), 1),
        0x27 => ("DAA".to_string(), 1),
        0x2F => ("CPL".to_string(), 1),
        0x37 => ("SCF".to_string(), 1),
        0x3F => ("CCF".to_string(), 1),

        0xF3 => ("DI".to_string(), 1),
        0xFB => ("EI".to_string(), 1),

        0xF9 => (format!("LD SP,{hli}"), 1),

        0xC6 => (format!("ADD A,0x{:02X}", get(bytes, 1)), 2),
        0xCE => (format!("ADC A,0x{:02X}", get(bytes, 1)), 2),
        0xD6 => (format!("SUB 0x{:02X}", get(bytes, 1)), 2),
        0xDE => (format!("SBC A,0x{:02X}", get(bytes, 1)), 2),
        0xE6 => (format!("AND 0x{:02X}", get(bytes, 1)), 2),
        0xEE => (format!("XOR 0x{:02X}", get(bytes, 1)), 2),
        0xF6 => (format!("OR 0x{:02X}", get(bytes, 1)), 2),
        0xFE => (format!("CP 0x{:02X}", get(bytes, 1)), 2),

        0xD3 => (format!("OUT (0x{:02X}),A", get(bytes, 1)), 2),
        0xDB => (format!("IN A,(0x{:02X})", get(bytes, 1)), 2),

        _ => (format!("DB 0x{op:02X}"), 1),
    }
}

/// Like [`disassemble`], for an instruction located at `addr`: `JR`/`DJNZ` print their absolute target
/// (`JR NZ,0x1B63`) instead of the raw displacement, the way a monitor shows them and the way the
/// 6502 disassembler shows branches. Every other instruction is identical to `disassemble`.
pub fn disassemble_at(bytes: &[u8], addr: u16) -> (String, usize) {
    let (text, len) = disassemble(bytes);
    let op = bytes.first().copied().unwrap_or(0);
    if matches!(op, 0x10 | 0x18 | 0x20 | 0x28 | 0x30 | 0x38) {
        let target = addr
            .wrapping_add(2)
            .wrapping_add(get(bytes, 1) as i8 as u16);
        if let Some(cut) = text.rfind([',', ' ']) {
            return (format!("{}0x{target:04X}", &text[..=cut]), len);
        }
    }
    (text, len)
}

/// Like [`assemble_one`], for an instruction that will be placed at `addr`: the operand of
/// `JR`/`DJNZ` is the absolute target (what [`disassemble_at`] prints), range-checked against the
/// reach of a signed byte. Everything else is `assemble_one`.
pub fn assemble_one_at(text: &str, addr: u16) -> Result<Vec<u8>, String> {
    let trimmed = text.trim();
    let upper = trimmed.to_ascii_uppercase();
    let relative = upper
        .split_whitespace()
        .next()
        .is_some_and(|m| m == "JR" || m == "DJNZ");
    if !relative {
        return assemble_one(text);
    }
    let Some(cut) = trimmed.rfind([',', ' ']) else {
        return assemble_one(text);
    };
    let target = parse_number(trimmed[cut + 1..].trim())?;
    let offset = target as i32 - (addr as i32 + 2);
    if !(-128..=127).contains(&offset) {
        return Err(format!(
            "assemble: relative jump target 0x{target:04X} is out of range from 0x{addr:04X} (offset {offset})"
        ));
    }
    assemble_one(&format!("{}0x{:02X}", &trimmed[..=cut], offset as i8 as u8))
}

fn signed_hex(d: i8) -> String {
    if d >= 0 {
        format!("0x{d:02X}")
    } else {
        format!("-0x{:02X}", -(d as i16))
    }
}

fn cc_name(cc: u8) -> &'static str {
    match cc {
        0 => "NZ",
        1 => "Z",
        2 => "NC",
        3 => "C",
        4 => "PO",
        5 => "PE",
        6 => "P",
        7 => "M",
        _ => "?",
    }
}

/// LD r,r' (0x40-0x7F, excluding 0x76 HALT already matched earlier).
/// Substitution to IXH/IXL/IYH/IYL only happens when NEITHER operand is the
/// `(HL)`-role memory operand -- mirrors `real_reg_get`/`real_reg_set`'s own
/// doc comment in `opcodes/mod.rs` exactly.
fn ld_r_r_text(op: u8, active_index: Option<IndexReg>, bytes: &[u8]) -> (String, usize) {
    let dest = (op >> 3) & 0x07;
    let src = op & 0x07;
    let involves_indirect = dest == 6 || src == 6;

    let mut len = 1usize;
    let (src_text, src_extra) = if involves_indirect && src != 6 {
        (real_reg_name(src).to_string(), 0)
    } else {
        reg_operand(src, active_index, bytes, 1)
    };
    len += src_extra;
    let (dest_text, dest_extra) = if involves_indirect && dest != 6 {
        (real_reg_name(dest).to_string(), 0)
    } else {
        reg_operand(dest, active_index, bytes, 1)
    };
    len += dest_extra;

    (format!("LD {dest_text},{src_text}"), len)
}

fn alu_text(op: u8, active_index: Option<IndexReg>, bytes: &[u8]) -> (String, usize) {
    let alu_op = (op >> 3) & 0x07;
    let src = op & 0x07;
    let (src_text, extra) = reg_operand(src, active_index, bytes, 1);
    let mnemonic = match alu_op {
        0 => "ADD A,",
        1 => "ADC A,",
        2 => "SUB ",
        3 => "SBC A,",
        4 => "AND ",
        5 => "XOR ",
        6 => "OR ",
        7 => "CP ",
        _ => "?",
    };
    (format!("{mnemonic}{src_text}"), 1 + extra)
}

fn ld_r_n_text(idx: u8, active_index: Option<IndexReg>, bytes: &[u8]) -> (String, usize) {
    // idx is always in 0..=5|7 here (never 6 -- 0x36 is handled separately),
    // so `reg_operand` never touches `bytes` for this call.
    let (name, _) = reg_operand(idx, active_index, bytes, usize::MAX);
    (format!("LD {name},0x{:02X}", get(bytes, 1)), 2)
}

fn ld_hl_ind_n_text(active_index: Option<IndexReg>, bytes: &[u8]) -> (String, usize) {
    match active_index {
        None => (format!("LD (HL),0x{:02X}", get(bytes, 1)), 2),
        Some(ix) => {
            // Displacement precedes the immediate under a DD/FD prefix (see
            // `base_loads::ld_hl_ind_n`'s own doc comment for why the order
            // can't be swapped).
            let d = get(bytes, 1) as i8;
            let n = get(bytes, 2);
            (format!("LD {},0x{n:02X}", indexed_operand(ix, d)), 3)
        }
    }
}

fn incdec_text(
    mnemonic: &str,
    op: u8,
    active_index: Option<IndexReg>,
    bytes: &[u8],
) -> (String, usize) {
    let idx = (op >> 3) & 0x07;
    let (text, extra) = reg_operand(idx, active_index, bytes, 1);
    (format!("{mnemonic} {text}"), 1 + extra)
}

fn ss_text(mnemonic: &str, op: u8, active_index: Option<IndexReg>) -> (String, usize) {
    let ss = (op >> 4) & 0x03;
    let name = match ss {
        0 => "BC",
        1 => "DE",
        2 => hl_or_index_name(active_index),
        3 => "SP",
        _ => "?",
    };
    (format!("{mnemonic} {name}"), 1)
}

fn add_hl_ss_text(op: u8, active_index: Option<IndexReg>) -> (String, usize) {
    let ss = (op >> 4) & 0x03;
    let dest = hl_or_index_name(active_index);
    let operand = match ss {
        0 => "BC",
        1 => "DE",
        2 => dest,
        3 => "SP",
        _ => "?",
    };
    (format!("ADD {dest},{operand}"), 1)
}

/// One CB-prefixed instruction (rotate/shift/BIT/RES/SET, register or `(HL)`
/// operand -- plain CB is never affected by a DD/FD prefix, that's the
/// separate 4-byte DD/FD-CB form `decode_indexed_cb` below handles). `bytes`
/// starts at the CB sub-opcode byte itself.
fn decode_cb(bytes: &[u8]) -> (String, usize) {
    let op = get(bytes, 0);
    let group = (op >> 6) & 0x03;
    let field = (op >> 3) & 0x07;
    let idx = op & 0x07;
    let reg = if idx == 6 {
        "(HL)".to_string()
    } else {
        real_reg_name(idx).to_string()
    };
    match group {
        0 => {
            let name = rotate_shift_name(field);
            (format!("{name} {reg}"), 1)
        }
        1 => (format!("BIT {field},{reg}"), 1),
        2 => (format!("RES {field},{reg}"), 1),
        3 => (format!("SET {field},{reg}"), 1),
        _ => (format!("DB 0xCB,0x{op:02X}"), 1),
    }
}

fn rotate_shift_name(field: u8) -> &'static str {
    match field {
        0 => "RLC",
        1 => "RRC",
        2 => "RL",
        3 => "RR",
        4 => "SLA",
        5 => "SRA",
        6 => "SLL",
        7 => "SRL",
        _ => "?",
    }
}

/// The undocumented DD/FD-CB "also copies the result into a register" copy
/// target's name: the real B/C/D/E/H/L/A, so a low3 field of 4 or 5 is `H`/`L`
/// under either prefix, exactly as the executor (`opcodes/indexed_cb.rs`,
/// `real_reg_set`) writes them. (Until 2026-10-14 both printed and wrote
/// IXH/IXL/IYH/IYL here, which differs from the Undocumented Z80 Documented
/// 4.1 and from FUSE.)
fn indexed_cb_copy_target_name(low3: u8) -> &'static str {
    match low3 {
        0 => "B",
        1 => "C",
        2 => "D",
        3 => "E",
        4 => "H",
        5 => "L",
        7 => "A",
        _ => "?",
    }
}

/// The DD/FD CB four-byte form (prefix already consumed, `d`/`sub` already
/// extracted by the caller) -- always exactly 4 bytes total (caller adds the
/// prefix+CB+displacement+sub-opcode length itself), so this returns text
/// only.
fn decode_indexed_cb(ix: IndexReg, d: i8, sub: u8) -> String {
    let group = (sub >> 6) & 0x03;
    let field = (sub >> 3) & 0x07;
    let low3 = sub & 0x07;
    let operand = indexed_operand(ix, d);

    match group {
        0 => {
            let name = rotate_shift_name(field);
            if low3 == 6 {
                format!("{name} {operand}")
            } else {
                format!("{name} {operand},{}", indexed_cb_copy_target_name(low3))
            }
        }
        // BIT never writes back -- no copy-target quirk applies.
        1 => format!("BIT {field},{operand}"),
        2 => {
            if low3 == 6 {
                format!("RES {field},{operand}")
            } else {
                format!(
                    "RES {field},{operand},{}",
                    indexed_cb_copy_target_name(low3)
                )
            }
        }
        3 => {
            if low3 == 6 {
                format!("SET {field},{operand}")
            } else {
                format!(
                    "SET {field},{operand},{}",
                    indexed_cb_copy_target_name(low3)
                )
            }
        }
        _ => format!("DB 0xCB,0x{sub:02X}"),
    }
}

/// One ED-prefixed instruction. `bytes` starts at the ED sub-opcode byte
/// itself. Every ED opcode this crate doesn't implement (see `opcodes/ed.rs`/
/// `ed_block.rs`'s own install lists) falls through to the safe placeholder
/// -- real hardware treats an undefined ED opcode as an 8-cycle NOP, but
/// this crate's own executor panics on it (a genuine bug if actually
/// reached), so this module doesn't claim a specific behavior for it either.
fn decode_ed(bytes: &[u8]) -> (String, usize) {
    let op = get(bytes, 0);
    match op {
        0x47 => ("LD I,A".to_string(), 1),
        0x4F => ("LD R,A".to_string(), 1),
        0x57 => ("LD A,I".to_string(), 1),
        0x5F => ("LD A,R".to_string(), 1),
        0x44 => ("NEG".to_string(), 1),
        0x45 => ("RETN".to_string(), 1),
        0x4D => ("RETI".to_string(), 1),
        0x46 => ("IM 0".to_string(), 1),
        0x56 => ("IM 1".to_string(), 1),
        0x5E => ("IM 2".to_string(), 1),
        0x67 => ("RRD".to_string(), 1),
        0x6F => ("RLD".to_string(), 1),

        0x4A | 0x5A | 0x6A | 0x7A => (format!("ADC HL,{}", pp_name((op >> 4) & 0x03)), 1),
        0x42 | 0x52 | 0x62 | 0x72 => (format!("SBC HL,{}", pp_name((op >> 4) & 0x03)), 1),
        0x43 | 0x53 | 0x63 | 0x73 => (
            format!(
                "LD (0x{:04X}),{}",
                imm16_at(bytes, 1),
                pp_name((op >> 4) & 0x03)
            ),
            3,
        ),
        0x4B | 0x5B | 0x6B | 0x7B => (
            format!(
                "LD {},(0x{:04X})",
                pp_name((op >> 4) & 0x03),
                imm16_at(bytes, 1)
            ),
            3,
        ),

        0x40 | 0x48 | 0x50 | 0x58 | 0x60 | 0x68 | 0x78 => {
            (format!("IN {},(C)", real_reg_name((op >> 3) & 0x07)), 1)
        }
        0x70 => ("IN (C)".to_string(), 1),
        0x41 | 0x49 | 0x51 | 0x59 | 0x61 | 0x69 | 0x79 => {
            (format!("OUT (C),{}", real_reg_name((op >> 3) & 0x07)), 1)
        }
        0x71 => ("OUT (C),0".to_string(), 1),

        0xA0 => ("LDI".to_string(), 1),
        0xA8 => ("LDD".to_string(), 1),
        0xB0 => ("LDIR".to_string(), 1),
        0xB8 => ("LDDR".to_string(), 1),
        0xA1 => ("CPI".to_string(), 1),
        0xA9 => ("CPD".to_string(), 1),
        0xB1 => ("CPIR".to_string(), 1),
        0xB9 => ("CPDR".to_string(), 1),
        0xA2 => ("INI".to_string(), 1),
        0xAA => ("IND".to_string(), 1),
        0xB2 => ("INIR".to_string(), 1),
        0xBA => ("INDR".to_string(), 1),
        0xA3 => ("OUTI".to_string(), 1),
        0xAB => ("OUTD".to_string(), 1),
        0xB3 => ("OTIR".to_string(), 1),
        0xBB => ("OTDR".to_string(), 1),

        _ => (format!("DB 0xED,0x{op:02X}"), 1),
    }
}

fn pp_name(pp: u8) -> &'static str {
    match pp {
        0 => "BC",
        1 => "DE",
        2 => "HL",
        3 => "SP",
        _ => "?",
    }
}

// ---------------------------------------------------------------------
// Single-instruction assembler
// ---------------------------------------------------------------------

/// Parses a single Z80 assembly instruction (no labels, no macros, no
/// multi-line programs -- see this module's own doc comment) into its
/// machine-code bytes. Numeric operands accept decimal (`42`) or
/// `0x`-prefixed hex (`0x8000`); an operand that isn't a register name and
/// doesn't parse as a number is a hard error naming the reason (unsupported
/// mnemonic, or -- explicitly -- that labels aren't supported), never a
/// silent best-effort guess.
pub fn assemble_one(text: &str) -> Result<Vec<u8>, String> {
    let (mnemonic, operands) = split_mnemonic(text);
    let ops: Vec<&str> = operands.iter().map(String::as_str).collect();

    match mnemonic.as_str() {
        "NOP" => Ok(vec![0x00]),
        "HALT" => Ok(vec![0x76]),
        "DI" => Ok(vec![0xF3]),
        "EI" => Ok(vec![0xFB]),
        "RLCA" => Ok(vec![0x07]),
        "RRCA" => Ok(vec![0x0F]),
        "RLA" => Ok(vec![0x17]),
        "RRA" => Ok(vec![0x1F]),
        "DAA" => Ok(vec![0x27]),
        "CPL" => Ok(vec![0x2F]),
        "SCF" => Ok(vec![0x37]),
        "CCF" => Ok(vec![0x3F]),
        "RET" if ops.is_empty() => Ok(vec![0xC9]),
        "RET" => cc_index(ops[0]).map(|cc| vec![0xC0 | (cc << 3)]),
        "EXX" => Ok(vec![0xD9]),
        "NEG" => Ok(vec![0xED, 0x44]),
        "RETN" => Ok(vec![0xED, 0x45]),
        "RETI" => Ok(vec![0xED, 0x4D]),
        "RRD" => Ok(vec![0xED, 0x67]),
        "RLD" => Ok(vec![0xED, 0x6F]),
        "LDI" => Ok(vec![0xED, 0xA0]),
        "LDD" => Ok(vec![0xED, 0xA8]),
        "LDIR" => Ok(vec![0xED, 0xB0]),
        "LDDR" => Ok(vec![0xED, 0xB8]),
        "CPI" => Ok(vec![0xED, 0xA1]),
        "CPD" => Ok(vec![0xED, 0xA9]),
        "CPIR" => Ok(vec![0xED, 0xB1]),
        "CPDR" => Ok(vec![0xED, 0xB9]),
        "INI" => Ok(vec![0xED, 0xA2]),
        "IND" => Ok(vec![0xED, 0xAA]),
        "INIR" => Ok(vec![0xED, 0xB2]),
        "INDR" => Ok(vec![0xED, 0xBA]),
        "OUTI" => Ok(vec![0xED, 0xA3]),
        "OUTD" => Ok(vec![0xED, 0xAB]),
        "OTIR" => Ok(vec![0xED, 0xB3]),
        "OTDR" => Ok(vec![0xED, 0xBB]),

        "IM" if ops.len() == 1 => match ops[0] {
            "0" => Ok(vec![0xED, 0x46]),
            "1" => Ok(vec![0xED, 0x56]),
            "2" => Ok(vec![0xED, 0x5E]),
            other => Err(format!(
                "assemble: invalid IM operand \"{other}\" (expected 0, 1, or 2)"
            )),
        },

        "EX" if ops.len() == 2 => assemble_ex(&ops),
        "PUSH" if ops.len() == 1 => assemble_push_pop(0xC5, ops[0]),
        "POP" if ops.len() == 1 => assemble_push_pop(0xC1, ops[0]),

        "DJNZ" if ops.len() == 1 => assemble_relative(0x10, ops[0]),
        "JR" if ops.len() == 1 => assemble_relative(0x18, ops[0]),
        "JR" if ops.len() == 2 => {
            let cc = cc_index_2(ops[0])?;
            assemble_relative(0x20 | (cc << 3), ops[1])
        }

        "JP" if ops.len() == 1 => assemble_jp(ops[0]),
        "JP" if ops.len() == 2 => {
            let cc = cc_index(ops[0])?;
            let addr = parse_number(ops[1])?;
            Ok(bytes3(0xC2 | (cc << 3), addr))
        }

        "CALL" if ops.len() == 1 => {
            let addr = parse_number(ops[0])?;
            Ok(bytes3(0xCD, addr))
        }
        "CALL" if ops.len() == 2 => {
            let cc = cc_index(ops[0])?;
            let addr = parse_number(ops[1])?;
            Ok(bytes3(0xC4 | (cc << 3), addr))
        }

        "RST" if ops.len() == 1 => {
            let n = parse_number(ops[0])?;
            if n > 0x38 || n % 8 != 0 {
                return Err(format!(
                    "assemble: RST target must be one of 0x00,0x08,..,0x38, got 0x{n:02X}"
                ));
            }
            Ok(vec![0xC7 | (n as u8)])
        }

        "IN" if ops.len() == 2 => assemble_in(&ops),
        "OUT" if ops.len() == 2 => assemble_out(&ops),

        "LD" if ops.len() == 2 => assemble_ld(&ops),

        "INC" if ops.len() == 1 => assemble_incdec(0x04, 0x03, ops[0]),
        "DEC" if ops.len() == 1 => assemble_incdec(0x05, 0x0B, ops[0]),

        "ADD" if ops.len() == 2 => assemble_add(&ops),
        "ADC" if ops.len() == 2 => assemble_alu_or_adc16(0x88, 0x4A, &ops),
        "SUB" if ops.len() == 1 => assemble_alu8(0x90, ops[0]),
        "SBC" if ops.len() == 2 => assemble_alu_or_adc16(0x98, 0x42, &ops),
        "AND" if ops.len() == 1 => assemble_alu8(0xA0, ops[0]),
        "XOR" if ops.len() == 1 => assemble_alu8(0xA8, ops[0]),
        "OR" if ops.len() == 1 => assemble_alu8(0xB0, ops[0]),
        "CP" if ops.len() == 1 => assemble_alu8(0xB8, ops[0]),

        "RLC" if ops.len() == 1 => assemble_cb(0x00, ops[0]),
        "RRC" if ops.len() == 1 => assemble_cb(0x08, ops[0]),
        "RL" if ops.len() == 1 => assemble_cb(0x10, ops[0]),
        "RR" if ops.len() == 1 => assemble_cb(0x18, ops[0]),
        "SLA" if ops.len() == 1 => assemble_cb(0x20, ops[0]),
        "SRA" if ops.len() == 1 => assemble_cb(0x28, ops[0]),
        "SLL" if ops.len() == 1 => assemble_cb(0x30, ops[0]),
        "SRL" if ops.len() == 1 => assemble_cb(0x38, ops[0]),
        "BIT" if ops.len() == 2 => assemble_cb_bit(0x40, &ops),
        "RES" if ops.len() == 2 => assemble_cb_bit(0x80, &ops),
        "SET" if ops.len() == 2 => assemble_cb_bit(0xC0, &ops),

        "" => Err("assemble: empty instruction".to_string()),
        other => Err(format!(
            "assemble: unsupported or unknown mnemonic \"{other}\""
        )),
    }
}

fn split_mnemonic(text: &str) -> (String, Vec<String>) {
    let text = text.trim();
    let (mnemonic, rest) = match text.find(char::is_whitespace) {
        Some(i) => (&text[..i], text[i..].trim()),
        None => (text, ""),
    };
    let operands = if rest.is_empty() {
        Vec::new()
    } else {
        rest.split(',').map(|s| s.trim().to_string()).collect()
    };
    (mnemonic.to_ascii_uppercase(), operands)
}

fn parse_number(s: &str) -> Result<u16, String> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16)
            .map_err(|e| format!("assemble: invalid hex number \"{s}\": {e}"))
    } else if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        Err(format!(
            "assemble: \"{s}\" looks like a label, but this single-instruction assembler doesn't support labels -- use a literal address (decimal or 0x-hex)"
        ))
    } else {
        s.parse::<u16>()
            .map_err(|e| format!("assemble: invalid number \"{s}\": {e}"))
    }
}

fn bytes3(op: u8, addr: u16) -> Vec<u8> {
    let [lo, hi] = addr.to_le_bytes();
    vec![op, lo, hi]
}

fn reg8_index(name: &str) -> Option<u8> {
    match name.to_ascii_uppercase().as_str() {
        "B" => Some(0),
        "C" => Some(1),
        "D" => Some(2),
        "E" => Some(3),
        "H" => Some(4),
        "L" => Some(5),
        "(HL)" => Some(6),
        "A" => Some(7),
        _ => None,
    }
}

fn cc_index(name: &str) -> Result<u8, String> {
    match name.to_ascii_uppercase().as_str() {
        "NZ" => Ok(0),
        "Z" => Ok(1),
        "NC" => Ok(2),
        "C" => Ok(3),
        "PO" => Ok(4),
        "PE" => Ok(5),
        "P" => Ok(6),
        "M" => Ok(7),
        other => Err(format!("assemble: unknown condition \"{other}\"")),
    }
}

/// JR/DJNZ only support the 4 conditions with a relative encoding
/// (NZ/Z/NC/C) -- JP/CALL's full 8-condition set doesn't apply here, same as
/// real Z80 syntax.
fn cc_index_2(name: &str) -> Result<u8, String> {
    match cc_index(name)? {
        cc @ 0..=3 => Ok(cc),
        _ => Err(format!(
            "assemble: JR only supports NZ/Z/NC/C conditions, not \"{name}\""
        )),
    }
}

/// An operand that's an 8-bit register name (with IX/IY substitution),
/// `(HL)`/`(IX+d)`/`(IY+d)`, or a numeric immediate -- used by ALU/INC/DEC/CB
/// operand parsing. Returns (opcode's register-field value or None for an
/// immediate, prefix byte if IX/IY-indexed, displacement byte if indexed,
/// immediate value if a number).
enum Operand {
    Reg(u8, Option<IndexReg>, Option<i8>),
    Imm(u8),
}

fn parse_reg_or_imm(s: &str) -> Result<Operand, String> {
    let su = s.trim().to_ascii_uppercase();
    match su.as_str() {
        "IXH" => Ok(Operand::Reg(4, Some(IndexReg::Ix), None)),
        "IXL" => Ok(Operand::Reg(5, Some(IndexReg::Ix), None)),
        "IYH" => Ok(Operand::Reg(4, Some(IndexReg::Iy), None)),
        "IYL" => Ok(Operand::Reg(5, Some(IndexReg::Iy), None)),
        _ if su.starts_with("(IX") || su.starts_with("(IY") => {
            let ix = if su.starts_with("(IX") {
                IndexReg::Ix
            } else {
                IndexReg::Iy
            };
            let d = parse_displacement(&su)?;
            Ok(Operand::Reg(6, Some(ix), Some(d)))
        }
        _ => {
            if let Some(idx) = reg8_index(&su) {
                Ok(Operand::Reg(idx, None, None))
            } else {
                parse_number(s).map(|n| Operand::Imm(n as u8))
            }
        }
    }
}

fn parse_displacement(s: &str) -> Result<i8, String> {
    let inner = s
        .trim_start_matches('(')
        .trim_end_matches(')')
        .get(2..)
        .unwrap_or("");
    if inner.is_empty() {
        return Ok(0);
    }
    let (sign, rest) = if let Some(r) = inner.strip_prefix('+') {
        (1i32, r)
    } else if let Some(r) = inner.strip_prefix('-') {
        (-1i32, r)
    } else {
        return Err(format!("assemble: invalid displacement in \"{s}\""));
    };
    let val = parse_number(rest)? as i32;
    let d = sign * val;
    if !(-128..=127).contains(&d) {
        return Err(format!("assemble: displacement out of range in \"{s}\""));
    }
    Ok(d as i8)
}

/// Emits an operand parsed by `parse_reg_or_imm` as an immediate/register
/// field, prepending a DD/FD prefix byte and appending a displacement byte
/// as needed. `field_shift` places the 3-bit register index at the right
/// bit position in `base_op` (0 for the low 3 bits, 3 for bits 3-5).
fn emit_reg_field(base_op: u8, field_shift: u8, operand: Operand) -> Result<Vec<u8>, String> {
    match operand {
        Operand::Reg(idx, prefix, d) => {
            let op = base_op | (idx << field_shift);
            let mut out = Vec::new();
            if let Some(ix) = prefix {
                out.push(prefix_byte(ix));
            }
            out.push(op);
            if let Some(d) = d {
                out.push(d as u8);
            }
            Ok(out)
        }
        Operand::Imm(_) => Err(
            "assemble: expected a register or (HL)/(IX+d)/(IY+d) operand, got a number".to_string(),
        ),
    }
}

fn prefix_byte(ix: IndexReg) -> u8 {
    match ix {
        IndexReg::Ix => 0xDD,
        IndexReg::Iy => 0xFD,
    }
}

fn assemble_alu8(base_op: u8, operand: &str) -> Result<Vec<u8>, String> {
    match parse_reg_or_imm(operand)? {
        Operand::Reg(idx, prefix, d) => {
            let mut out = Vec::new();
            if let Some(ix) = prefix {
                out.push(prefix_byte(ix));
            }
            out.push(base_op | idx);
            if let Some(d) = d {
                out.push(d as u8);
            }
            Ok(out)
        }
        Operand::Imm(n) => Ok(vec![base_op | 0x46, n]), // ALU n opcode = base|0x46, per the 0xC6.. row shape
    }
}

/// ADD/ADC/SBC with two operands: either `A,<8-bit operand>` (ALU-shaped) or
/// `HL/IX/IY,<16-bit register>` (ADC/SBC HL,ss or ADD HL/IX/IY,ss).
fn assemble_alu_or_adc16(alu_base: u8, ed_base: u8, ops: &[&str]) -> Result<Vec<u8>, String> {
    let dest = ops[0].trim().to_ascii_uppercase();
    if dest == "A" {
        match parse_reg_or_imm(ops[1])? {
            Operand::Reg(idx, prefix, d) => {
                let mut out = Vec::new();
                if let Some(ix) = prefix {
                    out.push(prefix_byte(ix));
                }
                out.push(alu_base | idx);
                if let Some(d) = d {
                    out.push(d as u8);
                }
                Ok(out)
            }
            Operand::Imm(n) => Ok(vec![alu_base | 0x46, n]),
        }
    } else if dest == "HL" {
        let pp = pp_index(ops[1])?;
        Ok(vec![0xED, ed_base | (pp << 4)])
    } else {
        Err(format!(
            "assemble: unsupported ADC/SBC destination \"{}\" (expected A or HL)",
            ops[0]
        ))
    }
}

fn pp_index(name: &str) -> Result<u8, String> {
    match name.trim().to_ascii_uppercase().as_str() {
        "BC" => Ok(0),
        "DE" => Ok(1),
        "HL" => Ok(2),
        "SP" => Ok(3),
        other => Err(format!("assemble: unknown 16-bit register \"{other}\"")),
    }
}

fn assemble_add(ops: &[&str]) -> Result<Vec<u8>, String> {
    let dest = ops[0].trim().to_ascii_uppercase();
    match dest.as_str() {
        "A" => match parse_reg_or_imm(ops[1])? {
            Operand::Reg(idx, prefix, d) => {
                let mut out = Vec::new();
                if let Some(ix) = prefix {
                    out.push(prefix_byte(ix));
                }
                out.push(0x80 | idx);
                if let Some(d) = d {
                    out.push(d as u8);
                }
                Ok(out)
            }
            Operand::Imm(n) => Ok(vec![0xC6, n]),
        },
        "HL" => {
            let ss = pp_index(ops[1])?;
            Ok(vec![0x09 | (ss << 4)])
        }
        "IX" | "IY" => {
            let ix = if dest == "IX" {
                IndexReg::Ix
            } else {
                IndexReg::Iy
            };
            let operand = ops[1].trim().to_ascii_uppercase();
            let ss = match operand.as_str() {
                "BC" => 0,
                "DE" => 1,
                "IX" if dest == "IX" => 2,
                "IY" if dest == "IY" => 2,
                "SP" => 3,
                other => {
                    return Err(format!(
                        "assemble: ADD {dest},{other} is not a real instruction"
                    ));
                }
            };
            Ok(vec![prefix_byte(ix), 0x09 | (ss << 4)])
        }
        other => Err(format!("assemble: unsupported ADD destination \"{other}\"")),
    }
}

fn assemble_incdec(base8: u8, base16: u8, operand: &str) -> Result<Vec<u8>, String> {
    let su = operand.trim().to_ascii_uppercase();
    match su.as_str() {
        "BC" => Ok(vec![base16]),
        "DE" => Ok(vec![base16 | 0x10]),
        "HL" => Ok(vec![base16 | 0x20]),
        "SP" => Ok(vec![base16 | 0x30]),
        "IX" => Ok(vec![0xDD, base16 | 0x20]),
        "IY" => Ok(vec![0xFD, base16 | 0x20]),
        _ => emit_reg_field(base8, 3, parse_reg_or_imm(operand)?),
    }
}

fn assemble_cb(base: u8, operand: &str) -> Result<Vec<u8>, String> {
    match parse_reg_or_imm(operand)? {
        Operand::Reg(idx, None, None) => Ok(vec![0xCB, base | idx]),
        Operand::Reg(_, Some(_), _) => Err(
            "assemble: indexed (IX+d)/(IY+d) rotate/shift is not supported by this single-instruction assembler"
                .to_string(),
        ),
        _ => Err(format!("assemble: invalid operand \"{operand}\" for rotate/shift")),
    }
}

fn assemble_cb_bit(base: u8, ops: &[&str]) -> Result<Vec<u8>, String> {
    let bit = parse_number(ops[0])?;
    if bit > 7 {
        return Err(format!("assemble: bit number must be 0-7, got {bit}"));
    }
    match parse_reg_or_imm(ops[1])? {
        Operand::Reg(idx, None, None) => Ok(vec![0xCB, base | ((bit as u8) << 3) | idx]),
        Operand::Reg(_, Some(_), _) => Err(
            "assemble: indexed (IX+d)/(IY+d) BIT/RES/SET is not supported by this single-instruction assembler"
                .to_string(),
        ),
        _ => Err(format!("assemble: invalid operand \"{}\" for BIT/RES/SET", ops[1])),
    }
}

fn assemble_relative(base: u8, operand: &str) -> Result<Vec<u8>, String> {
    let n = parse_number(operand)?;
    Ok(vec![base, n as u8])
}

fn assemble_jp(operand: &str) -> Result<Vec<u8>, String> {
    let su = operand.trim().to_ascii_uppercase();
    match su.as_str() {
        "(HL)" => Ok(vec![0xE9]),
        "(IX)" => Ok(vec![0xDD, 0xE9]),
        "(IY)" => Ok(vec![0xFD, 0xE9]),
        _ => {
            let addr = parse_number(operand)?;
            Ok(bytes3(0xC3, addr))
        }
    }
}

fn assemble_push_pop(base: u8, operand: &str) -> Result<Vec<u8>, String> {
    let su = operand.trim().to_ascii_uppercase();
    match su.as_str() {
        "BC" => Ok(vec![base]),
        "DE" => Ok(vec![base | 0x10]),
        "HL" => Ok(vec![base | 0x20]),
        "AF" => Ok(vec![base | 0x30]),
        "IX" => Ok(vec![0xDD, base | 0x20]),
        "IY" => Ok(vec![0xFD, base | 0x20]),
        other => Err(format!(
            "assemble: unknown register pair \"{other}\" for PUSH/POP"
        )),
    }
}

fn assemble_ex(ops: &[&str]) -> Result<Vec<u8>, String> {
    let a = ops[0].trim().to_ascii_uppercase();
    let b = ops[1].trim().to_ascii_uppercase();
    match (a.as_str(), b.as_str()) {
        ("AF", "AF'") => Ok(vec![0x08]),
        ("DE", "HL") => Ok(vec![0xEB]),
        ("(SP)", "HL") => Ok(vec![0xE3]),
        ("(SP)", "IX") => Ok(vec![0xDD, 0xE3]),
        ("(SP)", "IY") => Ok(vec![0xFD, 0xE3]),
        _ => Err(format!(
            "assemble: unsupported EX operands \"{},{}\"",
            ops[0], ops[1]
        )),
    }
}

fn assemble_in(ops: &[&str]) -> Result<Vec<u8>, String> {
    let dest = ops[0].trim().to_ascii_uppercase();
    let src = ops[1].trim().to_ascii_uppercase();
    if src == "(C)" {
        if dest == "A" {
            return Ok(vec![0xED, 0x78]);
        }
        let idx = reg8_index(&dest)
            .filter(|&i| i != 6)
            .ok_or_else(|| format!("assemble: invalid IN r,(C) register \"{}\"", ops[0]))?;
        return Ok(vec![0xED, 0x40 | (idx << 3)]);
    }
    if dest == "A" && src.starts_with('(') && src.ends_with(')') {
        let n = parse_number(&src[1..src.len() - 1])?;
        return Ok(vec![0xDB, n as u8]);
    }
    Err(format!(
        "assemble: unsupported IN operands \"{},{}\"",
        ops[0], ops[1]
    ))
}

fn assemble_out(ops: &[&str]) -> Result<Vec<u8>, String> {
    let dest = ops[0].trim().to_ascii_uppercase();
    let src = ops[1].trim().to_ascii_uppercase();
    if dest == "(C)" {
        if src == "0" {
            return Ok(vec![0xED, 0x71]);
        }
        let idx = reg8_index(&src)
            .filter(|&i| i != 6)
            .ok_or_else(|| format!("assemble: invalid OUT (C),r register \"{}\"", ops[1]))?;
        return Ok(vec![0xED, 0x41 | (idx << 3)]);
    }
    if dest.starts_with('(') && dest.ends_with(')') && src == "A" {
        let n = parse_number(&dest[1..dest.len() - 1])?;
        return Ok(vec![0xD3, n as u8]);
    }
    Err(format!(
        "assemble: unsupported OUT operands \"{},{}\"",
        ops[0], ops[1]
    ))
}

/// LD's operand space is the largest in the instruction set -- dispatched by
/// shape of the two operands rather than a flat opcode table, mirroring how
/// a human reads Z80 syntax (dest,src).
fn assemble_ld(ops: &[&str]) -> Result<Vec<u8>, String> {
    let dest_u = ops[0].trim().to_ascii_uppercase();
    let src_u = ops[1].trim().to_ascii_uppercase();

    // LD dd,nn / LD IX/IY,nn (16-bit immediate loads).
    if matches!(dest_u.as_str(), "BC" | "DE" | "HL" | "SP") && parse_number(ops[1]).is_ok() {
        let addr = parse_number(ops[1])?;
        let op = match dest_u.as_str() {
            "BC" => 0x01,
            "DE" => 0x11,
            "HL" => 0x21,
            "SP" => 0x31,
            _ => unreachable!(),
        };
        return Ok(bytes3(op, addr));
    }
    if matches!(dest_u.as_str(), "IX" | "IY") && parse_number(ops[1]).is_ok() {
        let addr = parse_number(ops[1])?;
        let prefix = if dest_u == "IX" { 0xDD } else { 0xFD };
        let mut out = bytes3(0x21, addr);
        out.insert(0, prefix);
        return Ok(out);
    }

    // LD SP,HL/IX/IY.
    if dest_u == "SP" && matches!(src_u.as_str(), "HL" | "IX" | "IY") {
        return match src_u.as_str() {
            "HL" => Ok(vec![0xF9]),
            "IX" => Ok(vec![0xDD, 0xF9]),
            "IY" => Ok(vec![0xFD, 0xF9]),
            _ => unreachable!(),
        };
    }

    // LD A,(BC)/(DE)/(nn) and LD (BC)/(DE)/(nn),A.
    if dest_u == "A" && src_u == "(BC)" {
        return Ok(vec![0x0A]);
    }
    if dest_u == "A" && src_u == "(DE)" {
        return Ok(vec![0x1A]);
    }
    if dest_u == "(BC)" && src_u == "A" {
        return Ok(vec![0x02]);
    }
    if dest_u == "(DE)" && src_u == "A" {
        return Ok(vec![0x12]);
    }
    if dest_u == "A" && src_u.starts_with("(0X") && src_u.ends_with(')') {
        let addr = parse_number(&src_u[1..src_u.len() - 1])?;
        return Ok(bytes3(0x3A, addr));
    }
    if dest_u.starts_with("(0X") && dest_u.ends_with(')') && src_u == "A" {
        let addr = parse_number(&dest_u[1..dest_u.len() - 1])?;
        return Ok(bytes3(0x32, addr));
    }

    // LD HL/IX/IY,(nn) and LD (nn),HL/IX/IY.
    if matches!(dest_u.as_str(), "HL" | "IX" | "IY")
        && src_u.starts_with("(0X")
        && src_u.ends_with(')')
    {
        let addr = parse_number(&src_u[1..src_u.len() - 1])?;
        let mut out = bytes3(0x2A, addr);
        match dest_u.as_str() {
            "HL" => {}
            "IX" => out.insert(0, 0xDD),
            "IY" => out.insert(0, 0xFD),
            _ => unreachable!(),
        }
        return Ok(out);
    }
    if dest_u.starts_with("(0X")
        && dest_u.ends_with(')')
        && matches!(src_u.as_str(), "HL" | "IX" | "IY")
    {
        let addr = parse_number(&dest_u[1..dest_u.len() - 1])?;
        let mut out = bytes3(0x22, addr);
        match src_u.as_str() {
            "HL" => {}
            "IX" => out.insert(0, 0xDD),
            "IY" => out.insert(0, 0xFD),
            _ => unreachable!(),
        }
        return Ok(out);
    }

    // General 8-bit LD r,r'/r,n/r,(HL)/(IX+d)/(IY+d) (and the reverse).
    let dest_op = parse_reg_or_imm(ops[0]);
    let src_op = parse_reg_or_imm(ops[1]);
    match (dest_op, src_op) {
        (Ok(Operand::Reg(d_idx, d_prefix, d_disp)), Ok(Operand::Reg(s_idx, s_prefix, s_disp))) => {
            if d_idx == 6 && s_idx == 6 {
                return Err("assemble: LD (HL),(HL) is HALT, not a load -- use HALT".to_string());
            }
            // At most one side can carry a DD/FD prefix (the (HL)-role side,
            // per real Z80 encoding -- LD r,r' has only one prefix byte).
            let prefix = d_prefix.or(s_prefix);
            let mut out = Vec::new();
            if let Some(ix) = prefix {
                out.push(prefix_byte(ix));
            }
            out.push(0x40 | (d_idx << 3) | s_idx);
            if let Some(d) = d_disp.or(s_disp) {
                out.push(d as u8);
            }
            Ok(out)
        }
        (Ok(Operand::Reg(idx, prefix, disp)), Ok(Operand::Imm(_))) => {
            let n = parse_number(ops[1])? as u8;
            let mut out = Vec::new();
            if let Some(ix) = prefix {
                out.push(prefix_byte(ix));
            }
            if idx == 6 {
                out.push(0x36);
                if let Some(d) = disp {
                    out.push(d as u8);
                }
                out.push(n);
            } else {
                out.push(0x06 | (idx << 3));
                out.push(n);
            }
            Ok(out)
        }
        (Err(e), _) | (_, Err(e)) => Err(e),
        _ => Err(format!(
            "assemble: unsupported LD operands \"{}\",\"{}\"",
            ops[0], ops[1]
        )),
    }
}

#[cfg(test)]
mod absolute_target_tests {
    use super::*;

    #[test]
    fn relative_jumps_print_their_absolute_target() {
        assert_eq!(disassemble_at(&[0x20, 0x10], 0x8000).0, "JR NZ,0x8012");
        assert_eq!(disassemble_at(&[0x18, 0xFE], 0x8000).0, "JR 0x8000"); // onto itself
        assert_eq!(disassemble_at(&[0x10, 0xFB], 0x1B60).0, "DJNZ 0x1B5D");
        assert_eq!(disassemble_at(&[0x30, 0x80], 0x0100).0, "JR NC,0x0082");
        assert_eq!(disassemble_at(&[0x38, 0x7F], 0xFFF0).0, "JR C,0x0071"); // wraps at 64K
        // everything else is unchanged
        assert_eq!(
            disassemble_at(&[0x3E, 0x05], 0x8000),
            disassemble(&[0x3E, 0x05])
        );
        assert_eq!(disassemble_at(&[0xCD, 0x34, 0x12], 0x8000).0, "CALL 0x1234");
    }

    #[test]
    fn absolute_targets_assemble_back_and_round_trip() {
        for op in [0x10u8, 0x18, 0x20, 0x28, 0x30, 0x38] {
            for d in [0x00u8, 0x01, 0x7F, 0x80, 0xFE, 0xFF] {
                let addr = 0x4000u16;
                let (text, len) = disassemble_at(&[op, d], addr);
                assert_eq!(len, 2);
                assert_eq!(assemble_one_at(&text, addr).unwrap(), vec![op, d], "{text}");
            }
        }
        assert_eq!(
            assemble_one_at("JR NZ,0x8012", 0x8000).unwrap(),
            vec![0x20, 0x10]
        );
        assert_eq!(
            assemble_one_at("DJNZ 0x7FFC", 0x8000).unwrap(),
            vec![0x10, 0xFA]
        );
        assert_eq!(assemble_one_at("LD A,5", 0x8000).unwrap(), vec![0x3E, 0x05]);
    }

    #[test]
    fn out_of_reach_relative_targets_are_an_error() {
        assert!(
            assemble_one_at("JR 0x9000", 0x8000)
                .unwrap_err()
                .contains("out of range")
        );
        assert!(
            assemble_one_at("DJNZ 0x7000", 0x8000)
                .unwrap_err()
                .contains("out of range")
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_indexed_cb_copy_target_is_h_and_l_not_ixh_ixl() {
        // DD CB 05 04 / FD CB 05 05: the executor writes the real H / L, and the text says so.
        assert_eq!(disassemble(&[0xDD, 0xCB, 0x05, 0x04]).0, "RLC (IX+0x05),H");
        assert_eq!(disassemble(&[0xFD, 0xCB, 0x05, 0x05]).0, "RLC (IY+0x05),L");
        assert_eq!(
            disassemble(&[0xDD, 0xCB, 0x05, 0x84]).0,
            "RES 0,(IX+0x05),H"
        );
        assert_eq!(
            disassemble(&[0xFD, 0xCB, 0x05, 0xFD]).0,
            "SET 7,(IY+0x05),L"
        );
    }

    use super::*;

    #[test]
    fn disassembles_an_unprefixed_instruction() {
        assert_eq!(disassemble(&[0x3E, 0x05]), ("LD A,0x05".to_string(), 2));
    }

    #[test]
    fn disassembles_a_cb_prefixed_instruction() {
        assert_eq!(disassemble(&[0xCB, 0x00]), ("RLC B".to_string(), 2));
    }

    #[test]
    fn disassembles_an_ed_prefixed_instruction() {
        assert_eq!(disassemble(&[0xED, 0x44]), ("NEG".to_string(), 2));
    }

    #[test]
    fn disassembles_dd_prefixed_register_substitution() {
        // DD 7C -- undocumented LD A,IXH.
        assert_eq!(disassemble(&[0xDD, 0x7C]), ("LD A,IXH".to_string(), 2));
    }

    #[test]
    fn disassembles_the_other_operand_stays_real_h_exception() {
        // DD 74 -- LD (IX+d),H stores the REAL H, not IXH (real_reg_get).
        assert_eq!(
            disassemble(&[0xDD, 0x74, 0x05]),
            ("LD (IX+0x05),H".to_string(), 3)
        );
    }

    #[test]
    fn disassembles_the_dd_cb_four_byte_form_with_duplicate_target() {
        // DD CB 05 00 -- RLC (IX+5),B (undocumented copy-to-register form).
        assert_eq!(
            disassemble(&[0xDD, 0xCB, 0x05, 0x00]),
            ("RLC (IX+0x05),B".to_string(), 4)
        );
    }

    #[test]
    fn disassembles_the_dd_cb_form_with_no_duplicate_target() {
        // DD CB 00 06 -- RLC (IX+0), low3==6, no register copy.
        assert_eq!(
            disassemble(&[0xDD, 0xCB, 0x00, 0x06]),
            ("RLC (IX+0x00)".to_string(), 4)
        );
    }

    #[test]
    fn disassembles_an_undocumented_sll_opcode() {
        assert_eq!(disassemble(&[0xCB, 0x30]), ("SLL B".to_string(), 2));
    }

    #[test]
    fn unimplemented_ed_opcode_disassembles_to_a_safe_placeholder_without_panicking() {
        // ED 00 is not installed by any opcode group in this crate.
        let (text, len) = disassemble(&[0xED, 0x00]);
        assert!(!text.is_empty());
        assert_eq!(len, 2);
    }

    #[test]
    fn empty_bytes_do_not_panic() {
        let (text, len) = disassemble(&[]);
        assert!(!text.is_empty());
        assert!(len >= 1);
    }

    #[test]
    fn assemble_then_disassemble_round_trips_ld_a_n() {
        let bytes = assemble_one("LD A,5").unwrap();
        assert_eq!(disassemble(&bytes), ("LD A,0x05".to_string(), bytes.len()));
    }

    #[test]
    fn assemble_is_case_insensitive() {
        let upper = assemble_one("LD A,5").unwrap();
        let lower = assemble_one("ld a,5").unwrap();
        assert_eq!(upper, lower);
    }

    #[test]
    fn assemble_then_disassemble_round_trips_a_jump() {
        let bytes = assemble_one("JP 0x8000").unwrap();
        assert_eq!(disassemble(&bytes), ("JP 0x8000".to_string(), 3));
    }

    #[test]
    fn assemble_then_disassemble_round_trips_an_indexed_load() {
        let bytes = assemble_one("LD A,(IX+5)").unwrap();
        assert_eq!(
            disassemble(&bytes),
            ("LD A,(IX+0x05)".to_string(), bytes.len())
        );
    }

    #[test]
    fn assemble_rejects_a_label_operand_with_a_clear_message() {
        let err = assemble_one("JP loop").unwrap_err();
        assert!(err.contains("label"), "error should mention labels: {err}");
    }

    #[test]
    fn assemble_rejects_an_unknown_mnemonic() {
        let err = assemble_one("FROB A,B").unwrap_err();
        assert!(err.contains("FROB"));
    }
}
