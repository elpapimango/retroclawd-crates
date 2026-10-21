//! Full NMOS 6502 opcode dispatch: all documented opcodes, all addressing
//! modes, and the illegal/undocumented opcodes real C64 software (many
//! demos/cracks, some games) relies on -- in scope from day one per this
//! crate's own plan (`TODO.md`'s "Commodore 64 support" section), unlike
//! the Z80 core's own undocumented-flag-bit work which was staged later.
//!
//! Single flat 256-entry `match` rather than a table-of-function-pointers
//! or a `(mnemonic, addressing mode)` data table -- the 6502's dispatch is
//! genuinely simpler than the Z80's (one byte, no CB/ED/DD/FD-style prefix
//! decoding), so there's no shared-decode-logic reason to indirect through
//! a table the way `z80::opcodes` does for its prefixed forms.
//!
//! Epistemic tiers, matching this project's own convention (see
//! `CLAUDE.md`'s AY-3-8910 entry for the "commonly documented, not
//! attributed to one source" tier):
//! - Documented opcodes' semantics/addressing/cycle counts: extremely
//!   well-established, near-universal agreement across every 6502
//!   reference.
//! - Stable illegal opcodes (SLO/RLA/SRE/RRA/DCP/ISC/SAX/LAX/ANC/ALR/
//!   ARR/SBX, illegal NOPs, JAM) and NMOS decimal arithmetic are
//!   cross-checked by the external Tom Seddon suite in `tests/functional.rs`.
//!   That suite exposed and now guards the ADC partial-adjustment N/V rule
//!   and decimal ARR's nibble corrections.
//! - The chip-dependent unstable group (SHY/SHX/SHA/TAS/LAS/XAA) still has
//!   no single universally-correct result. It uses the deterministic common
//!   formula documented in `CREDITS.md`; do not claim hardware-universal
//!   precision for those opcodes.

use crate::Bus;
use crate::cpu::{Cpu, FLAG_B, FLAG_C, FLAG_D, FLAG_N, FLAG_U, FLAG_V, FLAG_Z};

impl Cpu {
    // ---- Addressing-mode helpers -----------------------------------
    // Each consumes however many operand bytes the mode needs (via
    // `self.fetch`) and returns the effective address, plus a
    // page-crossed flag where the caller needs it (read-only-instruction
    // extra-cycle rule; write/RMW instructions always ignore it and pay
    // the fixed max cost instead -- decided per call site below, not
    // inside these helpers, since the same addressing mode is shared by
    // both kinds of instruction).

    fn addr_zp<B: Bus>(&mut self, bus: &mut B) -> u16 {
        self.fetch(bus) as u16
    }

    // Every helper below also reports the cycles the real chip spends that
    // carry no useful data (`Bus::dummy_cycle`), in real order, so a
    // cycle-accurate bus sees exactly one bus event per cycle.

