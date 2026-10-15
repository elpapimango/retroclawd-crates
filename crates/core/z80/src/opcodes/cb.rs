use super::{Handler, apply_alu_flags, flags_byte, parity, reg_get, reg_set};
use crate::Bus;
use crate::cpu::{Cpu, FLAG_C};

/// 0x00-0x3F: rotate/shift group. Sub-op = (opcode >> 3) & 7, register field
/// = opcode & 7 (standard B,C,D,E,H,L,(HL),A encoding via reg_get/reg_set).
fn rotate_shift<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let subop = (opcode >> 3) & 0x07;
    let idx = opcode & 0x07;

    let val = reg_get(cpu, bus, idx);
    let old_carry = u8::from(cpu.f() & FLAG_C != 0);

    let (result, carry_out) = match subop {
        0 => (val.rotate_left(1), val & 0x80 != 0),     // RLC
        1 => (val.rotate_right(1), val & 0x01 != 0),    // RRC
        2 => ((val << 1) | old_carry, val & 0x80 != 0), // RL
        3 => ((val >> 1) | (old_carry << 7), val & 0x01 != 0), // RR
        4 => (val << 1, val & 0x80 != 0),               // SLA
        5 => ((val >> 1) | (val & 0x80), val & 0x01 != 0), // SRA (sign bit preserved)
        6 => ((val << 1) | 0x01, val & 0x80 != 0),      // SLL (undocumented)
        7 => (val >> 1, val & 0x01 != 0),               // SRL
        _ => unreachable!("rotate/shift sub-op out of range: {subop}"),
    };

    reg_set(cpu, bus, idx, result);
    let flags = flags_byte(
        result & 0x80 != 0,
        result == 0,
        false,
        parity(result),
        false,
        carry_out,
    );
    apply_alu_flags(cpu, flags, result);

    if idx == 6 { 15 } else { 8 }
}

/// 0x40-0x7F: BIT b,r. Tests bit b of r without writing it back.
fn bit<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let b = (opcode >> 3) & 0x07;
    let idx = opcode & 0x07;

    let val = reg_get(cpu, bus, idx);
    let bit_set = val & (1 << b) != 0;
    let z = !bit_set;
    let s = b == 7 && bit_set;
    let old_carry = cpu.f() & FLAG_C != 0;

    // PV mirrors Z on BIT. F3/F5 normally come from the tested value, but
    // for the (HL) form real hardware sources them from the high byte of
    // the internal WZ address latch (HL+1) instead.
    let f3f5_source = if idx == 6 {
        (cpu.hl.wrapping_add(1) >> 8) as u8
    } else {
        val
    };
    let flags = flags_byte(s, z, true, z, false, old_carry);
    apply_alu_flags(cpu, flags, f3f5_source);

    if idx == 6 { 12 } else { 8 }
}

/// 0x80-0xBF: RES b,r. Clears bit b of r. No flags affected.
fn res<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let b = (opcode >> 3) & 0x07;
    let idx = opcode & 0x07;

    let val = reg_get(cpu, bus, idx);
    reg_set(cpu, bus, idx, val & !(1 << b));

    if idx == 6 { 15 } else { 8 }
}

/// 0xC0-0xFF: SET b,r. Sets bit b of r. No flags affected.
fn set<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let b = (opcode >> 3) & 0x07;
    let idx = opcode & 0x07;

    let val = reg_get(cpu, bus, idx);
    reg_set(cpu, bus, idx, val | (1 << b));

    if idx == 6 { 15 } else { 8 }
}

