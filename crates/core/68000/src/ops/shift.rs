//! Shifts and rotates: ASL/ASR, LSL/LSR, ROXL/ROXR, ROL/ROR, on a data register (count 1-8 or Dn modulo 64) or
//! a memory word (count 1). Flags per PRM 4 (each instruction's condition-code table). UM Table 8-8: register
//! forms take 6 + 2n (byte, word) or 8 + 2n (long) clocks; memory forms 8 + EA.

use crate::bus::M68kBus;
use crate::cpu::{Cpu, Fault};
use crate::opcodes::{Shift, Sz};
use crate::ops::move_::src_field;

impl Cpu {
    /// Shifts `v` (at `sz`) `count` times; returns the result and sets X, N, Z, V, C.
    fn shift(&mut self, kind: Shift, left: bool, v: u32, count: u32, sz: Sz) -> u32 {
        let msb = sz.msb();
        let mask = sz.mask();
        let mut v = v & mask;
        let old = self.sr() & 0x1F;
        let mut x = old & 0x10 != 0;
        let mut c = false;
        let mut overflow = false;
        for _ in 0..count {
            let out = if left { v & msb != 0 } else { v & 1 != 0 };
            v = match (kind, left) {
                (Shift::As | Shift::Ls, true) => (v << 1) & mask,
                (Shift::As, false) => (v >> 1) | (v & msb),
                (Shift::Ls, false) => v >> 1,
                (Shift::Ro, true) => ((v << 1) & mask) | out as u32,
                (Shift::Ro, false) => (v >> 1) | if out { msb } else { 0 },
                (Shift::Rox, true) => ((v << 1) & mask) | x as u32,
                (Shift::Rox, false) => (v >> 1) | if x { msb } else { 0 },
            };
            if kind == Shift::As && left && (v & msb != 0) != out {
                overflow = true;
            }
            c = out;
            if kind != Shift::Ro {
                x = out;
            }
        }
        if count == 0 && kind == Shift::Rox {
            c = x;
        }
        let mut ccr = if x { 0x10 } else { 0 };
        if v & msb != 0 {
            ccr |= 0x08;
        }
        if v == 0 {
            ccr |= 0x04;
        }
        if overflow {
            ccr |= 0x02;
        }
        if c {
            ccr |= 0x01;
        }
        self.set_sr((self.sr() & 0xFF00) | ccr);
        v
    }

    /// Register form: `1110 ccc d ss i tt rrr`.
    pub(crate) fn op_shift<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        kind: Shift,
        sz: Sz,
    ) -> Result<(), Fault> {
        let left = op & 0x100 != 0;
        let field = ((op >> 9) & 7) as u32;
        let count = if op & 0x20 != 0 {
            self.d[field as usize] & 63
        } else if field == 0 {
            8
        } else {
            field
        };
        let r = (op & 7) as usize;
        let v = self.shift(kind, left, self.d[r], count, sz);
        self.d[r] = (self.d[r] & !sz.mask()) | v;
        self.prefetch_then_idle(bus, 2 * count + if sz == Sz::L { 4 } else { 2 })
    }

    /// Memory form: one bit of a word.
    pub(crate) fn op_shift_mem<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        kind: Shift,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, Sz::W, false)?;
        let d = self.read_loc(bus, dst, Sz::W)?;
        let v = self.shift(kind, op & 0x100 != 0, d, 1, Sz::W);
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, Sz::W, v)
    }
}
