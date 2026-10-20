//! Binary-coded decimal ABCD, SBCD, NBCD; and the instructions that trap: CHK, TRAP, TRAPV (PRM chapter 4).
//! The BCD flags N and V, undefined in PRM, follow the hardware as SingleStepTests records it.

use crate::bus::M68kBus;
use crate::cpu::{Cpu, Fault};
use crate::opcodes::Sz;
use crate::ops::move_::src_field;

pub(crate) const VECTOR_CHK: u8 = 6;
pub(crate) const VECTOR_TRAPV: u8 = 7;
pub(crate) const VECTOR_TRAP0: u8 = 32;

impl Cpu {
    fn x(&self) -> u32 {
        ((self.sr() >> 4) & 1) as u32
    }

    /// Sets X, N, Z (only cleared), V, C after a BCD operation.
    fn set_bcd_flags(&mut self, result: u32, carry: bool, overflow: bool) {
        let old = self.sr() & 0x1F;
        let mut ccr = 0;
        if carry {
            ccr |= 0x11;
        }
        if result & 0x80 != 0 {
            ccr |= 0x08;
        }
        if result & 0xFF == 0 && old & 0x04 != 0 {
            ccr |= 0x04;
        }
        if overflow {
            ccr |= 0x02;
        }
        self.set_sr((self.sr() & 0xFF00) | ccr);
    }

    /// `dst + src + X` in BCD, digit by digit as the hardware corrects it (also for digits above 9): a digit
    /// gets 6 added when it carried in binary or exceeds 9.
    fn bcd_add(&mut self, dst: u32, src: u32) -> u32 {
        let ss = dst + src + self.x();
        let bc = ((dst & src) | (!ss & dst) | (!ss & src)) & 0x88;
        let dc = (((ss + 0x66) ^ ss) & 0x110) >> 1;
        let corf = (bc | dc) - ((bc | dc) >> 2);
        let rr = ss + corf;
        let carry = (bc | (ss & !rr)) & 0x80 != 0;
        let overflow = !ss & rr & 0x80 != 0;
        self.set_bcd_flags(rr, carry, overflow);
        rr & 0xFF
    }

    /// `dst - src - X` in BCD: a digit that borrowed in binary gets 6 subtracted.
    fn bcd_sub(&mut self, dst: u32, src: u32) -> u32 {
        let dd = dst.wrapping_sub(src).wrapping_sub(self.x());
        let bc = ((!dst & src) | (dd & !dst) | (dd & src)) & 0x88;
        let corf = bc - (bc >> 2);
        let rr = dd.wrapping_sub(corf);
        let carry = (bc | (!dd & rr)) & 0x80 != 0;
        let overflow = dd & !rr & 0x80 != 0;
        self.set_bcd_flags(rr, carry, overflow);
        rr & 0xFF
    }

    /// ABCD/SBCD `Dy,Dx` or `-(Ay),-(Ax)`.
    pub(crate) fn op_bcd<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        add: bool,
    ) -> Result<(), Fault> {
        let rx = ((op >> 9) & 7) as usize;
        let ry = (op & 7) as usize;
        if op & 8 == 0 {
            let (d, s) = (self.d[rx] & 0xFF, self.d[ry] & 0xFF);
            let v = if add {
                self.bcd_add(d, s)
            } else {
                self.bcd_sub(d, s)
            };
            self.d[rx] = (self.d[rx] & !0xFF) | v;
            self.idle(2);
            self.prefetch_next(bus)
        } else {
            self.idle(2);
            let src = self.ea(bus, 4, ry as u16, Sz::B, true)?;
            let s = self.read_loc(bus, src, Sz::B)?;
            let dst = self.ea(bus, 4, rx as u16, Sz::B, true)?;
            let d = self.read_loc(bus, dst, Sz::B)?;
            let v = if add {
                self.bcd_add(d, s)
            } else {
                self.bcd_sub(d, s)
            };
            self.prefetch_next(bus)?;
            self.write_loc(bus, dst, Sz::B, v)
        }
    }

    pub(crate) fn op_nbcd<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, Sz::B, false)?;
        let d = self.read_loc(bus, dst, Sz::B)?;
        let v = self.bcd_sub(0, d);
        if m == 0 {
            self.idle(2);
        }
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, Sz::B, v)
    }

    /// CHK <ea>,Dn: traps (vector 6) when Dn.W < 0 (N set) or Dn.W > <ea> (N cleared).
    pub(crate) fn op_chk<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, Sz::W, false)?;
        let bound = self.read_loc(bus, src, Sz::W)? as u16 as i16;
        let v = self.d[((op >> 9) & 7) as usize] as u16 as i16;
        let ccr = self.sr() & 0x10;
        if v < 0 {
            self.set_sr((self.sr() & 0xFF00) | ccr | 0x08);
            self.idle(4);
            return Err(Fault::Exception(VECTOR_CHK));
        }
        if v > bound {
            self.set_sr((self.sr() & 0xFF00) | ccr);
            self.idle(4);
            return Err(Fault::Exception(VECTOR_CHK));
        }
        // No trap: Z from Dn.W, V and C cleared, N kept (SingleStepTests; PRM leaves Z, V, C undefined).
        let z = if v == 0 { 0x04 } else { 0 };
        self.set_sr((self.sr() & 0xFFF8 & !0x04) | z);
        self.idle(6);
        self.prefetch_next(bus)
    }

    pub(crate) fn op_trap(&mut self, op: u16) -> Result<(), Fault> {
        Err(Fault::Exception(VECTOR_TRAP0 + (op & 0xF) as u8))
    }

    pub(crate) fn op_trapv<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        if self.sr() & 0x02 != 0 {
            return Err(Fault::Exception(VECTOR_TRAPV));
        }
        self.prefetch_next(bus)
    }
}