pub(crate) const fn install<B: Bus>(table: [Handler<B>; 256]) -> [Handler<B>; 256] {
    let mut table = table;

    let mut opcode = 0x00usize;
    while opcode <= 0x3F {
        table[opcode] = rotate_shift;
        opcode += 1;
    }
    while opcode <= 0x7F {
        table[opcode] = bit;
        opcode += 1;
    }
    while opcode <= 0xBF {
        table[opcode] = res;
        opcode += 1;
    }
    while opcode <= 0xFF {
        table[opcode] = set;
        opcode += 1;
    }

    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{FLAG_H, FLAG_N, FLAG_PV, FLAG_S, FLAG_Z};
    use crate::test_support::FlatBus;

    fn run(cpu: &mut Cpu, bus: &mut FlatBus, cb_opcode: u8) -> u8 {
        bus.write(0, 0xCB);
        bus.write(1, cb_opcode);
        cpu.pc = 0;
        cpu.step(bus)
    }

    #[test]
    fn rlc_b_rotates_high_bit_into_carry_and_bit0() {
        let mut cpu = Cpu::new();
        cpu.set_b(0x80);
        let mut bus = FlatBus::new();

        let cycles = run(&mut cpu, &mut bus, 0x00); // RLC B

        assert_eq!(cycles, 8);
        assert_eq!(cpu.b(), 0x01);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_Z, 0);
    }

    #[test]
    fn rrc_a_rotates_low_bit_into_carry_and_bit7() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x01);
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x0F); // RRC A

        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
    }

    #[test]
    fn rl_c_shifts_in_old_carry_and_sets_new_carry() {
        let mut cpu = Cpu::new();
        cpu.set_c(0x80);
        cpu.set_f(FLAG_C); // old carry = 1
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x11); // RL C

        assert_eq!(cpu.c(), 0x01); // bit0 <- old carry
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // bit7 -> new carry
    }

    #[test]
    fn rr_d_shifts_in_old_carry_and_sets_new_carry() {
        let mut cpu = Cpu::new();
        cpu.set_d(0x01);
        cpu.set_f(FLAG_C); // old carry = 1
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x1A); // RR D

        assert_eq!(cpu.d(), 0x80); // bit7 <- old carry
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // bit0 -> new carry
    }

    #[test]
    fn sla_e_shifts_zero_into_bit0() {
        let mut cpu = Cpu::new();
        cpu.set_e(0x81);
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x23); // SLA E

        assert_eq!(cpu.e(), 0x02);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // old bit7 -> carry
    }

    #[test]
    fn sra_h_preserves_sign_bit() {
        let mut cpu = Cpu::new();
        cpu.set_h(0x81);
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x2C); // SRA H

        assert_eq!(cpu.h(), 0xC0); // bit7 unchanged (still 1), shifted right
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // old bit0 -> carry
    }

    #[test]
    fn sll_l_shifts_one_into_bit0_undocumented() {
        let mut cpu = Cpu::new();
        cpu.set_l(0x80);
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x35); // SLL L

        assert_eq!(cpu.l(), 0x01); // bit0 forced to 1
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
    }

    #[test]
    fn srl_a_shifts_zero_into_bit7() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x81);
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x3F); // SRL A

        assert_eq!(cpu.a(), 0x40);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // old bit0 -> carry
    }

    #[test]
    fn rlc_hl_indirect_reads_and_writes_memory_and_takes_15_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x2000;
        let mut bus = FlatBus::new();
        bus.write(0x2000, 0x80);

        let cycles = run(&mut cpu, &mut bus, 0x06); // RLC (HL)

        assert_eq!(cycles, 15);
        assert_eq!(bus.read(0x2000), 0x01);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
    }

    #[test]
    fn rotate_result_sets_parity_and_f3_f5_from_result() {
        let mut cpu = Cpu::new();
        cpu.set_b(0x11); // 0001_0001
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x00); // RLC B -> 0x22 (0010_0010): F5 set, F3 clear

        assert_eq!(cpu.b(), 0x22);
        assert_eq!(cpu.f() & crate::cpu::FLAG_F5, crate::cpu::FLAG_F5);
        assert_eq!(cpu.f() & crate::cpu::FLAG_F3, 0);
        // 0x22 = 0010_0010 has 2 set bits -> even parity
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV);
    }

    #[test]
    fn bit_b_r_sets_zero_flag_when_bit_clear() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x00);
        let mut bus = FlatBus::new();

        let cycles = run(&mut cpu, &mut bus, 0x47); // BIT 0,A

        assert_eq!(cycles, 8);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_N, 0);
    }

    #[test]
    fn bit_b_r_clears_zero_flag_when_bit_set() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x01);
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x47); // BIT 0,A

        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_PV, 0);
    }

    #[test]
    fn bit_7_sets_sign_flag_when_bit_set() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x80);
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x7F); // BIT 7,A

        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_Z, 0);
    }

    #[test]
    fn bit_preserves_existing_carry_flag() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x00);
        cpu.set_f(FLAG_C);
        let mut bus = FlatBus::new();

        run(&mut cpu, &mut bus, 0x47); // BIT 0,A

        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
    }

    #[test]
    fn bit_hl_indirect_takes_12_cycles_and_does_not_write() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x3000;
        let mut bus = FlatBus::new();
        bus.write(0x3000, 0xFF);

        let cycles = run(&mut cpu, &mut bus, 0x46); // BIT 0,(HL)

        assert_eq!(cycles, 12);
        assert_eq!(bus.read(0x3000), 0xFF); // untouched
        assert_eq!(cpu.f() & FLAG_Z, 0);
    }

    #[test]
    fn res_clears_bit_and_does_not_affect_flags() {
        let mut cpu = Cpu::new();
        cpu.set_a(0xFF);
        cpu.set_f(0xFF); // all flags pre-set, sentinel
        let mut bus = FlatBus::new();

        let cycles = run(&mut cpu, &mut bus, 0x87); // RES 0,A

        assert_eq!(cycles, 8);
        assert_eq!(cpu.a(), 0xFE);
        assert_eq!(cpu.f(), 0xFF); // completely unaffected
    }

    #[test]
    fn res_hl_indirect_takes_15_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x4000;
        let mut bus = FlatBus::new();
        bus.write(0x4000, 0xFF);

        let cycles = run(&mut cpu, &mut bus, 0x86); // RES 0,(HL)

        assert_eq!(cycles, 15);
        assert_eq!(bus.read(0x4000), 0xFE);
    }

    #[test]
    fn set_sets_bit_and_does_not_affect_flags() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x00);
        cpu.set_f(0x00); // sentinel: no flags set
        let mut bus = FlatBus::new();

        let cycles = run(&mut cpu, &mut bus, 0xC7); // SET 0,A

        assert_eq!(cycles, 8);
        assert_eq!(cpu.a(), 0x01);
        assert_eq!(cpu.f(), 0x00); // completely unaffected
    }

    #[test]
    fn set_hl_indirect_takes_15_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x5000;
        let mut bus = FlatBus::new();
        bus.write(0x5000, 0x00);

        let cycles = run(&mut cpu, &mut bus, 0xC6); // SET 0,(HL)

        assert_eq!(cycles, 15);
        assert_eq!(bus.read(0x5000), 0x01);
    }

    #[test]
    fn set_and_res_cover_every_bit_position() {
        for b in 0..8u8 {
            let mut cpu = Cpu::new();
            let mut bus = FlatBus::new();
            let set_opcode = 0xC0 | (b << 3) | 0x07; // SET b,A
            run(&mut cpu, &mut bus, set_opcode);
            assert_eq!(cpu.a(), 1 << b, "SET {b},A");

            let res_opcode = 0x80 | (b << 3) | 0x07; // RES b,A
            run(&mut cpu, &mut bus, res_opcode);
            assert_eq!(cpu.a(), 0, "RES {b},A");
        }
    }
}
