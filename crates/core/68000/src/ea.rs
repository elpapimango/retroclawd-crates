//! Effective addresses (PRM 2.2): where an operand lives, and reading and writing it.
//!
//! Timing (UM Table 8-1): extension words come through the prefetch queue (4 clocks each); `-(An)` and the two
//! indexed modes spend 2 internal clocks computing the address, except `-(An)` as a MOVE destination.

use crate::bus::{M68kBus, Size};
use crate::cpu::{Cpu, Fault};
use crate::opcodes::Sz;

/// Where an operand lives once its effective address is computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Loc {
    D(usize),
    A(usize),
    Mem(u32),
    Imm(u32),
}

pub(crate) fn sext8(v: u32) -> u32 {
    v as u8 as i8 as i32 as u32
}

pub(crate) fn sext16(v: u32) -> u32 {
    v as u16 as i16 as i32 as u32
}

impl Cpu {
    /// Computes the effective address of field (`mode`, `reg`) for an operand of size `sz`, consuming extension
    /// words and applying `(An)+`/`-(An)` (a byte on A7 moves it by 2, PRM 2.2.4). `move_dst`: the destination of
    /// a MOVE, whose `-(An)` costs no extra clocks.
    pub(crate) fn ea<B: M68kBus>(
        &mut self,
        bus: &mut B,
        mode: u16,
        reg: u16,
        sz: Sz,
        move_dst: bool,
    ) -> Result<Loc, Fault> {
        let r = reg as usize;
        let step = if sz == Sz::B && r == 7 { 2 } else { sz.bytes() };
        Ok(match mode {
            0 => Loc::D(r),
            1 => Loc::A(r),
            2 => Loc::Mem(self.an(r)),
            3 => {
                let a = self.an(r);
                self.set_an(r, a.wrapping_add(step));
                Loc::Mem(a)
            }
            4 => {
                if !move_dst {
                    self.idle(2);
                }
                let a = self.an(r).wrapping_sub(step);
                self.set_an(r, a);
                Loc::Mem(a)
            }
            5 => {
                let d = sext16(self.extension(bus)? as u32);
                Loc::Mem(self.an(r).wrapping_add(d))
            }
            6 => {
                let base = self.an(r);
                Loc::Mem(self.indexed(bus, base)?)
            }
            7 => match reg {
                0 => Loc::Mem(sext16(self.extension(bus)? as u32)),
                1 => {
                    let hi = self.extension(bus)? as u32;
                    let lo = self.extension(bus)? as u32;
                    Loc::Mem(hi << 16 | lo)
                }
                2 => {
                    let base = self.pc.wrapping_add(2);
                    let d = sext16(self.extension(bus)? as u32);
                    Loc::Mem(base.wrapping_add(d))
                }
                3 => {
                    let base = self.pc.wrapping_add(2);
                    Loc::Mem(self.indexed(bus, base)?)
                }
                _ => match sz {
                    Sz::B => Loc::Imm(self.extension(bus)? as u32 & 0xFF),
                    Sz::W => Loc::Imm(self.extension(bus)? as u32),
                    Sz::L => {
                        let hi = self.extension(bus)? as u32;
                        let lo = self.extension(bus)? as u32;
                        Loc::Imm(hi << 16 | lo)
                    }
                },
            },
            _ => unreachable!("mode is three bits"),
        })
    }

    /// `(d8,base,Xn)`: the brief extension word (PRM 2.2.7): D/A in bit 15, register in 14-12, W/L in 11, d8 in 7-0.
    fn indexed<B: M68kBus>(&mut self, bus: &mut B, base: u32) -> Result<u32, Fault> {
        // The index is added while the queue refills (SingleStepTests): the 2 internal clocks come first.
        let ext = self.extension_after(bus, 2)? as u32;
        let r = ((ext >> 12) & 7) as usize;
        let x = if ext & 0x8000 != 0 {
            self.an(r)
        } else {
            self.d[r]
        };
        let x = if ext & 0x0800 != 0 { x } else { sext16(x) };
        Ok(base.wrapping_add(x).wrapping_add(sext8(ext)))
    }

