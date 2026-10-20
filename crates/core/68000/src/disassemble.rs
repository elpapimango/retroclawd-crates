//! One-instruction disassembler in Motorola syntax, driven by the same pattern table as the decoder
//! (`crate::opcodes::OPCODES`), so it accepts exactly the words the core executes.
//!
//! Syntax (lower case, hex as `$`): `move.w $10(a0),-(a1)`, `lea $fc0030,a0`, `$1234.w` for absolute short,
//! `$40(a0,d1.w)` for indexed, PC-relative operands as their absolute target (`$fc0040(pc)`), branch targets as
//! absolute addresses, MOVEM lists as `d0-d7/a0-a6`. A word that is no instruction, or whose extension words are
//! missing, comes back as `dc.w` (`dc.b` for a lone byte).

use crate::ea::{sext8, sext16};
use crate::opcodes::{Alu, Logic, Op, Shift, Sz, find};

const CONDITIONS: [&str; 16] = [
    "t", "f", "hi", "ls", "cc", "cs", "ne", "eq", "vc", "vs", "pl", "mi", "ge", "lt", "gt", "le",
];

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    pc: u32,
}

impl Reader<'_> {
    /// The address of the next unread word.
    fn addr(&self) -> u32 {
        self.pc.wrapping_add(self.pos as u32)
    }

    fn word(&mut self) -> Option<u32> {
        let w = self.bytes.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([w[0], w[1]]) as u32)
    }

    fn long(&mut self) -> Option<u32> {
        Some(self.word()? << 16 | self.word()?)
    }

    fn imm(&mut self, sz: Sz) -> Option<String> {
        Some(match sz {
            Sz::B => format!("#${:x}", self.word()? & 0xFF),
            Sz::W => format!("#${:x}", self.word()?),
            Sz::L => format!("#${:x}", self.long()?),
        })
    }
}

fn signed_hex(v: i32) -> String {
    if v < 0 {
        format!("-${:x}", v.unsigned_abs())
    } else {
        format!("${v:x}")
    }
}

fn suffix(sz: Sz) -> &'static str {
    match sz {
        Sz::B => ".b",
        Sz::W => ".w",
        Sz::L => ".l",
    }
}

/// The text of effective-address field (`mode`, `reg`).
fn ea(r: &mut Reader, mode: u16, reg: u16, sz: Sz) -> Option<String> {
    Some(match (mode, reg) {
        (0, n) => format!("d{n}"),
        (1, n) => format!("a{n}"),
        (2, n) => format!("(a{n})"),
        (3, n) => format!("(a{n})+"),
        (4, n) => format!("-(a{n})"),
        (5, n) => format!("{}(a{n})", signed_hex(sext16(r.word()?) as i32)),
        (6, n) => index(r, &format!("a{n}"), 0)?,
        (7, 0) => format!("${:x}.w", r.word()?),
        (7, 1) => format!("${:x}", r.long()?),
        (7, 2) => {
            let base = r.addr();
            format!(
                "${:x}(pc)",
                base.wrapping_add(sext16(r.word()?)) & 0xFF_FFFF
            )
        }
        (7, 3) => {
            let base = r.addr();
            index(r, "pc", base)?
        }
        _ => r.imm(sz)?,
    })
}

/// `d8(base,Xn.s)`; for PC the displacement is folded into the absolute target.
fn index(r: &mut Reader, base: &str, pc: u32) -> Option<String> {
    let ext = r.word()?;
    let xn = format!(
        "{}{}{}",
        if ext & 0x8000 != 0 { "a" } else { "d" },
        (ext >> 12) & 7,
        if ext & 0x0800 != 0 { ".l" } else { ".w" }
    );
    let d = sext8(ext) as i32;
    Some(if base == "pc" {
        format!("${:x}(pc,{xn})", pc.wrapping_add(d as u32) & 0xFF_FFFF)
    } else {
        format!("{}({base},{xn})", signed_hex(d))
    })
}

