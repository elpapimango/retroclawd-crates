//! The instruction set as data: one pattern per instruction form (PRM chapter 4 and 8 encodings), each with the
//! effective-address modes it accepts. The 65 536-entry decode table is built from it once, and the disassembler
//! reads the same patterns, so decoding and disassembly cannot disagree.

use std::sync::OnceLock;

/// Operand size of an instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sz {
    B,
    W,
    L,
}

impl Sz {
    pub(crate) fn bytes(self) -> u32 {
        match self {
            Sz::B => 1,
            Sz::W => 2,
            Sz::L => 4,
        }
    }

    pub(crate) fn mask(self) -> u32 {
        match self {
            Sz::B => 0xFF,
            Sz::W => 0xFFFF,
            Sz::L => 0xFFFF_FFFF,
        }
    }

    pub(crate) fn msb(self) -> u32 {
        match self {
            Sz::B => 0x80,
            Sz::W => 0x8000,
            Sz::L => 0x8000_0000,
        }
    }
}

/// The operation of the ADD and SUB families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Alu {
    Add,
    Sub,
}

/// What an opcode word decodes to; the handler reads registers and modes from the word itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Illegal,
    Nop,
    Move(Sz),
    Movea(Sz),
    Moveq,
    Lea,
    Pea,
    Clr(Sz),
    Tst(Sz),
    Exg,
    Swap,
    Ext(Sz),
    Arith(Alu, Sz),
    ArithMem(Alu, Sz),
    ArithA(Alu, Sz),
    ArithX(Alu, Sz),
    ArithI(Alu, Sz),
    ArithQ(Alu, Sz),
    Cmp(Sz),
    Cmpa(Sz),
    Cmpm(Sz),
    Cmpi(Sz),
    Neg(Sz),
    Negx(Sz),
}

/// A set of effective-address modes, one bit per mode in `EaSet::*` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EaSet(pub u16);

impl EaSet {
    pub const NONE: Self = Self(0);
    pub const DN: u16 = 1 << 0;
    pub const AN: u16 = 1 << 1;
    pub const IND: u16 = 1 << 2;
    pub const POSTINC: u16 = 1 << 3;
    pub const PREDEC: u16 = 1 << 4;
    pub const DISP: u16 = 1 << 5;
    pub const INDEX: u16 = 1 << 6;
    pub const ABS_W: u16 = 1 << 7;
    pub const ABS_L: u16 = 1 << 8;
    pub const PC_DISP: u16 = 1 << 9;
    pub const PC_INDEX: u16 = 1 << 10;
    pub const IMM: u16 = 1 << 11;

    /// PRM 2.2.6 categories.
    pub const ALL: Self = Self(0x0FFF);
    pub const DATA: Self = Self(0x0FFF & !Self::AN);
    pub const MEMORY: Self = Self(0x0FFF & !Self::AN & !Self::DN);
    pub const CONTROL: Self = Self(
        Self::IND
            | Self::DISP
            | Self::INDEX
            | Self::ABS_W
            | Self::ABS_L
            | Self::PC_DISP
            | Self::PC_INDEX,
    );
    pub const ALTERABLE: Self = Self(0x01FF);
    pub const DATA_ALT: Self = Self(0x01FF & !Self::AN);
    pub const MEMORY_ALT: Self = Self(0x01FF & !Self::AN & !Self::DN);
    pub const CONTROL_ALT: Self =
        Self(Self::IND | Self::DISP | Self::INDEX | Self::ABS_W | Self::ABS_L);

    fn contains(self, mode: Option<u16>) -> bool {
        mode.is_some_and(|m| self.0 & m != 0)
    }
}

/// The mode bit of an effective-address field (`mode`, `reg` as encoded), or `None` for an invalid one.
pub(crate) fn ea_mode(mode: u16, reg: u16) -> Option<u16> {
    Some(match (mode, reg) {
        (0, _) => EaSet::DN,
        (1, _) => EaSet::AN,
        (2, _) => EaSet::IND,
        (3, _) => EaSet::POSTINC,
        (4, _) => EaSet::PREDEC,
        (5, _) => EaSet::DISP,
        (6, _) => EaSet::INDEX,
        (7, 0) => EaSet::ABS_W,
        (7, 1) => EaSet::ABS_L,
        (7, 2) => EaSet::PC_DISP,
        (7, 3) => EaSet::PC_INDEX,
        (7, 4) => EaSet::IMM,
        _ => return None,
    })
}

