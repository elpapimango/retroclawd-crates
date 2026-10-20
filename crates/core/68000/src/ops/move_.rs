//! Data movement: MOVE, MOVEA, MOVEQ, LEA, PEA, CLR, TST, EXG, SWAP, EXT (PRM chapter 4).

use crate::bus::M68kBus;
use crate::cpu::{Cpu, Fault};
use crate::ea::{Loc, sext8, sext16};
use crate::opcodes::Sz;

/// The source field (bits 5-0) of an opcode.
pub(crate) fn src_field(op: u16) -> (u16, u16) {
    ((op >> 3) & 7, op & 7)
}

/// Whether an effective-address field is one of the two indexed modes.
fn indexed(mode: u16, reg: u16) -> bool {
    mode == 6 || (mode == 7 && reg == 3)
}

impl Cpu {
    pub(crate) fn op_move<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        sz: Sz,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, sz, false)?;
        let v = self.read_loc(bus, src, sz)?;
        let dst = self.ea(bus, (op >> 6) & 7, (op >> 9) & 7, sz, true)?;
        self.set_logic_flags(v, sz);
        self.write_loc(bus, dst, sz, v)?;
        self.prefetch_next(bus)
    }

    pub(crate) fn op_movea<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        sz: Sz,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, sz, false)?;
        let v = self.read_loc(bus, src, sz)?;
        let v = if sz == Sz::W { sext16(v) } else { v };
        self.set_an(((op >> 9) & 7) as usize, v);
        self.prefetch_next(bus)
    }

    pub(crate) fn op_moveq<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let v = sext8(op as u32);
        self.d[((op >> 9) & 7) as usize] = v;
        self.set_logic_flags(v, Sz::L);
        self.prefetch_next(bus)
    }

    /// The control address LEA and PEA compute; the indexed modes take 2 clocks more here than for an operand.
    fn control_address<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<u32, Fault> {
        let (m, r) = src_field(op);
        let Loc::Mem(a) = self.ea(bus, m, r, Sz::L, false)? else {
            unreachable!("decode only accepts control modes")
        };
        if indexed(m, r) {
            self.idle(2);
        }
        Ok(a)
    }

    pub(crate) fn op_lea<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let a = self.control_address(bus, op)?;
        self.set_an(((op >> 9) & 7) as usize, a);
        self.prefetch_next(bus)
    }

    pub(crate) fn op_pea<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let a = self.control_address(bus, op)?;
        self.push_long(bus, a)?;
        self.prefetch_next(bus)
    }

    pub(crate) fn push_long<B: M68kBus>(&mut self, bus: &mut B, v: u32) -> Result<(), Fault> {
        let sp = self.a7().wrapping_sub(4);
        self.set_a7(sp);
        self.write_mem(bus, sp, Sz::L, v)
    }

    /// CLR reads its memory operand before writing zero (UM Table 8-9 counts the read).
    pub(crate) fn op_clr<B: M68kBus>(&mut self, bus: &mut B, op: u16, sz: Sz) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, sz, false)?;
        if let Loc::Mem(_) = dst {
            self.read_loc(bus, dst, sz)?;
        } else if sz == Sz::L {
            self.idle(2);
        }
        self.set_logic_flags(0, sz);
        self.write_loc(bus, dst, sz, 0)?;
        self.prefetch_next(bus)
    }

    pub(crate) fn op_tst<B: M68kBus>(&mut self, bus: &mut B, op: u16, sz: Sz) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, sz, false)?;
        let v = self.read_loc(bus, src, sz)?;
        self.set_logic_flags(v, sz);
        self.prefetch_next(bus)
    }

    /// EXG: opmode 01000 two data registers, 01001 two address registers, 10001 Dx and Ay.
    pub(crate) fn op_exg<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let x = ((op >> 9) & 7) as usize;
        let y = (op & 7) as usize;
        match (op >> 3) & 0x1F {
            0b01000 => self.d.swap(x, y),
            0b01001 => {
                let (ax, ay) = (self.an(x), self.an(y));
                self.set_an(x, ay);
                self.set_an(y, ax);
            }
            _ => {
                let (dx, ay) = (self.d[x], self.an(y));
                self.d[x] = ay;
                self.set_an(y, dx);
            }
        }
        self.idle(2);
        self.prefetch_next(bus)
    }

    pub(crate) fn op_swap<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let r = (op & 7) as usize;
        self.d[r] = self.d[r].rotate_left(16);
        self.set_logic_flags(self.d[r], Sz::L);
        self.prefetch_next(bus)
    }

    /// EXT.W extends the low byte into the low word, EXT.L the low word into the long.
    pub(crate) fn op_ext<B: M68kBus>(&mut self, bus: &mut B, op: u16, sz: Sz) -> Result<(), Fault> {
        let r = (op & 7) as usize;
        if sz == Sz::W {
            let v = sext8(self.d[r]) & 0xFFFF;
            self.d[r] = (self.d[r] & 0xFFFF_0000) | v;
            self.set_logic_flags(v, Sz::W);
        } else {
            self.d[r] = sext16(self.d[r]);
            self.set_logic_flags(self.d[r], Sz::L);
        }
        self.prefetch_next(bus)
    }
}
