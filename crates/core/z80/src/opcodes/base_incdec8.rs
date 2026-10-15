use super::{Handler, IndexReg};
use crate::Bus;
use crate::cpu::{Cpu, FLAG_C};

pub(crate) const fn install<B: Bus>(table: [Handler<B>; 256]) -> [Handler<B>; 256] {
    let mut table = table;

    table[0x04] = inc_r;
    table[0x0C] = inc_r;
    table[0x14] = inc_r;
    table[0x1C] = inc_r;
    table[0x24] = inc_r;
    table[0x2C] = inc_r;
    table[0x34] = inc_r;
    table[0x3C] = inc_r;

    table[0x05] = dec_r;
    table[0x0D] = dec_r;
    table[0x15] = dec_r;
    table[0x1D] = dec_r;
    table[0x25] = dec_r;
    table[0x2D] = dec_r;
    table[0x35] = dec_r;
    table[0x3D] = dec_r;

    table
}

/// INC r (0b00rrr100). Reuses reg_get/reg_set for the plain register slots
/// (idx != 6) so H/L -> IXH/IXL/IYH/IYL substitution under a DD/FD prefix
/// comes for free. The (HL)/(IX+d)/(IY+d) slot (idx == 6) is handled by
/// computing the effective address once (see `apply_incdec`) rather than via
/// reg_get+reg_set, since those each independently call `indexed_addr` and
/// would otherwise double-charge the displacement fetch and double-advance
/// `pc` when composed back to back.
fn inc_r<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let idx = (opcode >> 3) & 0x07;
    apply_incdec(cpu, bus, idx, true)
}

/// DEC r (0b00rrr101). Same register-field decoding and addressing as
/// `inc_r`, mirrored for subtract-by-1.
fn dec_r<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let idx = (opcode >> 3) & 0x07;
    apply_incdec(cpu, bus, idx, false)
}

fn apply_incdec<B: Bus>(cpu: &mut Cpu, bus: &mut B, idx: u8, is_inc: bool) -> u8 {
    if idx == 6 {
        // Compute the (HL)/(IX+d)/(IY+d) address exactly once, then read and
        // write through it directly -- see the note on `inc_r` above.
        let addr = match cpu.active_index {
            None => cpu.hl,
            Some(IndexReg::Ix) => super::indexed_addr(cpu, bus, cpu.ix),
            Some(IndexReg::Iy) => super::indexed_addr(cpu, bus, cpu.iy),
        };
        let val = cpu.mem_read(bus, addr);
        let result = incdec_result(cpu, val, is_inc);
        cpu.mem_write(bus, addr, result);
        return 11;
    }

    let val = super::reg_get(cpu, bus, idx);
    let result = incdec_result(cpu, val, is_inc);
    super::reg_set(cpu, bus, idx, result);
    4
}

