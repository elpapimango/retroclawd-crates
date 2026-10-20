//! Program control: Bcc/BRA/BSR, DBcc, Scc, JMP, JSR, RTS, RTR, RTE, LINK, UNLK (PRM chapter 4).
//!
//! A branch or jump displacement that is already in the prefetch queue is used from there without a new bus
//! cycle; the target is then fetched into an empty queue (two reads). Clock counts: UM Tables 8-9 and 8-10.

use crate::bus::{M68kBus, Size};
use crate::cpu::{Cpu, Fault};
use crate::ea::{Loc, sext8, sext16};
use crate::opcodes::Sz;
use crate::ops::move_::src_field;

impl Cpu {
    /// Condition `cc` (PRM Table 3-19) against the current flags.
    pub(crate) fn condition(&self, cc: u16) -> bool {
        let sr = self.sr();
        let (n, z, v, c) = (sr & 8 != 0, sr & 4 != 0, sr & 2 != 0, sr & 1 != 0);
        match cc & 0xF {
            0 => true,
            1 => false,
            2 => !c && !z,
            3 => c || z,
            4 => !c,
            5 => c,
            6 => !z,
            7 => z,
            8 => !v,
            9 => v,
            10 => !n,
            11 => n,
            12 => n == v,
            13 => n != v,
            14 => !z && n == v,
            _ => z || n != v,
        }
    }

    /// An address error for an odd jump target, raised before the instruction stacks anything.
    pub(crate) fn check_target(&self, target: u32) -> Result<(), Fault> {
        if target & 1 == 0 {
            Ok(())
        } else {
            Err(Fault::Address {
                addr: target,
                write: false,
                fc: self.program_fc(),
                instruction: true,
            })
        }
    }

    /// Continues at `target` with an empty prefetch queue.
    pub(crate) fn jump_to<B: M68kBus>(&mut self, bus: &mut B, target: u32) -> Result<(), Fault> {
        self.check_target(target)?;
        self.pc = target;
        self.fill_prefetch(bus)
    }