/// A MOVEM register list; `reversed` for `-(An)`, where bit 0 is A7.
fn reglist(mask: u32, reversed: bool) -> String {
    let has = |i: u32| {
        let bit = if reversed { 15 - i } else { i };
        mask & (1 << bit) != 0
    };
    let mut parts = Vec::new();
    for (kind, base) in [("d", 0), ("a", 8)] {
        let mut i = 0;
        while i < 8 {
            if !has(base + i) {
                i += 1;
                continue;
            }
            let start = i;
            while i + 1 < 8 && has(base + i + 1) {
                i += 1;
            }
            parts.push(if start == i {
                format!("{kind}{start}")
            } else {
                format!("{kind}{start}-{kind}{i}")
            });
            i += 1;
        }
    }
    parts.join("/")
}

fn alu_name(alu: Alu) -> &'static str {
    match alu {
        Alu::Add => "add",
        Alu::Sub => "sub",
    }
}

fn logic_name(logic: Logic) -> &'static str {
    match logic {
        Logic::And => "and",
        Logic::Or => "or",
        Logic::Eor => "eor",
    }
}

fn shift_name(kind: Shift, left: bool) -> String {
    let stem = match kind {
        Shift::As => "as",
        Shift::Ls => "ls",
        Shift::Rox => "rox",
        Shift::Ro => "ro",
    };
    format!("{stem}{}", if left { "l" } else { "r" })
}

/// Disassembles the instruction at the start of `bytes`, located at address `pc`: its text and length in bytes.
pub fn disassemble(bytes: &[u8], pc: u32) -> (String, usize) {
    let mut r = Reader { bytes, pos: 0, pc };
    match r.word() {
        None => match bytes.first() {
            Some(b) => (format!("dc.b ${b:x}"), 1),
            None => (String::new(), 0),
        },
        Some(op) => match instruction(&mut r, op as u16) {
            Some(text) => (text, r.pos),
            None => (format!("dc.w ${op:x}"), 2),
        },
    }
}

