//! MULU, MULS (16 x 16 -> 32) and DIVU, DIVS (32 / 16 -> 16 quotient, 16 remainder) (PRM chapter 4).
//!
//! Clock counts depend on the operands. MULU: 38 + 2n, n = the number of 1 bits in the source; MULS: 38 + 2n,
//! n = the number of 01 or 10 pairs in the source with a 0 appended below bit 0 (UM Table 8-7 notes). DIVU and
//! DIVS follow the hardware's shift-and-subtract loop step by step (the structure UM Table 8-7 leaves as "worst
//! case 140 / 158"), checked clock for clock against SingleStepTests.

use crate::bus::M68kBus;
use crate::cpu::{Cpu, Fault};
use crate::opcodes::Sz;
use crate::ops::move_::src_field;

pub(crate) const VECTOR_ZERO_DIVIDE: u8 = 5;

/// Clocks DIVU spends after reading its operand (no prefetch), for a non-zero divisor.
fn divu_clocks(dividend: u32, divisor: u32) -> u32 {
    if dividend >> 16 >= divisor {
        return 6;
    }
    let mut clocks = 38 * 2 - 4;
    let hdivisor = divisor << 16;
    let mut rem = dividend;
    for _ in 0..15 {
        let msb = rem & 0x8000_0000 != 0;
        rem <<= 1;
        if msb {
            rem = rem.wrapping_sub(hdivisor);
        } else {
            clocks += 4;
            if rem >= hdivisor {
                rem = rem.wrapping_sub(hdivisor);
                clocks -= 2;
            }
        }
    }
    clocks
}

/// Clocks DIVS spends after reading its operand (no prefetch), for a non-zero divisor.
fn divs_clocks(dividend: i32, divisor: i16) -> u32 {
    let mut clocks = 6 * 2 - 4;
    if dividend < 0 {
        clocks += 2;
    }
    let adividend = dividend.unsigned_abs();
    let adivisor = (divisor as i32).unsigned_abs();
    if adividend >> 15 >= adivisor {
        return clocks + 4;
    }
    clocks += 55 * 2;
    if divisor >= 0 {
        if dividend >= 0 {
            clocks -= 2;
        } else {
            clocks += 2;
        }
    }
    let mut aquot = adividend / adivisor;
    for _ in 0..15 {
        if aquot & 0x8000 == 0 {
            clocks += 2;
        }
        aquot <<= 1;
    }
    clocks
}

impl Cpu {
    pub(crate) fn op_mul<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        signed: bool,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, Sz::W, false)?;
        let s = self.read_loc(bus, src, Sz::W)?;
        let dn = ((op >> 9) & 7) as usize;
        let (v, n) = if signed {
            let v = (s as u16 as i16 as i32).wrapping_mul(self.d[dn] as u16 as i16 as i32) as u32;
            (v, (((s << 1) ^ s) as u16).count_ones())
        } else {
            (s.wrapping_mul(self.d[dn] & 0xFFFF), s.count_ones())
        };
        self.d[dn] = v;
        self.set_logic_flags(v, Sz::L);
        self.idle(34 + 2 * n);
        self.prefetch_next(bus)
    }

    /// DIVU/DIVS: on overflow V is set and Dn is unchanged; a zero divisor raises vector 5.
    pub(crate) fn op_div<B: M68kBus>(
        &mut self,
        bus: &mut B,
        op: u16,
        signed: bool,
    ) -> Result<(), Fault> {
        let (m, r) = src_field(op);
        let src = self.ea(bus, m, r, Sz::W, false)?;
        let s = self.read_loc(bus, src, Sz::W)?;
        let dn = ((op >> 9) & 7) as usize;
        let dividend = self.d[dn];
        if s == 0 {
            self.idle(4);
            return Err(Fault::Exception(VECTOR_ZERO_DIVIDE));
        }
        let ccr_x = self.sr() & 0x10;
        let result = if signed {
            self.idle(divs_clocks(dividend as i32, s as u16 as i16));
            let q = (dividend as i32 as i64) / (s as u16 as i16 as i64);
            let rem = (dividend as i32 as i64) % (s as u16 as i16 as i64);
            // The hardware's early test |dividend| >> 15 >= |divisor| also refuses a quotient of -32768.
            let early =
                (dividend as i32).unsigned_abs() >> 15 >= (s as u16 as i16 as i32).unsigned_abs();
            if early || q < i16::MIN as i64 || q > i16::MAX as i64 {
                None
            } else {
                Some((q as u32, rem as u32))
            }
        } else {
            self.idle(divu_clocks(dividend, s));
            let q = dividend / s;
            if q > 0xFFFF {
                None
            } else {
                Some((q, dividend % s))
            }
        };
        match result {
            None => {
                // Overflow: V set, C cleared; N and Z as the hardware leaves them (SingleStepTests).
                let nz = self.sr() & 0x0C;
                self.set_sr((self.sr() & 0xFF00) | ccr_x | nz | 0x02);
            }
            Some((q, rem)) => {
                self.d[dn] = (rem & 0xFFFF) << 16 | (q & 0xFFFF);
                self.set_logic_flags(q, Sz::W);
            }
        }
        self.prefetch_next(bus)
    }
}
