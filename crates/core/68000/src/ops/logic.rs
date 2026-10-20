//! Logical operations: AND, OR, EOR (register, memory and immediate forms) and NOT (PRM chapter 4).

use crate::bus::M68kBus;
use crate::cpu::{Cpu, Fault};
use crate::ea::Loc;
use crate::opcodes::{Logic, Sz};
use crate::ops::move_::src_field;

fn apply(logic: Logic, d: u32, s: u32) -> u32 {
    match logic {
        Logic::And => d & s,
        Logic::Or => d | s,
        Logic::Eor => d ^ s,
    }
}

impl Cpu {
    /// AND/OR `<ea>,Dn`.
    pub(crate) fn op_logic<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        logic: Logic,
        sz: Sz,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, sz, false)?;
        let s = self.read_loc(bus, src, sz)?;
        let dn = ((op >> 9) & 7) as usize;
        let v = apply(logic, self.d[dn], s) & sz.mask();
        self.d[dn] = (self.d[dn] & !sz.mask()) | v;
        self.set_logic_flags(v, sz);
        if sz == Sz::L {
            self.idle(if m == 0 || (m == 7 && r == 4) { 4 } else { 2 });
        }
        self.prefetch_next(bus)
    }

    /// AND/OR/EOR `Dn,<ea>`; EOR's destination may be a data register.
    pub(crate) fn op_logic_mem<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        logic: Logic,
        sz: Sz,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, sz, false)?;
        let d = self.read_loc(bus, dst, sz)?;
        let v = apply(logic, d, self.d[((op >> 9) & 7) as usize]) & sz.mask();
        self.set_logic_flags(v, sz);
        if m == 0 && sz == Sz::L {
            self.idle(4);
        }
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, sz, v)
    }

    /// ANDI/ORI/EORI `#imm,<ea>`.
    pub(crate) fn op_logic_i<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        logic: Logic,
        sz: Sz,
    ) -> Result<(), Fault> {
        let s = self.immediate(bus, sz)?;
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, sz, false)?;
        let d = self.read_loc(bus, dst, sz)?;
        let v = apply(logic, d, s) & sz.mask();
        self.set_logic_flags(v, sz);
        if matches!(dst, Loc::D(_)) && sz == Sz::L {
            self.idle(4);
        }
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, sz, v)
    }

    pub(crate) fn op_not<B: M68kBus>(&mut self, bus: &mut B, op: u16, sz: Sz) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, sz, false)?;
        let v = !self.read_loc(bus, dst, sz)? & sz.mask();
        self.set_logic_flags(v, sz);
        if m == 0 && sz == Sz::L {
            self.idle(2);
        }
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, sz, v)
    }
}
