//! Integer arithmetic: ADD, SUB in all their forms (register, memory, address, extended, immediate, quick), CMP,
//! CMPA, CMPM, CMPI, NEG, NEGX (PRM chapter 4; flags from PRM 3.4 / Table 3-18).
//!
//! Timing: the bus cycles fall out of the operand accesses; the internal clocks added here are the differences UM
//! Tables 8-4 to 8-7 give for long operations on registers and for address-register destinations.

use crate::bus::M68kBus;
use crate::cpu::{Cpu, Fault};
use crate::ea::{Loc, sext16};
use crate::opcodes::{Alu, Sz};
use crate::ops::move_::src_field;

const X: u16 = 0x10;
const N: u16 = 0x08;
const Z: u16 = 0x04;
const V: u16 = 0x02;
const C: u16 = 0x01;

/// The source of a two-operand instruction is a register or an immediate (UM: "8 if the effective address is
/// register direct or immediate" for the long forms).
fn reg_or_imm(mode: u16, reg: u16) -> bool {
    mode <= 1 || (mode == 7 && reg == 4)
}

impl Cpu {
    fn set_ccr(&mut self, ccr: u16) {
        self.set_sr((self.sr() & 0xFF00) | ccr);
    }

    fn x_flag(&self) -> u32 {
        ((self.sr() >> 4) & 1) as u32
    }

    /// `d + s (+ x)` at `sz`, setting the flags. `extend`: ADDX, where Z is only ever cleared.
    pub(crate) fn alu_add(&mut self, d: u32, s: u32, x: u32, sz: Sz, extend: bool) -> u32 {
        let r = d.wrapping_add(s).wrapping_add(x) & sz.mask();
        let (d, s, msb) = (d & sz.mask(), s & sz.mask(), sz.msb());
        let carry = ((s & d) | (!r & (s | d))) & msb != 0;
        let overflow = (s ^ r) & (d ^ r) & msb != 0;
        self.set_arith_flags(r, sz, carry, overflow, extend, true);
        r
    }

    /// `d - s (- x)` at `sz`, setting the flags. `extend`: SUBX/NEGX; `set_x`: false for CMP.
    pub(crate) fn alu_sub(
        &mut self,
        d: u32,
        s: u32,
        x: u32,
        sz: Sz,
        extend: bool,
        set_x: bool,
    ) -> u32 {
        let r = d.wrapping_sub(s).wrapping_sub(x) & sz.mask();
        let (d, s, msb) = (d & sz.mask(), s & sz.mask(), sz.msb());
        let borrow = ((s & !d) | (r & !d) | (s & r)) & msb != 0;
        let overflow = (s ^ d) & (r ^ d) & msb != 0;
        self.set_arith_flags(r, sz, borrow, overflow, extend, set_x);
        r
    }

    fn set_arith_flags(
        &mut self,
        r: u32,
        sz: Sz,
        carry: bool,
        overflow: bool,
        extend: bool,
        set_x: bool,
    ) {
        let old = self.sr() & 0x1F;
        let mut ccr = if set_x { 0 } else { old & X };
        if carry {
            ccr |= C | if set_x { X } else { 0 };
        }
        if overflow {
            ccr |= V;
        }
        if r & sz.msb() != 0 {
            ccr |= N;
        }
        if r == 0 && (!extend || old & Z != 0) {
            ccr |= Z;
        }
        self.set_ccr(ccr);
    }

    fn alu(&mut self, alu: Alu, d: u32, s: u32, sz: Sz) -> u32 {
        match alu {
            Alu::Add => self.alu_add(d, s, 0, sz, false),
            Alu::Sub => self.alu_sub(d, s, 0, sz, false, true),
        }
    }