/// Computes `val +/- 1`, applies the resulting S/Z/H/PV/N flags (preserving
/// the existing C flag, which INC/DEC never touch), and returns the result.
fn incdec_result(cpu: &mut Cpu, val: u8, is_inc: bool) -> u8 {
    let result = if is_inc {
        val.wrapping_add(1)
    } else {
        val.wrapping_sub(1)
    };

    let half = if is_inc {
        val & 0x0F == 0x0F
    } else {
        val & 0x0F == 0x00
    };
    let overflow = if is_inc { val == 0x7F } else { val == 0x80 };
    let current_c = cpu.f() & FLAG_C != 0;
    let flags = super::flags_byte(
        result & 0x80 != 0,
        result == 0,
        half,
        overflow,
        !is_inc,
        current_c,
    );
    super::apply_alu_flags(cpu, flags, result);

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{FLAG_H, FLAG_N, FLAG_PV, FLAG_S, FLAG_Z};
    use crate::test_support::FlatBus;

    #[test]
    fn inc_b_increments_and_takes_4_cycles() {
        let mut cpu = Cpu::new();
        cpu.set_b(0x01);
        let mut bus = FlatBus::new();
        bus.write(0, 0x04); // INC B

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.b(), 0x02);
        assert_eq!(cpu.f() & FLAG_H, 0);
        assert_eq!(cpu.f() & FLAG_PV, 0);
    }

    #[test]
    fn inc_a_preserves_carry_flag() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x01);
        cpu.set_f(FLAG_C); // carry set beforehand
        let mut bus = FlatBus::new();
        bus.write(0, 0x3C); // INC A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x02);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // untouched by INC
    }

    #[test]
    fn inc_a_sets_half_carry_on_nibble_wrap() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x0F);
        let mut bus = FlatBus::new();
        bus.write(0, 0x3C); // INC A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x10);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_PV, 0); // PV not set
    }

    #[test]
    fn inc_a_sets_overflow_on_signed_wrap_from_0x7f() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x7F);
        let mut bus = FlatBus::new();
        bus.write(0, 0x3C); // INC A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // overflow
        assert_eq!(cpu.f() & FLAG_N, 0); // N=0 for INC
    }

    #[test]
    fn inc_a_zero_result_sets_zero_flag_and_wraps_from_0xff() {
        let mut cpu = Cpu::new();
        cpu.set_a(0xFF);
        let mut bus = FlatBus::new();
        bus.write(0, 0x3C); // INC A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H); // 0xF nibble wrap
        assert_eq!(cpu.f() & FLAG_PV, 0); // no overflow
    }

    #[test]
    fn dec_b_decrements_and_takes_4_cycles() {
        let mut cpu = Cpu::new();
        cpu.set_b(0x02);
        let mut bus = FlatBus::new();
        bus.write(0, 0x05); // DEC B

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.b(), 0x01);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N); // N set for DEC
    }

    #[test]
    fn dec_a_preserves_carry_flag() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x02);
        cpu.set_f(FLAG_C);
        let mut bus = FlatBus::new();
        bus.write(0, 0x3D); // DEC A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x01);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // untouched by DEC
    }

    #[test]
    fn dec_a_sets_half_borrow_on_nibble_wrap() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x10);
        let mut bus = FlatBus::new();
        bus.write(0, 0x3D); // DEC A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x0F);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H); // half borrow
        assert_eq!(cpu.f() & FLAG_PV, 0); // PV not set
    }

    #[test]
    fn dec_a_sets_overflow_on_signed_wrap_from_0x80() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x80);
        let mut bus = FlatBus::new();
        bus.write(0, 0x3D); // DEC A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x7F);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // overflow
        assert_eq!(cpu.f() & FLAG_S, 0); // S clear
    }

    #[test]
    fn dec_a_zero_input_wraps_to_0xff_and_sets_sign() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x00);
        let mut bus = FlatBus::new();
        bus.write(0, 0x3D); // DEC A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0xFF);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H); // borrow from bit 4
    }

    #[test]
    fn inc_hl_indirect_reads_and_writes_memory_and_takes_11_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x2000;
        let mut bus = FlatBus::new();
        bus.write(0x2000, 0x41);
        bus.write(0, 0x34); // INC (HL)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 11);
        assert_eq!(bus.read(0x2000), 0x42);
    }

    #[test]
    fn dec_hl_indirect_reads_and_writes_memory_and_takes_11_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x2000;
        let mut bus = FlatBus::new();
        bus.write(0x2000, 0x41);
        bus.write(0, 0x35); // DEC (HL)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 11);
        assert_eq!(bus.read(0x2000), 0x40);
    }

    #[test]
    fn dd_prefixed_inc_ixh_substitutes_ix_high_byte() {
        // DD 24 -- undocumented INC IXH. Exercises reg_get/reg_set's
        // active_index substitution for the H register-field slot.
        let mut cpu = Cpu::new();
        cpu.ix = 0x1200;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x24);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 8); // 4 prefix + 4 base
        assert_eq!(cpu.ixh(), 0x13);
        assert_eq!(cpu.h(), 0); // real HL untouched
    }

    #[test]
    fn dd_prefixed_dec_indirect_targets_ix_plus_displacement_and_takes_23_cycles() {
        // DD 35 d -- DEC (IX+d). Exercises indexed_addr's displacement fetch
        // (+8 T-states) stacked on top of the base 11-cycle memory INC/DEC.
        let mut cpu = Cpu::new();
        cpu.ix = 0x3000;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x35);
        bus.write(2, 0x02); // d = +2
        bus.write(0x3002, 0x01);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 23); // 4 prefix + 11 base + 8 displacement
        assert_eq!(bus.read(0x3002), 0x00);
    }
}
