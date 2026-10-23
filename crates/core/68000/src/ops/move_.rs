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
        let (dm, dr) = ((op >> 6) & 7, (op >> 9) & 7);
        self.set_logic_flags(v, sz);
        // The destination's bus order around a fault, as SingleStepTests records it.
        match (dm, dr) {
            // (An)+: An moves only after the write succeeds.
            (3, _) => {
                let a = self.an(dr as usize);
                self.write_mem(bus, a, sz, v)?;
                let step = if sz == Sz::B && dr == 7 {
                    2
                } else {
                    sz.bytes()
                };
                self.set_an(dr as usize, a.wrapping_add(step));
                self.prefetch_next(bus)
            }
            // -(An): the next word is prefetched first; a long then writes its low word, then the high one.
            (4, _) if sz != Sz::L => {
                let step = if sz == Sz::B && dr == 7 {
                    2
                } else {
                    sz.bytes()
                };
                let addr = self.an(dr as usize).wrapping_sub(step);
                self.prefetch_next(bus)?;
                self.set_an(dr as usize, addr);
                self.write_mem(bus, addr, sz, v)
            }
            (4, _) => {
                let addr = self.an(dr as usize).wrapping_sub(4);
                self.prefetch_next(bus)?;
                self.set_an(dr as usize, addr.wrapping_add(2));
                self.write_mem(bus, addr.wrapping_add(2), Sz::W, v & 0xFFFF)?;
                self.write_mem(bus, addr, Sz::W, v >> 16)?;
                self.set_an(dr as usize, addr);
                Ok(())
            }
            // (xxx).L after a memory source (not immediate): the low address word is used from the queue and the queue refilled
            // after the write.
            (7, 1) if m >= 2 && !(m == 7 && r == 4) => {
                let hi = self.extension(bus)? as u32;
                let addr = hi << 16 | self.prefetch[1] as u32;
                self.write_mem(bus, addr, sz, v)?;
                self.extension(bus)?;
                self.prefetch_next(bus)
            }
            _ => {
                let dst = self.ea(bus, dm, dr, sz, true)?;
                self.write_loc(bus, dst, sz, v)?;
                self.prefetch_next(bus)
            }
        }
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
        // The absolute modes push before the final fetch; the others after it (SingleStepTests).
        if (op >> 3) & 7 == 7 && op & 7 <= 1 {
            self.push_long(bus, a)?;
            return self.prefetch_next(bus);
        }
        self.prefetch_next(bus)?;
        self.push_long(bus, a)
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
            self.set_logic_flags(0, sz);
            self.prefetch_next(bus)?;
            return self.write_loc_rmw(bus, dst, sz, 0);
        }
        self.set_logic_flags(0, sz);
        self.write_loc(bus, dst, sz, 0)?;
        self.prefetch_then_idle(bus, if sz == Sz::L { 2 } else { 0 })
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
        self.prefetch_then_idle(bus, 2)
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
