use super::Handler;
use crate::Bus;
use crate::cpu::{Cpu, FLAG_C, FLAG_F3, FLAG_F5, FLAG_H, FLAG_PV, FLAG_S, FLAG_Z};

pub(crate) const fn install<B: Bus>(table: [Handler<B>; 256]) -> [Handler<B>; 256] {
    let mut table = table;

    table[0x03] = inc_ss;
    table[0x13] = inc_ss;
    table[0x23] = inc_ss;
    table[0x33] = inc_ss;

    table[0x0B] = dec_ss;
    table[0x1B] = dec_ss;
    table[0x2B] = dec_ss;
    table[0x3B] = dec_ss;

    table[0x09] = add_hl_ss;
    table[0x19] = add_hl_ss;
    table[0x29] = add_hl_ss;
    table[0x39] = add_hl_ss;

    table
}

/// Decodes the `ss` field (bits 4-5) shared by INC ss / DEC ss / ADD HL,ss
/// from the opcode byte just consumed.
fn ss_field<B: Bus>(cpu: &Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    (opcode >> 4) & 0x03
}

/// INC BC/DE/HL(or IX/IY)/SP -- 0x03/0x13/0x23/0x33. No flags affected.
fn inc_ss<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    match ss_field(cpu, bus) {
        0 => cpu.bc = cpu.bc.wrapping_add(1),
        1 => cpu.de = cpu.de.wrapping_add(1),
        2 => cpu.set_hl_or_index(cpu.hl_or_index().wrapping_add(1)),
        3 => cpu.sp = cpu.sp.wrapping_add(1),
        _ => unreachable!("ss field out of range"),
    }
    6
}

/// DEC BC/DE/HL(or IX/IY)/SP -- 0x0B/0x1B/0x2B/0x3B. No flags affected.
fn dec_ss<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    match ss_field(cpu, bus) {
        0 => cpu.bc = cpu.bc.wrapping_sub(1),
        1 => cpu.de = cpu.de.wrapping_sub(1),
        2 => cpu.set_hl_or_index(cpu.hl_or_index().wrapping_sub(1)),
        3 => cpu.sp = cpu.sp.wrapping_sub(1),
        _ => unreachable!("ss field out of range"),
    }
    6
}