    /// `zp,X`: the CPU reads the unindexed zero-page byte while it adds X.
    fn addr_zpx<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let zp = self.fetch(bus);
        bus.dummy_cycle(zp as u16, false, 0);
        zp.wrapping_add(self.x) as u16
    }

    fn addr_zpy<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let zp = self.fetch(bus);
        bus.dummy_cycle(zp as u16, false, 0);
        zp.wrapping_add(self.y) as u16
    }

    fn addr_abs<B: Bus>(&mut self, bus: &mut B) -> u16 {
        self.fetch_u16(bus)
    }

    /// Indexed-address fix-up cycle: the chip first reads at the address
    /// with the *uncorrected* high byte. When the index carried into the
    /// high byte that read is wasted (and the real access follows); a
    /// write or read-modify-write always pays it, crossing or not, which
    /// is why `always` is true for those callers.
    fn index_fixup<B: Bus>(bus: &mut B, base: u16, addr: u16, always: bool) -> bool {
        let crossed = (base & 0xFF00) != (addr & 0xFF00);
        if crossed || always {
            bus.dummy_cycle((base & 0xFF00) | (addr & 0x00FF), false, 0);
        }
        crossed
    }

    /// `abs,X`. `always` = a write/RMW caller (see `index_fixup`).
    fn addr_absx<B: Bus>(&mut self, bus: &mut B, always: bool) -> (u16, bool) {
        let base = self.fetch_u16(bus);
        let addr = base.wrapping_add(self.x as u16);
        let crossed = Self::index_fixup(bus, base, addr, always);
        (addr, crossed)
    }

    fn addr_absy<B: Bus>(&mut self, bus: &mut B, always: bool) -> (u16, bool) {
        let base = self.fetch_u16(bus);
        let addr = base.wrapping_add(self.y as u16);
        let crossed = Self::index_fixup(bus, base, addr, always);
        (addr, crossed)
    }

    /// `(zp,X)` -- zero-page pointer, added to X (wrapping in zero page)
    /// *before* the indirection, so never crosses a page.
    fn addr_indx<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let raw = self.fetch(bus);
        bus.dummy_cycle(raw as u16, false, 0);
        let zp = raw.wrapping_add(self.x);
        let lo = bus.read(zp as u16) as u16;
        let hi = bus.read(zp.wrapping_add(1) as u16) as u16;
        lo | (hi << 8)
    }

    /// `(zp),Y` -- zero-page pointer read first, Y added to the resulting
    /// 16-bit address *after* the indirection, so this one can cross a
    /// page.
    fn addr_indy<B: Bus>(&mut self, bus: &mut B, always: bool) -> (u16, bool) {
        let zp = self.fetch(bus);
        let lo = bus.read(zp as u16) as u16;
        let hi = bus.read(zp.wrapping_add(1) as u16) as u16;
        let base = lo | (hi << 8);
        let addr = base.wrapping_add(self.y as u16);
        let crossed = Self::index_fixup(bus, base, addr, always);
        (addr, crossed)
    }

    /// `JMP (abs)` -- the real NMOS indirect-jump page-wrap bug: if the
    /// pointer's low byte is `0xFF`, the high byte of the target is
    /// fetched from `ptr & 0xFF00` (wrapping within the same page) rather
    /// than `ptr + 1`. Modeled exactly, not approximated -- real programs
    /// occasionally exploit or accidentally trigger this.
    fn addr_ind_bug<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let ptr = self.addr_abs(bus);
        let lo = bus.read(ptr) as u16;
        let hi_addr = (ptr & 0xFF00) | (ptr.wrapping_add(1) & 0x00FF);
        let hi = bus.read(hi_addr) as u16;
        lo | (hi << 8)
    }

    // ---- Shared operation helpers -----------------------------------

    fn store_high<B: Bus>(bus: &mut B, address: u16, index: u8, register: u8) {
        let base = address.wrapping_sub(u16::from(index));
        let value = register & ((base >> 8) as u8).wrapping_add(1);
        // A page carry replaces the destination high byte with the masked data.
        let target = if (base & 0xff00) != (address & 0xff00) {
            (u16::from(value) << 8) | (address & 0xff)
        } else {
            address
        };
        bus.write(target, value);
    }

    fn op_and(&mut self, v: u8) {
        self.a &= v;
        self.set_zn(self.a);
    }

    fn op_ora(&mut self, v: u8) {
        self.a |= v;
        self.set_zn(self.a);
    }

    fn op_eor(&mut self, v: u8) {
        self.a ^= v;
        self.set_zn(self.a);
    }

    fn op_bit(&mut self, v: u8) {
        let z = if self.a & v == 0 { FLAG_Z } else { 0 };
        self.p = (self.p & !(FLAG_Z | FLAG_V | FLAG_N)) | z | (v & (FLAG_V | FLAG_N));
    }

    fn op_cmp(&mut self, reg: u8, v: u8) {
        let (r, borrow) = reg.overflowing_sub(v);
        self.set_zn(r);
        self.p = (self.p & !FLAG_C) | if borrow { 0 } else { FLAG_C };
    }

    fn op_adc(&mut self, v: u8) {
        if self.p & FLAG_D != 0 {
            self.adc_decimal(v);
        } else {
            let carry_in = (self.p & FLAG_C != 0) as u16;
            let a = self.a as u16;
            let m = v as u16;
            let sum = a + m + carry_in;
            let result = sum as u8;
            let overflow = (!(self.a ^ v) & (self.a ^ result) & 0x80) != 0;
            self.p = (self.p & !(FLAG_C | FLAG_V))
                | if sum > 0xFF { FLAG_C } else { 0 }
                | if overflow { FLAG_V } else { 0 };
            self.set_zn(result);
            self.a = result;
        }
    }

    fn op_sbc(&mut self, v: u8) {
        if self.p & FLAG_D != 0 {
            self.sbc_decimal(v);
        } else {
            // SBC(v) == ADC(!v) in binary two's-complement arithmetic.
            self.op_adc(!v);
        }
    }

    /// BCD `ADC`, per the algorithm from 6502.org's "Decimal Mode"
    /// tutorial (Bruce Clark) -- see this module's own doc comment for
    /// this file's epistemic-tier note on decimal mode.
    fn adc_decimal(&mut self, v: u8) {
        let a = self.a;
        let carry_in: u16 = (self.p & FLAG_C != 0) as u16;

        // NMOS quirk: Z comes from the unadjusted binary sum, while N/V
        // come from the partially BCD-adjusted result (after the low-digit
        // correction has carried into the high digit, but before the final
        // high-digit correction).
        let bin_result = a.wrapping_add(v).wrapping_add(carry_in as u8);

        let mut lo = (a & 0x0F) as u16 + (v & 0x0F) as u16 + carry_in;
        if lo >= 0x0A {
            lo = ((lo + 0x06) & 0x0F) + 0x10;
        }
        let mut sum = (a & 0xF0) as u16 + (v & 0xF0) as u16 + lo;
        let intermediate = sum as u8;
        let overflow = (!(a ^ v) & (a ^ intermediate) & 0x80) != 0;
        let carry_out = sum >= 0xA0;
        if carry_out {
            sum += 0x60;
        }
        let result = sum as u8;

        self.a = result;
        self.p = (self.p & !(FLAG_C | FLAG_V | FLAG_Z | FLAG_N))
            | if carry_out { FLAG_C } else { 0 }
            | if overflow { FLAG_V } else { 0 }
            | if bin_result == 0 { FLAG_Z } else { 0 }
            | (intermediate & FLAG_N);
    }

    /// BCD `SBC`, same source/tier as `adc_decimal` above.
    fn sbc_decimal(&mut self, v: u8) {
        let a = self.a;
        let borrow_in: i16 = if self.p & FLAG_C != 0 { 0 } else { 1 };

        let bin_diff: i16 = a as i16 - v as i16 - borrow_in;
        let bin_result = bin_diff as u8;
        let overflow = ((a ^ v) & (a ^ bin_result) & 0x80) != 0;
        let carry_out = bin_diff >= 0;

        let mut lo: i16 = (a & 0x0F) as i16 - (v & 0x0F) as i16 - borrow_in;
        if lo < 0 {
            lo = ((lo - 0x06) & 0x0F) - 0x10;
        }
        let mut sum: i16 = (a & 0xF0) as i16 - (v & 0xF0) as i16 + lo;
        if sum < 0 {
            sum -= 0x60;
        }
        let result = sum as u8;

        self.a = result;
        self.p = (self.p & !(FLAG_C | FLAG_V | FLAG_Z | FLAG_N))
            | if carry_out { FLAG_C } else { 0 }
            | if overflow { FLAG_V } else { 0 }
            | if bin_result == 0 { FLAG_Z } else { 0 }
            | (bin_result & FLAG_N);
    }

    fn asl_val(&mut self, v: u8) -> u8 {
        let carry = v & 0x80 != 0;
        let r = v << 1;
        self.p = (self.p & !FLAG_C) | if carry { FLAG_C } else { 0 };
        self.set_zn(r);
        r
    }

    fn lsr_val(&mut self, v: u8) -> u8 {
        let carry = v & 0x01 != 0;
        let r = v >> 1;
        self.p = (self.p & !FLAG_C) | if carry { FLAG_C } else { 0 };
        self.set_zn(r);
        r
    }

    fn rol_val(&mut self, v: u8) -> u8 {
        let carry_in = (self.p & FLAG_C != 0) as u8;
        let carry_out = v & 0x80 != 0;
        let r = (v << 1) | carry_in;
        self.p = (self.p & !FLAG_C) | if carry_out { FLAG_C } else { 0 };
        self.set_zn(r);
        r
    }

    fn ror_val(&mut self, v: u8) -> u8 {
        let carry_in = (self.p & FLAG_C != 0) as u8;
        let carry_out = v & 0x01 != 0;
        let r = (v >> 1) | (carry_in << 7);
        self.p = (self.p & !FLAG_C) | if carry_out { FLAG_C } else { 0 };
        self.set_zn(r);
        r
    }

    fn inc_val(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.set_zn(r);
        r
    }

    fn dec_val(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.set_zn(r);
        r
    }

    fn branch<B: Bus>(&mut self, bus: &mut B, cond: bool) -> u32 {
        let offset = self.fetch(bus) as i8;
        let mut cycles = 2;
        if cond {
            let old_pc = self.pc;
            self.pc = self.pc.wrapping_add(offset as u16);
            cycles += 1;
            // Taken: one wasted read of the next instruction's opcode ...
            bus.dummy_cycle(old_pc, false, 0);
            if (old_pc & 0xFF00) != (self.pc & 0xFF00) {
                cycles += 1;
                // ... and, across a page, one more at the not-yet-fixed PC.
                bus.dummy_cycle((old_pc & 0xFF00) | (self.pc & 0x00FF), false, 0);
            }
        }
        cycles
    }

    /// The real BRK/software-interrupt sequence: pushes `PC + 1` (BRK is
    /// architecturally a 2-byte instruction, the byte after the opcode is
    /// a padding/signature byte real monitors use to identify which BRK
    /// fired), then `P` with `FLAG_B` **set** this time (the real,
    /// hardware-visible distinction from `interrupt_sequence`'s IRQ/NMI
    /// path, where it's clear) -- lets an interrupt handler tell a
    /// software break from a real hardware IRQ apart by inspecting the
    /// stacked P.
    fn brk<B: Bus>(&mut self, bus: &mut B) -> u32 {
        // Cycle 2 reads (and skips) the signature byte.
        bus.dummy_cycle(self.pc, false, 0);
        self.pc = self.pc.wrapping_add(1);
        self.push_u16(bus, self.pc);
        let vector = Self::irq_vector(bus);
        self.push(bus, self.p | FLAG_U | FLAG_B);
        self.p |= crate::cpu::FLAG_I;
        self.pc = {
            let lo = bus.read(vector) as u16;
            let hi = bus.read(vector + 1) as u16;
            lo | (hi << 8)
        };
        7
    }

    fn rti<B: Bus>(&mut self, bus: &mut B) -> u32 {
        bus.dummy_cycle(self.pc, false, 0);
        bus.dummy_cycle(self.stack_addr(), false, 0);
        self.p = (self.pop(bus) & !FLAG_B) | FLAG_U;
        self.pc = self.pop_u16(bus);
        6
    }

    fn rts<B: Bus>(&mut self, bus: &mut B) -> u32 {
        bus.dummy_cycle(self.pc, false, 0);
        bus.dummy_cycle(self.stack_addr(), false, 0);
        self.pc = self.pop_u16(bus);
        // Cycle 6 reads at the popped address before incrementing it.
        bus.dummy_cycle(self.pc, false, 0);
        self.pc = self.pc.wrapping_add(1);
        6
    }

    fn jsr<B: Bus>(&mut self, bus: &mut B) -> u32 {
        // Real order: fetch the low byte, a stack read, push the address
        // of the *last byte of JSR itself* (PC now points at the high
        // operand byte), and only then fetch the high byte -- RTS's `+1`
        // is what makes the pair correct.
        let lo = self.fetch(bus) as u16;
        bus.dummy_cycle(self.stack_addr(), false, 0);
        self.push_u16(bus, self.pc);
        let hi = self.fetch(bus) as u16;
        self.pc = lo | (hi << 8);
        6
    }

    /// The real NMOS "JAM"/"KIL"/"HLT" illegal opcodes -- lock the
    /// address/data bus solid on real silicon, requiring a hardware
    /// reset. Modeled as a halt, same role `z80::Cpu::halted` plays for a
    /// Z80 `HALT`.
    fn jam<B: Bus>(&mut self, bus: &mut B) -> u32 {
        // Keep the bus's cycle count equal to the 2 reported below.
        bus.dummy_cycle(self.pc, false, 0);
        self.halted = true;
        2
    }

    #[rustfmt::skip]
    pub(crate) fn execute<B: Bus>(&mut self, opcode: u8, bus: &mut B) -> u32 {
        // Implied/accumulator instructions spend their second cycle reading
        // (and discarding) the byte after the opcode.
        if matches!(
            opcode,
            0x0A | 0x4A | 0x2A | 0x6A | 0xE8 | 0xC8 | 0xCA | 0x88 | 0xAA | 0xA8 | 0x8A | 0x98
                | 0xBA | 0x9A | 0x18 | 0x38 | 0x58 | 0x78 | 0xB8 | 0xD8 | 0xF8 | 0xEA | 0x1A
                | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA
        ) {
            bus.dummy_cycle(self.pc, false, 0);
        }
        match opcode {
            // ---- Loads --------------------------------------------
            0xA9 => { let v = self.fetch(bus); self.a = v; self.set_zn(v); 2 }
            0xA5 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.a = v; self.set_zn(v); 3 }
            0xB5 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); self.a = v; self.set_zn(v); 4 }
            0xAD => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.a = v; self.set_zn(v); 4 }
            0xBD => { let (addr, pc) = self.addr_absx(bus, false); let v = bus.read(addr); self.a = v; self.set_zn(v); 4 + pc as u32 }
            0xB9 => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.a = v; self.set_zn(v); 4 + pc as u32 }
            0xA1 => { let addr = self.addr_indx(bus); let v = bus.read(addr); self.a = v; self.set_zn(v); 6 }
            0xB1 => { let (addr, pc) = self.addr_indy(bus, false); let v = bus.read(addr); self.a = v; self.set_zn(v); 5 + pc as u32 }

            0xA2 => { let v = self.fetch(bus); self.x = v; self.set_zn(v); 2 }
            0xA6 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.x = v; self.set_zn(v); 3 }
            0xB6 => { let addr = self.addr_zpy(bus); let v = bus.read(addr); self.x = v; self.set_zn(v); 4 }
            0xAE => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.x = v; self.set_zn(v); 4 }
            0xBE => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.x = v; self.set_zn(v); 4 + pc as u32 }

            0xA0 => { let v = self.fetch(bus); self.y = v; self.set_zn(v); 2 }
            0xA4 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.y = v; self.set_zn(v); 3 }
            0xB4 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); self.y = v; self.set_zn(v); 4 }
            0xAC => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.y = v; self.set_zn(v); 4 }
            0xBC => { let (addr, pc) = self.addr_absx(bus, false); let v = bus.read(addr); self.y = v; self.set_zn(v); 4 + pc as u32 }

            // ---- Stores (fixed cost, no page-cross savings) --------
            0x85 => { let addr = self.addr_zp(bus); bus.write(addr, self.a); 3 }
            0x95 => { let addr = self.addr_zpx(bus); bus.write(addr, self.a); 4 }
            0x8D => { let addr = self.addr_abs(bus); bus.write(addr, self.a); 4 }
            0x9D => { let (addr, _) = self.addr_absx(bus, true); bus.write(addr, self.a); 5 }
            0x99 => { let (addr, _) = self.addr_absy(bus, true); bus.write(addr, self.a); 5 }
            0x81 => { let addr = self.addr_indx(bus); bus.write(addr, self.a); 6 }
            0x91 => { let (addr, _) = self.addr_indy(bus, true); bus.write(addr, self.a); 6 }

            0x86 => { let addr = self.addr_zp(bus); bus.write(addr, self.x); 3 }
            0x96 => { let addr = self.addr_zpy(bus); bus.write(addr, self.x); 4 }
            0x8E => { let addr = self.addr_abs(bus); bus.write(addr, self.x); 4 }

            0x84 => { let addr = self.addr_zp(bus); bus.write(addr, self.y); 3 }
            0x94 => { let addr = self.addr_zpx(bus); bus.write(addr, self.y); 4 }
            0x8C => { let addr = self.addr_abs(bus); bus.write(addr, self.y); 4 }

            // ---- Logic / arithmetic (read value from any mode) -----
            0x29 => { let v = self.fetch(bus); self.op_and(v); 2 }
            0x25 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_and(v); 3 }
            0x35 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); self.op_and(v); 4 }
            0x2D => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_and(v); 4 }
            0x3D => { let (addr, pc) = self.addr_absx(bus, false); let v = bus.read(addr); self.op_and(v); 4 + pc as u32 }
            0x39 => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.op_and(v); 4 + pc as u32 }
            0x21 => { let addr = self.addr_indx(bus); let v = bus.read(addr); self.op_and(v); 6 }
            0x31 => { let (addr, pc) = self.addr_indy(bus, false); let v = bus.read(addr); self.op_and(v); 5 + pc as u32 }

            0x09 => { let v = self.fetch(bus); self.op_ora(v); 2 }
            0x05 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_ora(v); 3 }
            0x15 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); self.op_ora(v); 4 }
            0x0D => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_ora(v); 4 }
            0x1D => { let (addr, pc) = self.addr_absx(bus, false); let v = bus.read(addr); self.op_ora(v); 4 + pc as u32 }
            0x19 => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.op_ora(v); 4 + pc as u32 }
            0x01 => { let addr = self.addr_indx(bus); let v = bus.read(addr); self.op_ora(v); 6 }
            0x11 => { let (addr, pc) = self.addr_indy(bus, false); let v = bus.read(addr); self.op_ora(v); 5 + pc as u32 }

            0x49 => { let v = self.fetch(bus); self.op_eor(v); 2 }
            0x45 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_eor(v); 3 }
            0x55 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); self.op_eor(v); 4 }
            0x4D => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_eor(v); 4 }
            0x5D => { let (addr, pc) = self.addr_absx(bus, false); let v = bus.read(addr); self.op_eor(v); 4 + pc as u32 }
            0x59 => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.op_eor(v); 4 + pc as u32 }
            0x41 => { let addr = self.addr_indx(bus); let v = bus.read(addr); self.op_eor(v); 6 }
            0x51 => { let (addr, pc) = self.addr_indy(bus, false); let v = bus.read(addr); self.op_eor(v); 5 + pc as u32 }

            0x69 => { let v = self.fetch(bus); self.op_adc(v); 2 }
            0x65 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_adc(v); 3 }
            0x75 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); self.op_adc(v); 4 }
            0x6D => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_adc(v); 4 }
            0x7D => { let (addr, pc) = self.addr_absx(bus, false); let v = bus.read(addr); self.op_adc(v); 4 + pc as u32 }
            0x79 => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.op_adc(v); 4 + pc as u32 }
            0x61 => { let addr = self.addr_indx(bus); let v = bus.read(addr); self.op_adc(v); 6 }
            0x71 => { let (addr, pc) = self.addr_indy(bus, false); let v = bus.read(addr); self.op_adc(v); 5 + pc as u32 }

            0xE9 | 0xEB => { let v = self.fetch(bus); self.op_sbc(v); 2 } // 0xEB: illegal SBC duplicate
            0xE5 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_sbc(v); 3 }
            0xF5 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); self.op_sbc(v); 4 }
            0xED => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_sbc(v); 4 }
            0xFD => { let (addr, pc) = self.addr_absx(bus, false); let v = bus.read(addr); self.op_sbc(v); 4 + pc as u32 }
            0xF9 => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.op_sbc(v); 4 + pc as u32 }
            0xE1 => { let addr = self.addr_indx(bus); let v = bus.read(addr); self.op_sbc(v); 6 }
            0xF1 => { let (addr, pc) = self.addr_indy(bus, false); let v = bus.read(addr); self.op_sbc(v); 5 + pc as u32 }

            0xC9 => { let v = self.fetch(bus); self.op_cmp(self.a, v); 2 }
            0xC5 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_cmp(self.a, v); 3 }
            0xD5 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); self.op_cmp(self.a, v); 4 }
            0xCD => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_cmp(self.a, v); 4 }
            0xDD => { let (addr, pc) = self.addr_absx(bus, false); let v = bus.read(addr); self.op_cmp(self.a, v); 4 + pc as u32 }
            0xD9 => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.op_cmp(self.a, v); 4 + pc as u32 }
            0xC1 => { let addr = self.addr_indx(bus); let v = bus.read(addr); self.op_cmp(self.a, v); 6 }
            0xD1 => { let (addr, pc) = self.addr_indy(bus, false); let v = bus.read(addr); self.op_cmp(self.a, v); 5 + pc as u32 }

            0xE0 => { let v = self.fetch(bus); self.op_cmp(self.x, v); 2 }
            0xE4 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_cmp(self.x, v); 3 }
            0xEC => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_cmp(self.x, v); 4 }

            0xC0 => { let v = self.fetch(bus); self.op_cmp(self.y, v); 2 }
            0xC4 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_cmp(self.y, v); 3 }
            0xCC => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_cmp(self.y, v); 4 }

            0x24 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.op_bit(v); 3 }
            0x2C => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.op_bit(v); 4 }

            // ---- Read-modify-write (fixed cost, no page-cross savings) --
            0x0A => { self.a = self.asl_val(self.a); 2 }
            0x06 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); 5 }
            0x16 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); 6 }
            0x0E => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); 6 }
            0x1E => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); 7 }

            0x4A => { self.a = self.lsr_val(self.a); 2 }
            0x46 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); 5 }
            0x56 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); 6 }
            0x4E => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); 6 }
            0x5E => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); 7 }

            0x2A => { self.a = self.rol_val(self.a); 2 }
            0x26 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); 5 }
            0x36 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); 6 }
            0x2E => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); 6 }
            0x3E => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); 7 }

            0x6A => { self.a = self.ror_val(self.a); 2 }
            0x66 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); 5 }
            0x76 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); 6 }
            0x6E => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); 6 }
            0x7E => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); 7 }

            0xE6 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); 5 }
            0xF6 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); 6 }
            0xEE => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); 6 }
            0xFE => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); 7 }

            0xC6 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); 5 }
            0xD6 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); 6 }
            0xCE => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); 6 }
            0xDE => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); 7 }

            // ---- Register increment/decrement ----------------------
            0xE8 => { self.x = self.x.wrapping_add(1); let v = self.x; self.set_zn(v); 2 }
            0xC8 => { self.y = self.y.wrapping_add(1); let v = self.y; self.set_zn(v); 2 }
            0xCA => { self.x = self.x.wrapping_sub(1); let v = self.x; self.set_zn(v); 2 }
            0x88 => { self.y = self.y.wrapping_sub(1); let v = self.y; self.set_zn(v); 2 }

            // ---- Transfers ------------------------------------------
            0xAA => { self.x = self.a; let v = self.x; self.set_zn(v); 2 }
            0xA8 => { self.y = self.a; let v = self.y; self.set_zn(v); 2 }
            0x8A => { self.a = self.x; let v = self.a; self.set_zn(v); 2 }
            0x98 => { self.a = self.y; let v = self.a; self.set_zn(v); 2 }
            0xBA => { self.x = self.sp; let v = self.x; self.set_zn(v); 2 }
            0x9A => { self.sp = self.x; 2 } // TXS: no flags affected

            // ---- Stack ------------------------------------------------
            0x48 => { bus.dummy_cycle(self.pc, false, 0); self.push(bus, self.a); 3 }
            0x08 => { bus.dummy_cycle(self.pc, false, 0); let p = self.p | FLAG_U | FLAG_B; self.push(bus, p); 3 }
            0x68 => { bus.dummy_cycle(self.pc, false, 0); bus.dummy_cycle(self.stack_addr(), false, 0); let v = self.pop(bus); self.a = v; self.set_zn(v); 4 }
            0x28 => { bus.dummy_cycle(self.pc, false, 0); bus.dummy_cycle(self.stack_addr(), false, 0); self.p = (self.pop(bus) & !FLAG_B) | FLAG_U; 4 }

            // ---- Branches -----------------------------------------
            0x90 => self.branch(bus, self.p & FLAG_C == 0),
            0xB0 => self.branch(bus, self.p & FLAG_C != 0),
            0xF0 => self.branch(bus, self.p & FLAG_Z != 0),
            0xD0 => self.branch(bus, self.p & FLAG_Z == 0),
            0x30 => self.branch(bus, self.p & FLAG_N != 0),
            0x10 => self.branch(bus, self.p & FLAG_N == 0),
            0x50 => self.branch(bus, self.p & FLAG_V == 0),
            0x70 => self.branch(bus, self.p & FLAG_V != 0),

            // ---- Jumps / subroutines / interrupts ------------------
            0x4C => { self.pc = self.addr_abs(bus); 3 }
            0x6C => { self.pc = self.addr_ind_bug(bus); 5 }
            0x20 => self.jsr(bus),
            0x60 => self.rts(bus),
            0x00 => self.brk(bus),
            0x40 => self.rti(bus),

            // ---- Flags ----------------------------------------------
            0x18 => { self.p &= !FLAG_C; 2 }
            0x38 => { self.p |= FLAG_C; 2 }
            0x58 => { self.p &= !crate::cpu::FLAG_I; 2 }
            0x78 => { self.p |= crate::cpu::FLAG_I; 2 }
            0xB8 => { self.p &= !FLAG_V; 2 }
            0xD8 => { self.p &= !FLAG_D; 2 }
            0xF8 => { self.p |= FLAG_D; 2 }

            // ---- NOP (documented) -----------------------------------
            0xEA => 2,

            // ================= Illegal / undocumented opcodes =======
            // LAX: LDA+LDX combined.
            0xA7 => { let addr = self.addr_zp(bus); let v = bus.read(addr); self.a = v; self.x = v; self.set_zn(v); 3 }
            0xB7 => { let addr = self.addr_zpy(bus); let v = bus.read(addr); self.a = v; self.x = v; self.set_zn(v); 4 }
            0xAF => { let addr = self.addr_abs(bus); let v = bus.read(addr); self.a = v; self.x = v; self.set_zn(v); 4 }
            0xBF => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr); self.a = v; self.x = v; self.set_zn(v); 4 + pc as u32 }
            0xA3 => { let addr = self.addr_indx(bus); let v = bus.read(addr); self.a = v; self.x = v; self.set_zn(v); 6 }
            0xB3 => { let (addr, pc) = self.addr_indy(bus, false); let v = bus.read(addr); self.a = v; self.x = v; self.set_zn(v); 5 + pc as u32 }
            // LAX #imm: the one genuinely chip-unstable illegal opcode in
            // this table (real hardware's result depends on analog bus
            // capacitance/leakage, not just logic) -- implemented as a
            // plain LDA #imm + implicit TAX, the behavior most real
            // software written against it actually expects/tolerates.
            0xAB => { let v = self.fetch(bus); self.a = v; self.x = v; self.set_zn(v); 2 }

            // SAX: store (A & X).
            0x87 => { let addr = self.addr_zp(bus); bus.write(addr, self.a & self.x); 3 }
            0x97 => { let addr = self.addr_zpy(bus); bus.write(addr, self.a & self.x); 4 }
            0x8F => { let addr = self.addr_abs(bus); bus.write(addr, self.a & self.x); 4 }
            0x83 => { let addr = self.addr_indx(bus); bus.write(addr, self.a & self.x); 6 }

            // SLO: ASL memory, then ORA with A.
            0x07 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); self.op_ora(r); 5 }
            0x17 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); self.op_ora(r); 6 }
            0x0F => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); self.op_ora(r); 6 }
            0x1F => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); self.op_ora(r); 7 }
            0x1B => { let (addr, _) = self.addr_absy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); self.op_ora(r); 7 }
            0x03 => { let addr = self.addr_indx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); self.op_ora(r); 8 }
            0x13 => { let (addr, _) = self.addr_indy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.asl_val(v); bus.write(addr, r); self.op_ora(r); 8 }

            // RLA: ROL memory, then AND with A.
            0x27 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); self.op_and(r); 5 }
            0x37 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); self.op_and(r); 6 }
            0x2F => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); self.op_and(r); 6 }
            0x3F => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); self.op_and(r); 7 }
            0x3B => { let (addr, _) = self.addr_absy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); self.op_and(r); 7 }
            0x23 => { let addr = self.addr_indx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); self.op_and(r); 8 }
            0x33 => { let (addr, _) = self.addr_indy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.rol_val(v); bus.write(addr, r); self.op_and(r); 8 }

            // SRE: LSR memory, then EOR with A.
            0x47 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); self.op_eor(r); 5 }
            0x57 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); self.op_eor(r); 6 }
            0x4F => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); self.op_eor(r); 6 }
            0x5F => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); self.op_eor(r); 7 }
            0x5B => { let (addr, _) = self.addr_absy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); self.op_eor(r); 7 }
            0x43 => { let addr = self.addr_indx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); self.op_eor(r); 8 }
            0x53 => { let (addr, _) = self.addr_indy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.lsr_val(v); bus.write(addr, r); self.op_eor(r); 8 }

            // RRA: ROR memory, then ADC with A.
            0x67 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); self.op_adc(r); 5 }
            0x77 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); self.op_adc(r); 6 }
            0x6F => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); self.op_adc(r); 6 }
            0x7F => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); self.op_adc(r); 7 }
            0x7B => { let (addr, _) = self.addr_absy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); self.op_adc(r); 7 }
            0x63 => { let addr = self.addr_indx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); self.op_adc(r); 8 }
            0x73 => { let (addr, _) = self.addr_indy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.ror_val(v); bus.write(addr, r); self.op_adc(r); 8 }

            // DCP: DEC memory, then CMP with A.
            0xC7 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); self.op_cmp(self.a, r); 5 }
            0xD7 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); self.op_cmp(self.a, r); 6 }
            0xCF => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); self.op_cmp(self.a, r); 6 }
            0xDF => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); self.op_cmp(self.a, r); 7 }
            0xDB => { let (addr, _) = self.addr_absy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); self.op_cmp(self.a, r); 7 }
            0xC3 => { let addr = self.addr_indx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); self.op_cmp(self.a, r); 8 }
            0xD3 => { let (addr, _) = self.addr_indy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.dec_val(v); bus.write(addr, r); self.op_cmp(self.a, r); 8 }

            // ISC/ISB: INC memory, then SBC with A.
            0xE7 => { let addr = self.addr_zp(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); self.op_sbc(r); 5 }
            0xF7 => { let addr = self.addr_zpx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); self.op_sbc(r); 6 }
            0xEF => { let addr = self.addr_abs(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); self.op_sbc(r); 6 }
            0xFF => { let (addr, _) = self.addr_absx(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); self.op_sbc(r); 7 }
            0xFB => { let (addr, _) = self.addr_absy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); self.op_sbc(r); 7 }
            0xE3 => { let addr = self.addr_indx(bus); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); self.op_sbc(r); 8 }
            0xF3 => { let (addr, _) = self.addr_indy(bus, true); let v = bus.read(addr); bus.dummy_cycle(addr, true, v); let r = self.inc_val(v); bus.write(addr, r); self.op_sbc(r); 8 }

            // ANC #imm: AND #imm, then copy bit 7 of the result into carry
            // (as if the AND result had been shifted through ASL/ROL).
            0x0B | 0x2B => {
                let v = self.fetch(bus);
                self.op_and(v);
                self.p = (self.p & !FLAG_C) | if self.a & 0x80 != 0 { FLAG_C } else { 0 };
                2
            }
            // ALR/ASR #imm: AND #imm, then LSR A.
            0x4B => {
                let v = self.fetch(bus);
                self.op_and(v);
                self.a = self.lsr_val(self.a);
                2
            }
            // ARR #imm: AND #imm, then ROR A. Binary mode derives C/V
            // from bits 6/5 of the rotated result. Decimal mode performs
            // the NMOS chip's two nibble corrections while N/Z remain the
            // flags from the unadjusted rotation.
            0x6B => {
                let operand = self.fetch(bus);
                let anded = self.a & operand;
                let carry_in = u8::from(self.p & FLAG_C != 0);
                let rotated = (anded >> 1) | (carry_in << 7);
                self.a = rotated;
                self.set_zn(rotated);

                if self.p & FLAG_D != 0 {
                    let overflow = (rotated ^ anded) & 0x40 != 0;
                    let mut result = rotated;
                    if (anded & 0x0F) + (anded & 0x01) > 5 {
                        result = (result & 0xF0) | (result.wrapping_add(0x06) & 0x0F);
                    }
                    let high = anded >> 4;
                    let carry_out = high + (high & 0x01) > 5;
                    if carry_out {
                        result = result.wrapping_add(0x60);
                    }
                    self.a = result;
                    self.p = (self.p & !(FLAG_C | FLAG_V))
                        | if carry_out { FLAG_C } else { 0 }
                        | if overflow { FLAG_V } else { 0 };
                } else {
                    let bit6 = rotated & 0x40 != 0;
                    let bit5 = rotated & 0x20 != 0;
                    self.p = (self.p & !(FLAG_C | FLAG_V))
                        | if bit6 { FLAG_C } else { 0 }
                        | if bit6 ^ bit5 { FLAG_V } else { 0 };
                }
                2
            }
            // SBX/AXS #imm: X = (A & X) - imm, flags set like CMP (no
            // decimal mode -- this one is defined as pure binary on real
            // hardware regardless of the D flag).
            0xCB => {
                let v = self.fetch(bus);
                let base = self.a & self.x;
                let (r, borrow) = base.overflowing_sub(v);
                self.x = r;
                self.set_zn(r);
                self.p = (self.p & !FLAG_C) | if borrow { 0 } else { FLAG_C };
                2
            }

            // ---- Highly unstable group: SHY/SHX/SHA/TAS/LAS --------
            // All commonly documented as chip-unstable (the stored value
            // depends on the specific high-byte-plus-1 ANDing behavior,
            // which real hardware sometimes deviates from under certain
            // bus conditions) -- implemented per the deterministic
            // formula every reference agrees on as the *common* case; see
            // this file's top-level epistemic-tier note.
            0x9C => { let (addr, _) = self.addr_absx(bus, true); Self::store_high(bus, addr, self.x, self.y); 5 }
            0x9E => { let (addr, _) = self.addr_absy(bus, true); Self::store_high(bus, addr, self.y, self.x); 5 }
            0x9F => { let (addr, _) = self.addr_absy(bus, true); Self::store_high(bus, addr, self.y, self.a & self.x); 5 }
            0x93 => { let (addr, _) = self.addr_indy(bus, true); Self::store_high(bus, addr, self.y, self.a & self.x); 6 }
            0x9B => { let (addr, _) = self.addr_absy(bus, true); self.sp = self.a & self.x; Self::store_high(bus, addr, self.y, self.sp); 5 }
            0xBB => { let (addr, pc) = self.addr_absy(bus, false); let v = bus.read(addr) & self.sp; self.a = v; self.x = v; self.sp = v; self.set_zn(v); 4 + pc as u32 }
            // XAA/ANE #imm: real hardware computes `(A | magic) & X & imm`
            // where `magic` is a chip/temperature-dependent constant --
            // the single most unstable opcode in the whole illegal set,
            // essentially never relied on intentionally by real software.
            // Approximated here as `A = X & imm` (`magic` treated as
            // `0x00`), the common "safe" approximation most emulators use.
            0x8B => { let v = self.fetch(bus); self.a = self.x & v; let r = self.a; self.set_zn(r); 2 }

            // ---- Illegal NOPs (various widths/addressing modes) ----
            0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => 2,
            0x80 | 0x82 | 0x89 | 0xC2 | 0xE2 => { self.fetch(bus); 2 }
            0x04 | 0x44 | 0x64 => { let addr = self.addr_zp(bus); bus.read(addr); 3 }
            0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 => { let addr = self.addr_zpx(bus); bus.read(addr); 4 }
            0x0C => { let addr = self.addr_abs(bus); bus.read(addr); 4 }
            0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => { let (addr, pc) = self.addr_absx(bus, false); bus.read(addr); 4 + pc as u32 }

            // ---- JAM/KIL/HLT: locks the CPU up -----------------------
            0x02 | 0x12 | 0x22 | 0x32 | 0x42 | 0x52 | 0x62 | 0x72 | 0x92 | 0xB2 | 0xD2 | 0xF2 => self.jam(bus),
        }
    }
}

