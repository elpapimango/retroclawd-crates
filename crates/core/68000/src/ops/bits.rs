//! Single-bit operations BTST, BCHG, BCLR, BSET (dynamic: bit number in Dn; static: in an extension word) and
//! TAS (PRM chapter 4). On a data register the bit number is taken modulo 32, on memory (a byte) modulo 8.

use crate::bus::M68kBus;
use crate::cpu::{Cpu, Fault};
use crate::ea::Loc;
use crate::opcodes::Sz;
use crate::ops::move_::src_field;

impl Cpu {
    /// `kind` is bits 7-6: 0 BTST, 1 BCHG, 2 BCLR, 3 BSET.
    pub(crate) fn op_bit<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        is_static: bool,
    ) -> Result<(), Fault> {
        let kind = (op >> 6) & 3;
        let bit = if is_static {
            self.extension(bus)? as u32
        } else {
            self.d[((op >> 9) & 7) as usize]
        };
        let (m, r) = src_field(op);
        let sz = if m == 0 { Sz::L } else { Sz::B };
        let dst = self.ea(bus, m, r, sz, false)?;
        let bit = if m == 0 { bit & 31 } else { bit & 7 };
        let v = self.read_loc(bus, dst, sz)?;
        let was_set = v & (1 << bit) != 0;
        let z = if was_set { 0 } else { 0x04 };
        self.set_sr((self.sr() & !0x04) | z);
        let new = match kind {
            1 => v ^ (1 << bit),
            2 => v & !(1 << bit),
            3 => v | (1 << bit),
            _ => v,
        };
        if matches!(dst, Loc::Imm(_)) {
            // BTST Dn,#imm: SingleStepTests 10 clocks.
            self.idle(2);
        } else if matches!(dst, Loc::D(_)) {
            // UM Table 8-12: on a register the operation takes 2 more clocks for bits 16-31; BCLR 2 more again.
            let high = if bit >= 16 { 2 } else { 0 };
            self.idle(match kind {
                0 => 2,
                2 => 4 + high,
                _ => 2 + high,
            });
        }
        self.prefetch_next(bus)?;
        if kind != 0 {
            self.write_loc(bus, dst, sz, new)?;
        }
        Ok(())
    }

    /// TAS: test a byte, then set its bit 7 with an indivisible read-modify-write cycle.
    pub(crate) fn op_tas<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, Sz::B, false)?;
        let v = self.read_loc(bus, dst, Sz::B)?;
        self.set_logic_flags(v, Sz::B);
        if let Loc::Mem(_) = dst {
            self.idle(2);
            self.write_loc(bus, dst, Sz::B, v | 0x80)?;
            self.prefetch_next(bus)
        } else {
            self.write_loc(bus, dst, Sz::B, v | 0x80)?;
            self.prefetch_next(bus)
        }
    }
}