    /// Bcc and BRA; condition 1 (false) is BSR.
    pub(crate) fn op_bcc<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let cc = (op >> 8) & 0xF;
        let byte = op & 0xFF != 0;
        let disp = if byte {
            sext8(op as u32)
        } else {
            sext16(self.prefetch()[1] as u32)
        };
        let target = self.pc.wrapping_add(2).wrapping_add(disp);
        if cc == 1 {
            let ret = self.pc.wrapping_add(if byte { 2 } else { 4 });
            self.idle(2);
            self.push_long(bus, ret)?;
            return self.jump_to(bus, target);
        }
        if self.condition(cc) {
            self.idle(2);
            return self.jump_to(bus, target);
        }
        self.idle(4);
        if !byte {
            self.extension(bus)?;
        }
        self.prefetch_next(bus)
    }

    /// DBcc Dn,disp: unless the condition holds, Dn.W is decremented and the branch taken until it reaches -1.
    pub(crate) fn op_dbcc<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let target = self
            .pc
            .wrapping_add(2)
            .wrapping_add(sext16(self.prefetch()[1] as u32));
        if self.condition((op >> 8) & 0xF) {
            self.idle(4);
            self.extension(bus)?;
            return self.prefetch_next(bus);
        }
        let r = (op & 7) as usize;
        let count = (self.d[r] as u16).wrapping_sub(1);
        self.d[r] = (self.d[r] & 0xFFFF_0000) | count as u32;
        self.idle(2);
        if count != 0xFFFF {
            return self.jump_to(bus, target);
        }
        // UM Table 8-9: 14(3/0) when the count expires: the branch target is fetched and discarded.
        let fc = self.program_fc();
        self.bus_read(bus, target, Size::Word, fc, true)?;
        self.extension(bus)?;
        self.prefetch_next(bus)
    }

    /// Scc: $FF when the condition holds, else $00; a memory operand is read first.
    pub(crate) fn op_scc<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, Sz::B, false)?;
        let set = self.condition((op >> 8) & 0xF);
        if let Loc::Mem(_) = dst {
            self.read_loc(bus, dst, Sz::B)?;
        } else if set {
            self.idle(2);
        }
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, Sz::B, if set { 0xFF } else { 0 })
    }

    /// The target of JMP/JSR: the first extension word comes from the queue without a bus cycle. Returns the
    /// target and the address of the next instruction.
    fn jump_target<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(u32, u32), Fault> {
        let (m, r) = src_field(op);
        let ext = self.prefetch()[1] as u32;
        let next = self.pc.wrapping_add(2);
        Ok(match (m, r) {
            (2, _) => (self.an(r as usize), next),
            (5, _) => {
                self.idle(2);
                (
                    self.an(r as usize).wrapping_add(sext16(ext)),
                    next.wrapping_add(2),
                )
            }
            (6, _) => {
                self.idle(6);
                (
                    self.index_target(self.an(r as usize), ext),
                    next.wrapping_add(2),
                )
            }
            (7, 0) => {
                self.idle(2);
                (sext16(ext), next.wrapping_add(2))
            }
            (7, 1) => {
                let fc = self.program_fc();
                let lo = self.bus_read(bus, self.pc.wrapping_add(4), Size::Word, fc, true)?;
                (ext << 16 | lo, next.wrapping_add(4))
            }
            (7, 2) => {
                self.idle(2);
                (next.wrapping_add(sext16(ext)), next.wrapping_add(2))
            }
            _ => {
                self.idle(6);
                (self.index_target(next, ext), next.wrapping_add(2))
            }
        })
    }

    /// `base + Xn + d8` from a brief extension word (PRM 2.2.7).
    fn index_target(&self, base: u32, ext: u32) -> u32 {
        let r = ((ext >> 12) & 7) as usize;
        let x = if ext & 0x8000 != 0 {
            self.an(r)
        } else {
            self.d[r]
        };
        let x = if ext & 0x0800 != 0 { x } else { sext16(x) };
        base.wrapping_add(x).wrapping_add(sext8(ext))
    }

    pub(crate) fn op_jmp<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let (target, _) = self.jump_target(bus, op)?;
        self.jump_to(bus, target)
    }

    pub(crate) fn op_jsr<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let (target, ret) = self.jump_target(bus, op)?;
        self.check_target(target)?;
        self.push_long(bus, ret)?;
        self.jump_to(bus, target)
    }

    pub(crate) fn pop_word<B: M68kBus>(&mut self, bus: &mut B) -> Result<u32, Fault> {
        let sp = self.a7();
        let v = self.read_mem(bus, sp, Sz::W)?;
        self.set_a7(sp.wrapping_add(2));
        Ok(v)
    }

    pub(crate) fn pop_long<B: M68kBus>(&mut self, bus: &mut B) -> Result<u32, Fault> {
        let sp = self.a7();
        let v = self.read_mem(bus, sp, Sz::L)?;
        self.set_a7(sp.wrapping_add(4));
        Ok(v)
    }

    pub(crate) fn op_rts<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        let target = self.pop_long(bus)?;
        self.jump_to(bus, target)
    }

    /// RTR: the condition codes, then the PC, from the stack.
    pub(crate) fn op_rtr<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        let ccr = self.pop_word(bus)? as u16;
        let target = self.pop_long(bus)?;
        self.set_sr((self.sr() & 0xFF00) | (ccr & 0x1F));
        self.jump_to(bus, target)
    }

    /// RTE: SR and PC from the supervisor stack, then SR takes effect (and may switch to the user stack).
    pub(crate) fn op_rte<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        let sr = self.pop_word(bus)? as u16;
        let target = self.pop_long(bus)?;
        self.set_sr(sr);
        self.jump_to(bus, target)
    }

    /// LINK An,#d16: push An, An = SP, SP += d16.
    pub(crate) fn op_link<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let r = (op & 7) as usize;
        let d = sext16(self.extension(bus)? as u32);
        let value = self.an(r);
        let sp = self.a7().wrapping_sub(4);
        self.set_a7(sp);
        self.write_mem(bus, sp, Sz::L, if r == 7 { sp } else { value })?;
        self.set_an(r, sp);
        self.set_a7(sp.wrapping_add(d));
        self.prefetch_next(bus)
    }

    /// UNLK An: SP = An, then An is popped.
    pub(crate) fn op_unlk<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let r = (op & 7) as usize;
        let sp = self.an(r);
        let v = self.read_mem(bus, sp, Sz::L)?;
        self.set_a7(sp.wrapping_add(4));
        self.set_an(r, v);
        self.prefetch_next(bus)
    }
}