/// ADD HL,ss (or ADD IX/IY,ss under a DD/FD prefix) -- 0x09/0x19/0x29/0x39.
/// Dest and, for ss=2, the operand are both `hl_or_index()`. Only H, N, C
/// are affected; S, Z, PV are left untouched.
fn add_hl_ss<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let ss = ss_field(cpu, bus);
    let hl = cpu.hl_or_index();
    let operand = match ss {
        0 => cpu.bc,
        1 => cpu.de,
        2 => cpu.hl_or_index(),
        3 => cpu.sp,
        _ => unreachable!("ss field out of range"),
    };

    let sum = hl as u32 + operand as u32;
    let result = sum as u16;
    let half = ((hl & 0x0FFF) + (operand & 0x0FFF)) > 0x0FFF;
    let carry = sum > 0xFFFF;

    let preserved = cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV);
    let mut f = preserved;
    if half {
        f |= FLAG_H;
    }
    if carry {
        f |= FLAG_C;
    }
    // N = 0 (ADD).
    let high_byte = (result >> 8) as u8;
    f |= high_byte & (FLAG_F3 | FLAG_F5);
    cpu.set_f(f);

    cpu.set_hl_or_index(result);

    11
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::FLAG_N;
    use crate::opcodes::IndexReg;
    use crate::test_support::FlatBus;

    #[test]
    fn inc_bc_wraps_and_takes_6_cycles() {
        let mut cpu = Cpu::new();
        cpu.bc = 0xFFFF;
        let mut bus = FlatBus::new();
        bus.write(0, 0x03); // INC BC

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 6);
        assert_eq!(cpu.bc, 0x0000);
        assert_eq!(cpu.f(), 0); // no flags touched
    }

    #[test]
    fn dec_de_wraps_and_takes_6_cycles() {
        let mut cpu = Cpu::new();
        cpu.de = 0x0000;
        let mut bus = FlatBus::new();
        bus.write(0, 0x1B); // DEC DE

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 6);
        assert_eq!(cpu.de, 0xFFFF);
        assert_eq!(cpu.f(), 0);
    }

    #[test]
    fn inc_hl_increments_real_hl_without_prefix() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x1234;
        let mut bus = FlatBus::new();
        bus.write(0, 0x23); // INC HL

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 6);
        assert_eq!(cpu.hl, 0x1235);
    }

    #[test]
    fn dd_prefixed_inc_hl_opcode_increments_ix_instead() {
        let mut cpu = Cpu::new();
        cpu.ix = 0x2000;
        cpu.hl = 0x9999;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x23); // INC IX

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10); // 4 prefix + 6 base
        assert_eq!(cpu.ix, 0x2001);
        assert_eq!(cpu.hl, 0x9999); // untouched
    }

    #[test]
    fn dec_sp_decrements_stack_pointer() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x0001;
        let mut bus = FlatBus::new();
        bus.write(0, 0x3B); // DEC SP

        cpu.step(&mut bus);

        assert_eq!(cpu.sp, 0x0000);
    }

    #[test]
    fn add_hl_bc_sets_half_and_full_carry_and_preserves_szpv() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x0FFF;
        cpu.bc = 0x0001;
        cpu.set_f(FLAG_S | FLAG_Z | FLAG_PV); // sentinel: must survive untouched
        let mut bus = FlatBus::new();
        bus.write(0, 0x09); // ADD HL,BC

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 11);
        assert_eq!(cpu.hl, 0x1000);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H); // carry out of bit 11
        assert_eq!(cpu.f() & FLAG_C, 0); // no carry out of bit 15
        assert_eq!(cpu.f() & FLAG_N, 0);
        // S/Z/PV preserved from before the instruction.
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV);
    }

    #[test]
    fn add_hl_sp_sets_carry_on_overflow_past_0xffff() {
        let mut cpu = Cpu::new();
        cpu.hl = 0xFFFF;
        cpu.sp = 0x0002;
        let mut bus = FlatBus::new();
        bus.write(0, 0x39); // ADD HL,SP

        cpu.step(&mut bus);

        assert_eq!(cpu.hl, 0x0001);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
    }

    #[test]
    fn add_hl_hl_doubles_hl_and_f3_f5_come_from_result_high_byte() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x1128; // high byte after doubling: 0x22 -> bit5 set, bit3 clear
        let mut bus = FlatBus::new();
        bus.write(0, 0x29); // ADD HL,HL

        cpu.step(&mut bus);

        assert_eq!(cpu.hl, 0x2250);
        assert_eq!(cpu.f() & FLAG_F5, FLAG_F5);
        assert_eq!(cpu.f() & FLAG_F3, 0);
    }

    #[test]
    fn dd_prefixed_add_ix_ix_uses_ix_for_both_operands() {
        // DD 29 -- ADD IX,IX. Both dest and the ss=2 operand resolve to IX,
        // not a mix of IX and the real HL.
        let mut cpu = Cpu::new();
        cpu.ix = 0x1111;
        cpu.hl = 0x0001; // must stay untouched and uninvolved
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x29); // ADD IX,IX

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 15); // 4 prefix + 11 base
        assert_eq!(cpu.ix, 0x2222);
        assert_eq!(cpu.hl, 0x0001);
    }

    #[test]
    fn fd_prefixed_add_iy_de_adds_de_into_iy() {
        let mut cpu = Cpu::new();
        cpu.iy = 0x4000;
        cpu.de = 0x0100;
        let mut bus = FlatBus::new();
        bus.write(0, 0xFD);
        bus.write(1, 0x19); // ADD IY,DE

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 15);
        assert_eq!(cpu.iy, 0x4100);
        assert_eq!(cpu.hl, 0x0000); // real HL untouched
    }

    #[test]
    fn dd_prefixed_dec_ix_decrements_index_register() {
        let mut cpu = Cpu::new();
        cpu.ix = 0x0000;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x2B); // DEC IX

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10);
        assert_eq!(cpu.ix, 0xFFFF);
    }

    // Sanity check that IndexReg is reachable via super:: as documented,
    // even though this file doesn't need to construct it directly elsewhere.
    #[test]
    fn active_index_is_none_outside_dispatch() {
        let cpu = Cpu::new();
        assert_eq!(cpu.active_index, None::<IndexReg>);
    }
}