    /// ADD/SUB `<ea>,Dn`.
    pub(crate) fn op_arith<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        alu: Alu,
        sz: Sz,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, sz, false)?;
        let s = self.read_loc(bus, src, sz)?;
        let dn = ((op >> 9) & 7) as usize;
        let v = self.alu(alu, self.d[dn], s, sz);
        self.d[dn] = (self.d[dn] & !sz.mask()) | v;
        if sz == Sz::L {
            self.idle(if reg_or_imm(m, r) { 4 } else { 2 });
        }
        self.prefetch_next(bus)
    }

    /// ADD/SUB `Dn,<ea>` (memory destination).
    pub(crate) fn op_arith_mem<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        alu: Alu,
        sz: Sz,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, sz, false)?;
        let d = self.read_loc(bus, dst, sz)?;
        let s = self.d[((op >> 9) & 7) as usize];
        let v = self.alu(alu, d, s, sz);
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, sz, v)
    }

    /// ADDA/SUBA: the source (a word sign-extended) on all 32 bits of An; no flags.
    pub(crate) fn op_arith_a<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        alu: Alu,
        sz: Sz,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, sz, false)?;
        let s = self.read_loc(bus, src, sz)?;
        let s = if sz == Sz::W { sext16(s) } else { s };
        let an = ((op >> 9) & 7) as usize;
        let v = match alu {
            Alu::Add => self.an(an).wrapping_add(s),
            Alu::Sub => self.an(an).wrapping_sub(s),
        };
        self.set_an(an, v);
        self.idle(if sz == Sz::W || reg_or_imm(m, r) {
            4
        } else {
            2
        });
        self.prefetch_next(bus)
    }

    /// ADDX/SUBX `Dy,Dx` or `-(Ay),-(Ax)`.
    pub(crate) fn op_arith_x<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        alu: Alu,
        sz: Sz,
    ) -> Result<(), Fault> {
        let rx = ((op >> 9) & 7) as usize;
        let ry = (op & 7) as usize;
        let x = self.x_flag();
        if op & 8 == 0 {
            let (d, s) = (self.d[rx], self.d[ry]);
            let v = match alu {
                Alu::Add => self.alu_add(d, s, x, sz, true),
                Alu::Sub => self.alu_sub(d, s, x, sz, true, true),
            };
            self.d[rx] = (self.d[rx] & !sz.mask()) | v;
            if sz == Sz::L {
                self.idle(4);
            }
            self.prefetch_next(bus)
        } else {
            // UM Table 8-11: 18 (byte, word) / 30 (long) clocks; the two predecrements share 2 internal clocks.
            self.idle(2);
            let src = self.ea(bus, 4, ry as u16, sz, true)?;
            let s = self.read_loc(bus, src, sz)?;
            let dst = self.ea(bus, 4, rx as u16, sz, true)?;
            let d = self.read_loc(bus, dst, sz)?;
            let v = match alu {
                Alu::Add => self.alu_add(d, s, x, sz, true),
                Alu::Sub => self.alu_sub(d, s, x, sz, true, true),
            };
            self.prefetch_next(bus)?;
            self.write_loc(bus, dst, sz, v)
        }
    }

    /// The immediate operand of an `xxxI` instruction, which precedes the effective address's extension words.
    pub(crate) fn immediate<B: M68kBus>(&mut self, bus: &mut B, sz: Sz) -> Result<u32, Fault> {
        Ok(match sz {
            Sz::B => self.extension(bus)? as u32 & 0xFF,
            Sz::W => self.extension(bus)? as u32,
            Sz::L => {
                let hi = self.extension(bus)? as u32;
                hi << 16 | self.extension(bus)? as u32
            }
        })
    }

    /// ADDI/SUBI `#imm,<ea>`.
    pub(crate) fn op_arith_i<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        alu: Alu,
        sz: Sz,
    ) -> Result<(), Fault> {
        let s = self.immediate(bus, sz)?;
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, sz, false)?;
        let d = self.read_loc(bus, dst, sz)?;
        let v = self.alu(alu, d, s, sz);
        if matches!(dst, Loc::D(_)) && sz == Sz::L {
            self.idle(4);
        }
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, sz, v)
    }

    /// ADDQ/SUBQ `#1-8,<ea>`; on an address register the whole 32 bits, without flags.
    pub(crate) fn op_arith_q<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        alu: Alu,
        sz: Sz,
    ) -> Result<(), Fault> {
        let q = match (op >> 9) & 7 {
            0 => 8,
            n => n as u32,
        };
        let (m, r) = src_field(op);
        if m == 1 {
            let a = self.an(r as usize);
            let v = match alu {
                Alu::Add => a.wrapping_add(q),
                Alu::Sub => a.wrapping_sub(q),
            };
            self.set_an(r as usize, v);
            // SingleStepTests: 8 clocks for .W, 6 for .L (UM Table 8-4 gives 8 for both; the test data wins).
            self.idle(if sz == Sz::L { 2 } else { 4 });
            return self.prefetch_next(bus);
        }
        let dst = self.ea(bus, m, r, sz, false)?;
        let d = self.read_loc(bus, dst, sz)?;
        let v = self.alu(alu, d, q, sz);
        if m == 0 && sz == Sz::L {
            self.idle(4);
        }
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, sz, v)
    }

    /// CMP `<ea>,Dn`.
    pub(crate) fn op_cmp<B: M68kBus>(&mut self, bus: &mut B, op: u16, sz: Sz) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, sz, false)?;
        let s = self.read_loc(bus, src, sz)?;
        self.alu_sub(self.d[((op >> 9) & 7) as usize], s, 0, sz, false, false);
        if sz == Sz::L {
            self.idle(2);
        }
        self.prefetch_next(bus)
    }

    /// CMPA: a word source is sign-extended and compared on 32 bits.
    pub(crate) fn op_cmpa<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        sz: Sz,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, sz, false)?;
        let s = self.read_loc(bus, src, sz)?;
        let s = if sz == Sz::W { sext16(s) } else { s };
        self.alu_sub(self.an(((op >> 9) & 7) as usize), s, 0, Sz::L, false, false);
        self.idle(2);
        self.prefetch_next(bus)
    }

    /// CMPM `(Ay)+,(Ax)+`.
    pub(crate) fn op_cmpm<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        sz: Sz,
    ) -> Result<(), Fault> {
        let src = self.ea(bus, 3, op & 7, sz, false)?;
        let s = self.read_loc(bus, src, sz)?;
        let dst = self.ea(bus, 3, (op >> 9) & 7, sz, false)?;
        let d = self.read_loc(bus, dst, sz)?;
        self.alu_sub(d, s, 0, sz, false, false);
        self.prefetch_next(bus)
    }

    /// CMPI `#imm,<ea>`.
    pub(crate) fn op_cmpi<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        sz: Sz,
    ) -> Result<(), Fault> {
        let s = self.immediate(bus, sz)?;
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, sz, false)?;
        let d = self.read_loc(bus, dst, sz)?;
        self.alu_sub(d, s, 0, sz, false, false);
        if m == 0 && sz == Sz::L {
            self.idle(2);
        }
        self.prefetch_next(bus)
    }

    /// NEG (`0 - d`) and NEGX (`0 - d - X`).
    pub(crate) fn op_neg<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        sz: Sz,
        extend: bool,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, sz, false)?;
        let d = self.read_loc(bus, dst, sz)?;
        let x = if extend { self.x_flag() } else { 0 };
        let v = self.alu_sub(0, d, x, sz, extend, true);
        if m == 0 && sz == Sz::L {
            self.idle(2);
        }
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, sz, v)
    }
}