fn instruction(r: &mut Reader, op: u16) -> Option<String> {
    if op == 0x4AFC {
        return Some("illegal".into());
    }
    let pattern = find(op)?;
    let (m, n) = ((op >> 3) & 7, op & 7);
    let rx = (op >> 9) & 7;
    let opcode_addr = r.pc;
    Some(match pattern.op {
        Op::Illegal => return None,
        Op::Nop | Op::Rts | Op::Rtr | Op::Rte | Op::Reset | Op::Trapv => pattern.mnemonic.into(),
        Op::Move(sz) => {
            let src = ea(r, m, n, sz)?;
            let dst = ea(r, (op >> 6) & 7, rx, sz)?;
            format!("move{} {src},{dst}", suffix(sz))
        }
        Op::Movea(sz) => format!("movea{} {},a{rx}", suffix(sz), ea(r, m, n, sz)?),
        Op::Moveq => format!("moveq #{},d{rx}", signed_hex(sext8(op as u32) as i32)),
        Op::Lea => format!("lea {},a{rx}", ea(r, m, n, Sz::L)?),
        Op::Pea => format!("pea {}", ea(r, m, n, Sz::L)?),
        Op::Clr(sz) | Op::Tst(sz) | Op::Neg(sz) | Op::Negx(sz) | Op::Not(sz) => {
            let stem = pattern
                .mnemonic
                .split('.')
                .next()
                .unwrap_or(pattern.mnemonic);
            format!("{stem}{} {}", suffix(sz), ea(r, m, n, sz)?)
        }
        Op::Nbcd | Op::Tas => format!("{} {}", pattern.mnemonic, ea(r, m, n, Sz::B)?),
        Op::Exg => match (op >> 3) & 0x1F {
            0b01000 => format!("exg d{rx},d{n}"),
            0b01001 => format!("exg a{rx},a{n}"),
            _ => format!("exg d{rx},a{n}"),
        },
        Op::Swap => format!("swap d{n}"),
        Op::Ext(sz) => format!("ext{} d{n}", suffix(sz)),
        Op::Arith(alu, sz) => format!("{}{} {},d{rx}", alu_name(alu), suffix(sz), ea(r, m, n, sz)?),
        Op::ArithMem(alu, sz) => {
            format!("{}{} d{rx},{}", alu_name(alu), suffix(sz), ea(r, m, n, sz)?)
        }
        Op::ArithA(alu, sz) => format!(
            "{}a{} {},a{rx}",
            alu_name(alu),
            suffix(sz),
            ea(r, m, n, sz)?
        ),
        Op::ArithX(alu, sz) => {
            let operands = if op & 8 == 0 {
                format!("d{n},d{rx}")
            } else {
                format!("-(a{n}),-(a{rx})")
            };
            format!("{}x{} {operands}", alu_name(alu), suffix(sz))
        }
        Op::ArithI(alu, sz) => {
            let imm = r.imm(sz)?;
            format!(
                "{}i{} {imm},{}",
                alu_name(alu),
                suffix(sz),
                ea(r, m, n, sz)?
            )
        }
        Op::ArithQ(alu, sz) => {
            let q = if rx == 0 { 8 } else { rx };
            format!("{}q{} #{q},{}", alu_name(alu), suffix(sz), ea(r, m, n, sz)?)
        }
        Op::Cmp(sz) => format!("cmp{} {},d{rx}", suffix(sz), ea(r, m, n, sz)?),
        Op::Cmpa(sz) => format!("cmpa{} {},a{rx}", suffix(sz), ea(r, m, n, sz)?),
        Op::Cmpm(sz) => format!("cmpm{} (a{n})+,(a{rx})+", suffix(sz)),
        Op::Cmpi(sz) => {
            let imm = r.imm(sz)?;
            format!("cmpi{} {imm},{}", suffix(sz), ea(r, m, n, sz)?)
        }
        Op::Logic(logic, sz) => format!(
            "{}{} {},d{rx}",
            logic_name(logic),
            suffix(sz),
            ea(r, m, n, sz)?
        ),
        Op::LogicMem(logic, sz) => format!(
            "{}{} d{rx},{}",
            logic_name(logic),
            suffix(sz),
            ea(r, m, n, sz)?
        ),
        Op::LogicI(logic, sz) => {
            let imm = r.imm(sz)?;
            format!(
                "{}i{} {imm},{}",
                logic_name(logic),
                suffix(sz),
                ea(r, m, n, sz)?
            )
        }
        Op::Shift(kind, sz) => {
            let count = if op & 0x20 != 0 {
                format!("d{rx}")
            } else {
                format!("#{}", if rx == 0 { 8 } else { rx })
            };
            format!(
                "{}{} {count},d{n}",
                shift_name(kind, op & 0x100 != 0),
                suffix(sz)
            )
        }
        Op::ShiftMem(kind) => format!(
            "{}.w {}",
            shift_name(kind, op & 0x100 != 0),
            ea(r, m, n, Sz::W)?
        ),
        Op::Bcc => {
            let cc = ((op >> 8) & 0xF) as usize;
            let name = match cc {
                0 => "bra".to_string(),
                1 => "bsr".to_string(),
                _ => format!("b{}", CONDITIONS[cc]),
            };
            let base = opcode_addr.wrapping_add(2);
            let (disp, size) = if op & 0xFF == 0 {
                (sext16(r.word()?), ".w")
            } else {
                (sext8(op as u32), ".s")
            };
            format!("{name}{size} ${:x}", base.wrapping_add(disp) & 0xFF_FFFF)
        }
        Op::Dbcc => {
            let base = r.addr();
            let target = base.wrapping_add(sext16(r.word()?)) & 0xFF_FFFF;
            format!(
                "db{} d{n},${target:x}",
                CONDITIONS[((op >> 8) & 0xF) as usize]
            )
        }
        Op::Scc => format!(
            "s{} {}",
            CONDITIONS[((op >> 8) & 0xF) as usize],
            ea(r, m, n, Sz::B)?
        ),
        Op::Jmp | Op::Jsr => format!("{} {}", pattern.mnemonic, ea(r, m, n, Sz::L)?),
        Op::Link => format!("link a{n},#{}", signed_hex(sext16(r.word()?) as i32)),
        Op::Unlk => format!("unlk a{n}"),
        Op::LogicSr(logic, to_sr) => {
            let imm = r.word()?;
            if to_sr {
                format!("{}i #${imm:x},sr", logic_name(logic))
            } else {
                format!("{}i #${:x},ccr", logic_name(logic), imm & 0xFF)
            }
        }
        Op::MoveToSr(to_sr) => format!(
            "move {},{}",
            ea(r, m, n, Sz::W)?,
            if to_sr { "sr" } else { "ccr" }
        ),
        Op::MoveFromSr => format!("move sr,{}", ea(r, m, n, Sz::W)?),
        Op::MoveUsp => {
            if op & 8 == 0 {
                format!("move a{n},usp")
            } else {
                format!("move usp,a{n}")
            }
        }
        Op::MovemToMem(sz) => {
            let mask = r.word()?;
            format!(
                "movem{} {},{}",
                suffix(sz),
                reglist(mask, m == 4),
                ea(r, m, n, sz)?
            )
        }
        Op::MovemToReg(sz) => {
            let mask = r.word()?;
            format!(
                "movem{} {},{}",
                suffix(sz),
                ea(r, m, n, sz)?,
                reglist(mask, false)
            )
        }
        Op::Movep => {
            let size = if op & 0x40 != 0 { ".l" } else { ".w" };
            let mem = format!("{}(a{n})", signed_hex(sext16(r.word()?) as i32));
            if op & 0x80 == 0 {
                format!("movep{size} {mem},d{rx}")
            } else {
                format!("movep{size} d{rx},{mem}")
            }
        }
        Op::Bit(is_static) => {
            let bit = if is_static {
                format!("#{}", r.word()? & 0xFF)
            } else {
                format!("d{rx}")
            };
            let sz = if m == 0 { Sz::L } else { Sz::B };
            format!("{} {bit},{}", pattern.mnemonic, ea(r, m, n, sz)?)
        }
        Op::Mul(_) | Op::Div(_) | Op::Chk => {
            format!("{}.w {},d{rx}", pattern.mnemonic, ea(r, m, n, Sz::W)?)
        }
        Op::Bcd(_) => {
            if op & 8 == 0 {
                format!("{} d{n},d{rx}", pattern.mnemonic)
            } else {
                format!("{} -(a{n}),-(a{rx})", pattern.mnemonic)
            }
        }
        Op::Trap => format!("trap #{}", op & 0xF),
        Op::Stop => format!("stop #${:x}", r.word()?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dis(words: &[u16], pc: u32) -> (String, usize) {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        disassemble(&bytes, pc)
    }

    #[test]
    fn disassembles_reference_lines() {
        assert_eq!(dis(&[0x4E71], 0), ("nop".into(), 2));
        assert_eq!(dis(&[0x602E], 0xFC0000), ("bra.s $fc0030".into(), 2));
        assert_eq!(
            dis(&[0x41F9, 0x00FC, 0x0030], 0),
            ("lea $fc0030,a0".into(), 6)
        );
        assert_eq!(
            dis(&[0x4CDF, 0x7FFF], 0),
            ("movem.l (a7)+,d0-d7/a0-a6".into(), 4)
        );
        assert_eq!(dis(&[0x0C40, 0x1234], 0), ("cmpi.w #$1234,d0".into(), 4));
        assert_eq!(
            dis(&[0x48E7, 0xFFFE], 0),
            ("movem.l d0-d7/a0-a6,-(a7)".into(), 4)
        );
        assert_eq!(dis(&[0x3229, 0xFFF0], 0), ("move.w -$10(a1),d1".into(), 4));
        assert_eq!(dis(&[0x7CFF], 0), ("moveq #-$1,d6".into(), 2));
    }

    #[test]
    fn truncated_input_falls_back_to_data() {
        assert_eq!(dis(&[0x41F9, 0x00FC], 0), ("dc.w $41f9".into(), 2));
        assert_eq!(disassemble(&[0x4E], 0), ("dc.b $4e".into(), 1));
        assert_eq!(dis(&[0x4AFB], 0), ("dc.w $4afb".into(), 2));
    }
}