    pub(crate) fn read_loc<B: M68kBus>(
        &mut self,
        bus: &mut B,
        loc: Loc,
        sz: Sz,
    ) -> Result<u32, Fault> {
        Ok(match loc {
            Loc::D(r) => self.d[r] & sz.mask(),
            Loc::A(r) => self.an(r) & sz.mask(),
            Loc::Imm(v) => v,
            Loc::Mem(a) => self.read_mem(bus, a, sz)?,
        })
    }

    /// Like `read_loc`, but a long in memory is read low word first (ADDX/SUBX on `-(An)`).
    pub(crate) fn read_loc_low_first<B: M68kBus>(
        &mut self,
        bus: &mut B,
        loc: Loc,
        sz: Sz,
    ) -> Result<u32, Fault> {
        match (loc, sz) {
            (Loc::Mem(a), Sz::L) => {
                let fc = self.data_fc();
                let lo = self.bus_read(bus, a.wrapping_add(2), Size::Word, fc, false)?;
                let hi = self.bus_read(bus, a, Size::Word, fc, false)?;
                Ok(hi << 16 | lo)
            }
            _ => self.read_loc(bus, loc, sz),
        }
    }

    /// Writes `v` (its low `sz` bits); a data register keeps its other bits, an address register takes all 32.
    pub(crate) fn write_loc<B: M68kBus>(
        &mut self,
        bus: &mut B,
        loc: Loc,
        sz: Sz,
        v: u32,
    ) -> Result<(), Fault> {
        match loc {
            Loc::D(r) => self.d[r] = (self.d[r] & !sz.mask()) | (v & sz.mask()),
            Loc::A(r) => self.set_an(r, v),
            Loc::Mem(a) => self.write_mem(bus, a, sz, v)?,
            Loc::Imm(_) => unreachable!("an immediate is never a destination"),
        }
        Ok(())
    }

    /// A data read of `sz` at `addr`: a long is two word cycles, high word first.
    pub(crate) fn read_mem<B: M68kBus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        sz: Sz,
    ) -> Result<u32, Fault> {
        let fc = self.data_fc();
        Ok(match sz {
            Sz::B => self.bus_read(bus, addr, Size::Byte, fc, false)?,
            Sz::W => self.bus_read(bus, addr, Size::Word, fc, false)?,
            Sz::L => {
                let hi = self.bus_read(bus, addr, Size::Word, fc, false)?;
                let lo = self.bus_read(bus, addr.wrapping_add(2), Size::Word, fc, false)?;
                hi << 16 | lo
            }
        })
    }

    /// A data write of `sz` at `addr`: a long is two word cycles, high word first.
    pub(crate) fn write_mem<B: M68kBus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        sz: Sz,
        v: u32,
    ) -> Result<(), Fault> {
        let fc = self.data_fc();
        match sz {
            Sz::B => self.bus_write(bus, addr, Size::Byte, v & 0xFF, fc),
            Sz::W => self.bus_write(bus, addr, Size::Word, v & 0xFFFF, fc),
            Sz::L => {
                self.bus_write(bus, addr, Size::Word, v >> 16, fc)?;
                self.bus_write(bus, addr.wrapping_add(2), Size::Word, v & 0xFFFF, fc)
            }
        }
    }

    /// The write that ends a read-modify-write instruction: a long goes out low word first (SingleStepTests).
    pub(crate) fn write_loc_rmw<B: M68kBus>(
        &mut self,
        bus: &mut B,
        loc: Loc,
        sz: Sz,
        v: u32,
    ) -> Result<(), Fault> {
        match (loc, sz) {
            (Loc::Mem(a), Sz::L) => {
                let fc = self.data_fc();
                self.bus_write(bus, a.wrapping_add(2), Size::Word, v & 0xFFFF, fc)?;
                self.bus_write(bus, a, Size::Word, v >> 16, fc)
            }
            _ => self.write_loc(bus, loc, sz, v),
        }
    }

    /// N and Z from `v` at `sz`; V and C cleared; X kept (MOVE, TST, CLR and the logical operations).
    pub(crate) fn set_logic_flags(&mut self, v: u32, sz: Sz) {
        let v = v & sz.mask();
        let mut ccr = self.sr() & 0x10;
        if v & sz.msb() != 0 {
            ccr |= 0x08;
        }
        if v == 0 {
            ccr |= 0x04;
        }
        self.set_sr((self.sr() & 0xFF00) | ccr);
    }
}