/// One instruction form.
pub struct OpPattern {
    pub mask: u16,
    pub bits: u16,
    pub mnemonic: &'static str,
    /// Modes accepted in bits 5-0 (`NONE`: that field is not an effective address).
    pub ea: EaSet,
    /// Modes accepted in MOVE's destination field, bits 11-6 (register 11-9, mode 8-6).
    pub ea2: EaSet,
    pub(crate) op: Op,
}

const fn p(
    mask: u16,
    bits: u16,
    mnemonic: &'static str,
    ea: EaSet,
    ea2: EaSet,
    op: Op,
) -> OpPattern {
    OpPattern {
        mask,
        bits,
        mnemonic,
        ea,
        ea2,
        op,
    }
}

const N: EaSet = EaSet::NONE;

pub static OPCODES: &[OpPattern] = &[
    p(0xFFFF, 0x4E71, "nop", N, N, Op::Nop),
    p(
        0xF000,
        0x1000,
        "move.b",
        EaSet::DATA,
        EaSet::DATA_ALT,
        Op::Move(Sz::B),
    ),
    p(
        0xF000,
        0x3000,
        "move.w",
        EaSet::ALL,
        EaSet::DATA_ALT,
        Op::Move(Sz::W),
    ),
    p(
        0xF000,
        0x2000,
        "move.l",
        EaSet::ALL,
        EaSet::DATA_ALT,
        Op::Move(Sz::L),
    ),
    p(0xF1C0, 0x3040, "movea.w", EaSet::ALL, N, Op::Movea(Sz::W)),
    p(0xF1C0, 0x2040, "movea.l", EaSet::ALL, N, Op::Movea(Sz::L)),
    p(0xF100, 0x7000, "moveq", N, N, Op::Moveq),
    p(0xF1C0, 0x41C0, "lea", EaSet::CONTROL, N, Op::Lea),
    p(0xFFC0, 0x4840, "pea", EaSet::CONTROL, N, Op::Pea),
    p(0xFFC0, 0x4200, "clr.b", EaSet::DATA_ALT, N, Op::Clr(Sz::B)),
    p(0xFFC0, 0x4240, "clr.w", EaSet::DATA_ALT, N, Op::Clr(Sz::W)),
    p(0xFFC0, 0x4280, "clr.l", EaSet::DATA_ALT, N, Op::Clr(Sz::L)),
    p(0xFFC0, 0x4A00, "tst.b", EaSet::DATA_ALT, N, Op::Tst(Sz::B)),
    p(0xFFC0, 0x4A40, "tst.w", EaSet::DATA_ALT, N, Op::Tst(Sz::W)),
    p(0xFFC0, 0x4A80, "tst.l", EaSet::DATA_ALT, N, Op::Tst(Sz::L)),
    p(0xF1F8, 0xC140, "exg", N, N, Op::Exg),
    p(0xF1F8, 0xC148, "exg", N, N, Op::Exg),
    p(0xF1F8, 0xC188, "exg", N, N, Op::Exg),
    p(0xFFF8, 0x4840, "swap", N, N, Op::Swap),
    p(0xFFF8, 0x4880, "ext.w", N, N, Op::Ext(Sz::W)),
    p(0xFFF8, 0x48C0, "ext.l", N, N, Op::Ext(Sz::L)),
    p(
        0xF1C0,
        0xD000,
        "add.b",
        EaSet::DATA,
        N,
        Op::Arith(Alu::Add, Sz::B),
    ),
    p(
        0xF1C0,
        0xD100,
        "add.b",
        EaSet::MEMORY_ALT,
        N,
        Op::ArithMem(Alu::Add, Sz::B),
    ),
    p(0xF1F0, 0xD100, "addx.b", N, N, Op::ArithX(Alu::Add, Sz::B)),
    p(
        0xF1C0,
        0xD040,
        "add.w",
        EaSet::ALL,
        N,
        Op::Arith(Alu::Add, Sz::W),
    ),
    p(
        0xF1C0,
        0xD140,
        "add.w",
        EaSet::MEMORY_ALT,
        N,
        Op::ArithMem(Alu::Add, Sz::W),
    ),
    p(0xF1F0, 0xD140, "addx.w", N, N, Op::ArithX(Alu::Add, Sz::W)),
    p(
        0xF1C0,
        0xD080,
        "add.l",
        EaSet::ALL,
        N,
        Op::Arith(Alu::Add, Sz::L),
    ),
    p(
        0xF1C0,
        0xD180,
        "add.l",
        EaSet::MEMORY_ALT,
        N,
        Op::ArithMem(Alu::Add, Sz::L),
    ),
    p(0xF1F0, 0xD180, "addx.l", N, N, Op::ArithX(Alu::Add, Sz::L)),
    p(
        0xF1C0,
        0xD0C0,
        "adda.w",
        EaSet::ALL,
        N,
        Op::ArithA(Alu::Add, Sz::W),
    ),
    p(
        0xF1C0,
        0xD1C0,
        "adda.l",
        EaSet::ALL,
        N,
        Op::ArithA(Alu::Add, Sz::L),
    ),
    p(
        0xF1C0,
        0x9000,
        "sub.b",
        EaSet::DATA,
        N,
        Op::Arith(Alu::Sub, Sz::B),
    ),
    p(
        0xF1C0,
        0x9100,
        "sub.b",
        EaSet::MEMORY_ALT,
        N,
        Op::ArithMem(Alu::Sub, Sz::B),
    ),
    p(0xF1F0, 0x9100, "subx.b", N, N, Op::ArithX(Alu::Sub, Sz::B)),
    p(
        0xF1C0,
        0x9040,
        "sub.w",
        EaSet::ALL,
        N,
        Op::Arith(Alu::Sub, Sz::W),
    ),
    p(
        0xF1C0,
        0x9140,
        "sub.w",
        EaSet::MEMORY_ALT,
        N,
        Op::ArithMem(Alu::Sub, Sz::W),
    ),
    p(0xF1F0, 0x9140, "subx.w", N, N, Op::ArithX(Alu::Sub, Sz::W)),
    p(
        0xF1C0,
        0x9080,
        "sub.l",
        EaSet::ALL,
        N,
        Op::Arith(Alu::Sub, Sz::L),
    ),
    p(
        0xF1C0,
        0x9180,
        "sub.l",
        EaSet::MEMORY_ALT,
        N,
        Op::ArithMem(Alu::Sub, Sz::L),
    ),
    p(0xF1F0, 0x9180, "subx.l", N, N, Op::ArithX(Alu::Sub, Sz::L)),
    p(
        0xF1C0,
        0x90C0,
        "suba.w",
        EaSet::ALL,
        N,
        Op::ArithA(Alu::Sub, Sz::W),
    ),
    p(
        0xF1C0,
        0x91C0,
        "suba.l",
        EaSet::ALL,
        N,
        Op::ArithA(Alu::Sub, Sz::L),
    ),
    p(
        0xFFC0,
        0x0600,
        "addi.b",
        EaSet::DATA_ALT,
        N,
        Op::ArithI(Alu::Add, Sz::B),
    ),
    p(
        0xFFC0,
        0x0640,
        "addi.w",
        EaSet::DATA_ALT,
        N,
        Op::ArithI(Alu::Add, Sz::W),
    ),
    p(
        0xFFC0,
        0x0680,
        "addi.l",
        EaSet::DATA_ALT,
        N,
        Op::ArithI(Alu::Add, Sz::L),
    ),
    p(
        0xFFC0,
        0x0400,
        "subi.b",
        EaSet::DATA_ALT,
        N,
        Op::ArithI(Alu::Sub, Sz::B),
    ),
    p(
        0xFFC0,
        0x0440,
        "subi.w",
        EaSet::DATA_ALT,
        N,
        Op::ArithI(Alu::Sub, Sz::W),
    ),
    p(
        0xFFC0,
        0x0480,
        "subi.l",
        EaSet::DATA_ALT,
        N,
        Op::ArithI(Alu::Sub, Sz::L),
    ),
    p(
        0xF1C0,
        0x5000,
        "addq.b",
        EaSet::DATA_ALT,
        N,
        Op::ArithQ(Alu::Add, Sz::B),
    ),
    p(
        0xF1C0,
        0x5040,
        "addq.w",
        EaSet::ALTERABLE,
        N,
        Op::ArithQ(Alu::Add, Sz::W),
    ),
    p(
        0xF1C0,
        0x5080,
        "addq.l",
        EaSet::ALTERABLE,
        N,
        Op::ArithQ(Alu::Add, Sz::L),
    ),
    p(
        0xF1C0,
        0x5100,
        "subq.b",
        EaSet::DATA_ALT,
        N,
        Op::ArithQ(Alu::Sub, Sz::B),
    ),
    p(
        0xF1C0,
        0x5140,
        "subq.w",
        EaSet::ALTERABLE,
        N,
        Op::ArithQ(Alu::Sub, Sz::W),
    ),
    p(
        0xF1C0,
        0x5180,
        "subq.l",
        EaSet::ALTERABLE,
        N,
        Op::ArithQ(Alu::Sub, Sz::L),
    ),
    p(0xF1C0, 0xB000, "cmp.b", EaSet::DATA, N, Op::Cmp(Sz::B)),
    p(0xF1F8, 0xB108, "cmpm.b", N, N, Op::Cmpm(Sz::B)),
    p(
        0xFFC0,
        0x0C00,
        "cmpi.b",
        EaSet::DATA_ALT,
        N,
        Op::Cmpi(Sz::B),
    ),
    p(0xFFC0, 0x4400, "neg.b", EaSet::DATA_ALT, N, Op::Neg(Sz::B)),
    p(
        0xFFC0,
        0x4000,
        "negx.b",
        EaSet::DATA_ALT,
        N,
        Op::Negx(Sz::B),
    ),
    p(0xF1C0, 0xB040, "cmp.w", EaSet::ALL, N, Op::Cmp(Sz::W)),
    p(0xF1F8, 0xB148, "cmpm.w", N, N, Op::Cmpm(Sz::W)),
    p(
        0xFFC0,
        0x0C40,
        "cmpi.w",
        EaSet::DATA_ALT,
        N,
        Op::Cmpi(Sz::W),
    ),
    p(0xFFC0, 0x4440, "neg.w", EaSet::DATA_ALT, N, Op::Neg(Sz::W)),
    p(
        0xFFC0,
        0x4040,
        "negx.w",
        EaSet::DATA_ALT,
        N,
        Op::Negx(Sz::W),
    ),
    p(0xF1C0, 0xB080, "cmp.l", EaSet::ALL, N, Op::Cmp(Sz::L)),
    p(0xF1F8, 0xB188, "cmpm.l", N, N, Op::Cmpm(Sz::L)),
    p(
        0xFFC0,
        0x0C80,
        "cmpi.l",
        EaSet::DATA_ALT,
        N,
        Op::Cmpi(Sz::L),
    ),
    p(0xFFC0, 0x4480, "neg.l", EaSet::DATA_ALT, N, Op::Neg(Sz::L)),
    p(
        0xFFC0,
        0x4080,
        "negx.l",
        EaSet::DATA_ALT,
        N,
        Op::Negx(Sz::L),
    ),
    p(0xF1C0, 0xB0C0, "cmpa.w", EaSet::ALL, N, Op::Cmpa(Sz::W)),
    p(0xF1C0, 0xB1C0, "cmpa.l", EaSet::ALL, N, Op::Cmpa(Sz::L)),
];

impl OpPattern {
    /// Whether `word` is this form, with legal effective-address fields.
    pub fn matches(&self, word: u16) -> bool {
        if word & self.mask != self.bits {
            return false;
        }
        if self.ea != N && !self.ea.contains(ea_mode((word >> 3) & 7, word & 7)) {
            return false;
        }
        if self.ea2 != N && !self.ea2.contains(ea_mode((word >> 6) & 7, (word >> 9) & 7)) {
            return false;
        }
        true
    }
}

/// The pattern `word` decodes to, if any.
pub fn find(word: u16) -> Option<&'static OpPattern> {
    OPCODES.iter().find(|p| p.matches(word))
}

pub(crate) fn decode(word: u16) -> Op {
    static TABLE: OnceLock<Box<[Op]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|w| find(w).map_or(Op::Illegal, |p| p.op))
            .collect()
    })[word as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_patterns_never_overlap() {
        for w in 0..=u16::MAX {
            let hits: Vec<_> = OPCODES
                .iter()
                .filter(|p| p.matches(w))
                .map(|p| p.mnemonic)
                .collect();
            assert!(hits.len() <= 1, "{w:04X} matches {hits:?}");
        }
    }
}