/// Focused unit tests for arithmetic flags, addressing-mode edge cases,
/// stack/interrupt behavior, and representative illegal opcodes. The
/// independent Klaus Dormann and Tom Seddon suites live in
/// `tests/functional.rs`; these smaller cases remain because they isolate
/// failures and run cheaply in ordinary workspace tests.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Cpu;
    use crate::test_support::FlatBus;

    fn run_one(cpu: &mut Cpu, bus: &mut FlatBus, pc: u16, bytes: &[u8]) -> u32 {
        bus.load(pc, bytes);
        cpu.pc = pc;
        cpu.step(bus)
    }

    fn adc_binary_basic() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x01;
        run_one(&mut cpu, &mut bus, 0x0200, &[0x69, 0x01]); // ADC #$01
        assert_eq!(cpu.a, 0x02);
        assert_eq!(cpu.p & (FLAG_C | FLAG_V | FLAG_Z | FLAG_N), 0);
    }

    fn adc_binary_signed_overflow_sets_v() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x7F;
        run_one(&mut cpu, &mut bus, 0x0200, &[0x69, 0x01]); // ADC #$01
        assert_eq!(cpu.a, 0x80);
        assert_ne!(
            cpu.p & FLAG_V,
            0,
            "positive + positive = negative must set V"
        );
        assert_ne!(cpu.p & FLAG_N, 0);
        assert_eq!(cpu.p & FLAG_C, 0);
    }

    fn adc_binary_carry_and_zero() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0xFF;
        run_one(&mut cpu, &mut bus, 0x0200, &[0x69, 0x01]); // ADC #$01
        assert_eq!(cpu.a, 0x00);
        assert_ne!(cpu.p & FLAG_C, 0);
        assert_ne!(cpu.p & FLAG_Z, 0);
    }
    /// Merged (every check kept): `adc_binary_basic`, `adc_binary_signed_overflow_sets_v`, `adc_binary_carry_and_zero`.
    #[test]
    fn adc_binary() {
        adc_binary_basic();
        adc_binary_signed_overflow_sets_v();
        adc_binary_carry_and_zero();
    }

    fn sbc_binary_no_borrow_keeps_carry_set() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x05;
        cpu.p |= FLAG_C; // carry set = no borrow going in
        run_one(&mut cpu, &mut bus, 0x0200, &[0xE9, 0x01]); // SBC #$01
        assert_eq!(cpu.a, 0x04);
        assert_ne!(cpu.p & FLAG_C, 0, "no borrow occurred, C should stay set");
    }

    fn sbc_binary_borrow_clears_carry() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x00;
        cpu.p |= FLAG_C;
        run_one(&mut cpu, &mut bus, 0x0200, &[0xE9, 0x01]); // SBC #$01
        assert_eq!(cpu.a, 0xFF);
        assert_eq!(cpu.p & FLAG_C, 0, "borrow occurred, C should clear");
        assert_ne!(cpu.p & FLAG_N, 0);
    }
    /// Merged (every check kept): `sbc_binary_no_borrow_keeps_carry_set`, `sbc_binary_borrow_clears_carry`.
    #[test]
    fn sbc_binary() {
        sbc_binary_no_borrow_keeps_carry_set();
        sbc_binary_borrow_clears_carry();
    }

    fn adc_decimal_99_plus_1_wraps_with_carry() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x99; // BCD 99
        cpu.p |= FLAG_D;
        run_one(&mut cpu, &mut bus, 0x0200, &[0x69, 0x01]); // ADC #$01
        assert_eq!(cpu.a, 0x00, "BCD 99 + 1 should wrap to 00");
        assert_ne!(cpu.p & FLAG_C, 0, "BCD 99 + 1 should set carry (=100)");
    }

    fn adc_decimal_basic_no_carry() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x12; // BCD 12
        cpu.p |= FLAG_D;
        run_one(&mut cpu, &mut bus, 0x0200, &[0x69, 0x34]); // ADC #$34 (BCD 34)
        assert_eq!(cpu.a, 0x46, "BCD 12 + 34 = 46");
        assert_eq!(cpu.p & FLAG_C, 0);
    }

    fn adc_decimal_flags_use_the_nmos_partial_adjustment() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x79;
        cpu.p |= FLAG_D | FLAG_C;

        run_one(&mut cpu, &mut bus, 0x0200, &[0x69, 0x00]);

        assert_eq!(cpu.a, 0x80);
        assert_eq!(cpu.p & (FLAG_N | FLAG_V | FLAG_Z | FLAG_C), FLAG_N | FLAG_V);
    }
    /// Merged (every check kept): `adc_decimal_99_plus_1_wraps_with_carry`, `adc_decimal_basic_no_carry`, `adc_decimal_flags_use_the_nmos_partial_adjustment`.
    #[test]
    fn adc_decimal() {
        adc_decimal_99_plus_1_wraps_with_carry();
        adc_decimal_basic_no_carry();
        adc_decimal_flags_use_the_nmos_partial_adjustment();
    }

    #[test]
    fn cmp_sets_carry_when_no_borrow_and_zero_when_equal() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x10;
        run_one(&mut cpu, &mut bus, 0x0200, &[0xC9, 0x10]); // CMP #$10
        assert_ne!(cpu.p & FLAG_C, 0);
        assert_ne!(cpu.p & FLAG_Z, 0);

        cpu.a = 0x10;
        run_one(&mut cpu, &mut bus, 0x0300, &[0xC9, 0x20]); // CMP #$20
        assert_eq!(cpu.p & FLAG_C, 0, "A < M should clear carry (borrow)");
        assert_ne!(cpu.p & FLAG_N, 0, "0x10 - 0x20 = 0xF0, bit 7 set");
    }

    #[test]
    fn asl_rol_chain_through_carry() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x81; // top and bottom bit set
        // ASL A: carry <- bit7(1), A = 0x02
        run_one(&mut cpu, &mut bus, 0x0200, &[0x0A]);
        assert_eq!(cpu.a, 0x02);
        assert_ne!(cpu.p & FLAG_C, 0);
        // ROL A: bit0 <- old carry(1), carry <- bit7(0) -> A = 0x05
        let cycles = run_one(&mut cpu, &mut bus, 0x0201, &[0x2A]);
        assert_eq!(cpu.a, 0x05);
        assert_eq!(cpu.p & FLAG_C, 0);
        assert_eq!(cycles, 2);
    }

    #[test]
    fn zero_page_x_wraps_within_zero_page() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.x = 0x05;
        bus.load(0x0003, &[0x42]); // 0xFE + 0x05 wraps to 0x03
        run_one(&mut cpu, &mut bus, 0x0200, &[0xB5, 0xFE]); // LDA $FE,X
        assert_eq!(cpu.a, 0x42);
    }

    #[test]
    fn indexed_indirect_x_wraps_pointer_in_zero_page() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.x = 0x04;
        // pointer bytes at ($FE + X) wrapped in zero page = $02/$03
        bus.load(0x0002, &[0x00, 0x03]); // -> target $0300
        bus.load(0x0300, &[0x55]);
        run_one(&mut cpu, &mut bus, 0x0200, &[0xA1, 0xFE]); // LDA ($FE,X)
        assert_eq!(cpu.a, 0x55);
    }

    #[test]
    fn indirect_indexed_y_crosses_page_and_adds_cycle() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.y = 0xFF;
        bus.load(0x0010, &[0x01, 0x02]); // base pointer = $0201
        bus.load(0x0300, &[0x77]); // $0201 + $FF = $0300 (page crossed)
        let cycles = run_one(&mut cpu, &mut bus, 0x0200, &[0xB1, 0x10]); // LDA ($10),Y
        assert_eq!(cpu.a, 0x77);
        assert_eq!(cycles, 6, "5 base + 1 page-cross");
    }

    #[test]
    fn jmp_indirect_has_the_real_page_wrap_bug() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        // Pointer at $30FF: low byte at $30FF, high byte real hardware
        // (mis-)fetches from $3000, not $3100.
        bus.load(0x30FF, &[0x80]);
        bus.load(0x3000, &[0x40]);
        bus.load(0x3100, &[0xFF]); // if the bug weren't modeled, we'd see this
        run_one(&mut cpu, &mut bus, 0x0200, &[0x6C, 0xFF, 0x30]); // JMP ($30FF)
        assert_eq!(cpu.pc, 0x4080, "high byte should wrap to $3000, not $3100");
    }

    #[test]
    fn jsr_then_rts_returns_to_the_instruction_after_jsr() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.sp = 0xFD;
        bus.load(0x0200, &[0x20, 0x00, 0x03]); // JSR $0300
        bus.load(0x0300, &[0x60]); // RTS
        cpu.pc = 0x0200;
        cpu.step(&mut bus); // JSR
        assert_eq!(cpu.pc, 0x0300);
        cpu.step(&mut bus); // RTS
        assert_eq!(cpu.pc, 0x0203, "should resume right after the 3-byte JSR");
        assert_eq!(cpu.sp, 0xFD, "stack should be balanced");
    }

    #[test]
    fn brk_then_rti_round_trips_pc_and_flags() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.load(0xFFFE, &[0x00, 0x04]); // IRQ/BRK vector -> $0400
        bus.load(0x0400, &[0x40]); // RTI
        bus.load(0x0200, &[0x00, 0xEA]); // BRK, padding byte
        cpu.pc = 0x0200;
        cpu.p = FLAG_U; // start with a known, distinctive flag state
        cpu.step(&mut bus); // BRK
        assert_eq!(cpu.pc, 0x0400);
        assert_ne!(cpu.p & crate::cpu::FLAG_I, 0, "BRK should set the I flag");
        cpu.step(&mut bus); // RTI
        assert_eq!(
            cpu.pc, 0x0202,
            "RTI should restore PC to right after BRK+padding"
        );
    }

    #[test]
    fn branch_cycle_costs_not_taken_same_page_and_crossing() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();

        cpu.p |= FLAG_Z;
        let c = run_one(&mut cpu, &mut bus, 0x0200, &[0xD0, 0x10]); // BNE, not taken (Z set)
        assert_eq!(c, 2);

        cpu.p &= !FLAG_Z;
        let c = run_one(&mut cpu, &mut bus, 0x0200, &[0xD0, 0x10]); // BNE, taken, same page
        assert_eq!(c, 3);

        cpu.p &= !FLAG_Z;
        // 0x02F0 + 2 + offset crosses into the next page.
        let c = run_one(&mut cpu, &mut bus, 0x02F0, &[0xD0, 0x20]);
        assert_eq!(c, 4);
    }

    #[test]
    fn lax_loads_both_a_and_x() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.load(0x0010, &[0x37]);
        run_one(&mut cpu, &mut bus, 0x0200, &[0xA7, 0x10]); // LAX $10
        assert_eq!(cpu.a, 0x37);
        assert_eq!(cpu.x, 0x37);
    }

    #[test]
    fn sax_stores_a_and_x() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0xF0;
        cpu.x = 0x0F;
        run_one(&mut cpu, &mut bus, 0x0200, &[0x87, 0x10]); // SAX $10
        assert_eq!(bus.read(0x0010), 0x00, "0xF0 & 0x0F == 0x00");
    }

    #[test]
    fn slo_shifts_memory_then_oras_into_a() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x01;
        bus.load(0x0010, &[0x80]); // ASL -> 0x00, carry set
        run_one(&mut cpu, &mut bus, 0x0200, &[0x07, 0x10]); // SLO $10
        assert_eq!(bus.read(0x0010), 0x00);
        assert_eq!(cpu.a, 0x01, "0x01 | 0x00 == 0x01");
        assert_ne!(cpu.p & FLAG_C, 0);
    }

    #[test]
    fn dcp_decrements_memory_then_compares_with_a() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x05;
        bus.load(0x0010, &[0x06]); // DEC -> 0x05
        run_one(&mut cpu, &mut bus, 0x0200, &[0xC7, 0x10]); // DCP $10
        assert_eq!(bus.read(0x0010), 0x05);
        assert_ne!(cpu.p & FLAG_Z, 0, "A == decremented memory");
    }

    #[test]
    fn isc_increments_memory_then_sbcs_from_a() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x05;
        cpu.p |= FLAG_C; // no borrow going in
        bus.load(0x0010, &[0x01]); // INC -> 0x02
        run_one(&mut cpu, &mut bus, 0x0200, &[0xE7, 0x10]); // ISC $10
        assert_eq!(bus.read(0x0010), 0x02);
        assert_eq!(cpu.a, 0x03, "5 - 2 = 3");
    }

    #[test]
    fn anc_copies_bit7_of_and_result_into_carry() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0xFF;
        run_one(&mut cpu, &mut bus, 0x0200, &[0x0B, 0x80]); // ANC #$80
        assert_eq!(cpu.a, 0x80);
        assert_ne!(cpu.p & FLAG_C, 0);
    }

    #[test]
    fn arr_decimal_applies_the_nmos_nibble_corrections() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0x05;
        cpu.p |= FLAG_D;

        run_one(&mut cpu, &mut bus, 0x0200, &[0x6B, 0x05]);

        assert_eq!(cpu.a, 0x08);
        assert_eq!(cpu.p & (FLAG_N | FLAG_V | FLAG_Z | FLAG_C), 0);
    }

    #[test]
    fn sbx_computes_a_and_x_minus_imm_into_x() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        cpu.a = 0xFF;
        cpu.x = 0x0F;
        run_one(&mut cpu, &mut bus, 0x0200, &[0xCB, 0x05]); // SBX #$05
        assert_eq!(cpu.x, 0x0A, "(0xFF & 0x0F) - 0x05 = 0x0A");
    }

    #[test]
    fn illegal_nops_consume_the_right_operand_width_and_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        let c = run_one(&mut cpu, &mut bus, 0x0200, &[0x1A]); // 1-byte NOP
        assert_eq!((c, cpu.pc), (2, 0x0201));

        let c = run_one(&mut cpu, &mut bus, 0x0300, &[0x80, 0x00]); // NOP #imm
        assert_eq!((c, cpu.pc), (2, 0x0302));

        let c = run_one(&mut cpu, &mut bus, 0x0400, &[0x1C, 0x00, 0x00]); // NOP abs,X
        assert_eq!((c, cpu.pc), (4, 0x0403));
    }

    #[test]
    fn every_jam_opcode_halts() {
        for op in [
            0x02, 0x12, 0x22, 0x32, 0x42, 0x52, 0x62, 0x72, 0x92, 0xB2, 0xD2, 0xF2,
        ] {
            let mut cpu = Cpu::new();
            let mut bus = FlatBus::new();
            run_one(&mut cpu, &mut bus, 0x0200, &[op]);
            assert!(cpu.halted, "opcode {op:#04x} should JAM/halt");
        }
    }

    #[test]
    fn irq_is_masked_by_the_i_flag_but_nmi_never_is() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.load(0xFFFE, &[0x00, 0x05]); // IRQ vector -> $0500
        bus.load(0xFFFA, &[0x00, 0x06]); // NMI vector -> $0600
        bus.load(0x0200, &[0xEA]); // NOP -- what a masked IRQ should just execute
        cpu.pc = 0x0200;
        cpu.p |= crate::cpu::FLAG_I; // mask IRQ
        cpu.request_irq();
        cpu.step(&mut bus);
        assert_eq!(
            cpu.pc, 0x0201,
            "masked IRQ should not fire, ordinary fetch happens"
        );

        cpu.pc = 0x0200;
        bus.load(0x0200, &[0xEA]); // NOP, just to have something valid there
        cpu.request_nmi();
        cpu.step(&mut bus);
        assert_eq!(cpu.pc, 0x0600, "NMI must fire even with I set");
    }
}
