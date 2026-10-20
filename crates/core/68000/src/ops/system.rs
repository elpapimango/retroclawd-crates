//! Status register and system instructions: ANDI/ORI/EORI to CCR and SR, MOVE to CCR/SR, MOVE from SR, MOVE USP,
//! RESET; and the multi-register moves MOVEM and MOVEP (PRM chapters 4 and 6). Privileged instructions in user
//! mode raise a privilege violation (vector 8).

use crate::bus::{M68kBus, Size};
use crate::cpu::{Cpu, Fault, SR_S};
use crate::ea::{Loc, sext16};
use crate::opcodes::{Logic, Sz};
use crate::ops::move_::src_field;

pub(crate) const VECTOR_PRIVILEGE: u8 = 8;

impl Cpu {
    pub(crate) fn require_supervisor(&self) -> Result<(), Fault> {
        if self.sr() & SR_S != 0 {
            Ok(())
        } else {
            Err(Fault::Exception(VECTOR_PRIVILEGE))
        }
    }

    /// Ends an instruction that changed SR or CCR: the queue is refetched from the next instruction.
    fn refetch_after_sr<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        self.pc = self.pc.wrapping_add(2);
        self.fill_prefetch(bus)
    }

    /// ANDI/ORI/EORI #imm to CCR (`to_sr` false) or SR (privileged). UM Table 8-12: 20 clocks.
    pub(crate) fn op_logic_sr<B: M68kBus>(
        &mut self,
        bus: &mut B,
        logic: Logic,
        to_sr: bool,
    ) -> Result<(), Fault> {
        if to_sr {
            self.require_supervisor()?;
        }
        let imm = self.extension(bus)?;
        let mask = if to_sr { 0xFFFF } else { 0x00FF };
        let old = self.sr();
        let v = match logic {
            Logic::And => old & (imm | !mask),
            Logic::Or => old | (imm & mask),
            Logic::Eor => old ^ (imm & mask),
        };
        self.set_sr(v);
        self.idle(8);
        self.refetch_after_sr(bus)
    }

    /// MOVE <ea>,CCR (word source, low byte used) and MOVE <ea>,SR (privileged).
    pub(crate) fn op_move_to_sr<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        to_sr: bool,
    ) -> Result<(), Fault> {
        if to_sr {
            self.require_supervisor()?;
        }
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, Sz::W, false)?;
        let v = self.read_loc(bus, src, Sz::W)? as u16;
        if to_sr {
            self.set_sr(v);
        } else {
            self.set_sr((self.sr() & 0xFF00) | (v & 0x1F));
        }
        self.idle(4);
        self.refetch_after_sr(bus)
    }

    /// MOVE SR,<ea>: not privileged on the 68000; a memory destination is read first.
    pub(crate) fn op_move_from_sr<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let dst = self.ea(bus, m, r, Sz::W, false)?;
        if let Loc::Mem(_) = dst {
            self.read_loc(bus, dst, Sz::W)?;
        } else {
            self.idle(2);
        }
        let sr = self.sr() as u32;
        self.prefetch_next(bus)?;
        self.write_loc(bus, dst, Sz::W, sr)
    }

    /// MOVE An,USP (`0x4E60`) and MOVE USP,An (`0x4E68`); privileged.
    pub(crate) fn op_move_usp<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        self.require_supervisor()?;
        let r = (op & 7) as usize;
        if op & 8 == 0 {
            self.usp = self.an(r);
        } else {
            let usp = self.usp;
            self.set_an(r, usp);
        }
        self.prefetch_next(bus)
    }

    /// RESET: asserts the RESET line for 124 clocks (UM 6.3.1; Table 8-12: 132 clocks in all).
    pub(crate) fn op_reset<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        self.require_supervisor()?;
        self.idle(128);
        bus.reset_devices();
        self.prefetch_next(bus)
    }

    /// MOVEM registers to memory. With `-(An)` the mask is reversed (bit 0 = A7) and registers are stored from A7
    /// down; An itself is stored with its value before the instruction.
    pub(crate) fn op_movem_to_mem<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        sz: Sz,
    ) -> Result<(), Fault> {
        let mask = self.extension(bus)?;
        let (m, r) = src_field(op);
        if m == 4 {
            let an = r as usize;
            let initial = self.an(an);
            let mut addr = initial;
            for i in 0..16 {
                if mask & (1 << i) == 0 {
                    continue;
                }
                let reg = 15 - i;
                let v = if reg >= 8 {
                    self.an(reg - 8)
                } else {
                    self.d[reg]
                };
                addr = addr.wrapping_sub(sz.bytes());
                self.write_mem(bus, addr, sz, v)?;
            }
            self.set_an(an, addr);
            return self.prefetch_next(bus);
        }
        let Loc::Mem(mut addr) = self.ea(bus, m, r, sz, false)? else {
            unreachable!("decode accepts control-alterable modes and -(An) only")
        };
        for reg in 0..16 {
            if mask & (1 << reg) == 0 {
                continue;
            }
            let v = if reg >= 8 {
                self.an(reg - 8)
            } else {
                self.d[reg]
            };
            self.write_mem(bus, addr, sz, v)?;
            addr = addr.wrapping_add(sz.bytes());
        }
        self.prefetch_next(bus)
    }

    /// MOVEM memory to registers; words are sign-extended to 32 bits, also into data registers. The 68000 reads one
    /// word more than it uses (UM Table 8-10: 12 + 4n / 12 + 8n clocks).
    pub(crate) fn op_movem_to_reg<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        sz: Sz,
    ) -> Result<(), Fault> {
        let mask = self.extension(bus)?;
        let (m, r) = src_field(op);
        let mut addr = if m == 3 {
            self.an(r as usize)
        } else {
            let Loc::Mem(a) = self.ea(bus, m, r, sz, false)? else {
                unreachable!("decode accepts control modes and (An)+ only")
            };
            a
        };
        for reg in 0..16 {
            if mask & (1 << reg) == 0 {
                continue;
            }
            let v = self.read_mem(bus, addr, sz)?;
            let v = if sz == Sz::W { sext16(v) } else { v };
            if reg >= 8 {
                self.set_an(reg - 8, v);
            } else {
                self.d[reg] = v;
            }
            addr = addr.wrapping_add(sz.bytes());
        }
        let fc = self.data_fc();
        self.bus_read(bus, addr, Size::Word, fc, false)?;
        if m == 3 {
            self.set_an(r as usize, addr);
        }
        self.prefetch_next(bus)
    }

    /// MOVEP: bytes at d16(An), +2, +4, +6 (high byte first). Opmode 100/101 memory to Dn (word/long), 110/111
    /// Dn to memory.
    pub(crate) fn op_movep<B: M68kBus>(&mut self, bus: &mut B, op: u16) -> Result<(), Fault> {
        let d = sext16(self.extension(bus)? as u32);
        let mut addr = self.an((op & 7) as usize).wrapping_add(d);
        let dn = ((op >> 9) & 7) as usize;
        let n = if op & 0x40 != 0 { 4 } else { 2 };
        let fc = self.data_fc();
        if op & 0x80 == 0 {
            let mut v = 0u32;
            for _ in 0..n {
                v = v << 8 | self.bus_read(bus, addr, Size::Byte, fc, false)?;
                addr = addr.wrapping_add(2);
            }
            let mask = if n == 4 { 0xFFFF_FFFF } else { 0xFFFF };
            self.d[dn] = (self.d[dn] & !mask) | v;
        } else {
            for i in (0..n).rev() {
                self.bus_write(bus, addr, Size::Byte, self.d[dn] >> (8 * i), fc)?;
                addr = addr.wrapping_add(2);
            }
        }
        self.prefetch_next(bus)
    }
}
