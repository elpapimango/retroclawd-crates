use super::{IndexedCbHandler, apply_alu_flags, flags_byte, parity, real_reg_set};
use crate::Bus;
use crate::cpu::{Cpu, FLAG_C};

pub(crate) const fn install<B: Bus>(
    table: [IndexedCbHandler<B>; 256],
) -> [IndexedCbHandler<B>; 256] {
    let mut table = table;

    let mut opcode = 0x00usize;
    while opcode <= 0x3F {
        table[opcode] = rot_shift_indexed_cb;
        opcode += 1;
    }

    let mut opcode = 0x40usize;
    while opcode <= 0x7F {
        table[opcode] = bit_indexed_cb;
        opcode += 1;
    }

    let mut opcode = 0x80usize;
    while opcode <= 0xBF {
        table[opcode] = res_indexed_cb;
        opcode += 1;
    }

    let mut opcode = 0xC0usize;
    while opcode <= 0xFF {
        table[opcode] = set_indexed_cb;
        opcode += 1;
    }

    table
}

/// Recovers the DD/FD-CB sub-opcode byte that selected this handler. By the
/// time a handler runs, `Cpu::step` has already advanced `pc` past it (the
/// same "read pc-1" trick `ld_r_r`/`alu_a_r` use in `opcodes/mod.rs`).
fn sub_opcode<B: Bus>(cpu: &Cpu, bus: &mut B) -> u8 {
    bus.read(cpu.pc.wrapping_sub(1))
}

/// Rotate/shift group (opcodes 0x00-0x3F): RLC/RRC/RL/RR/SLA/SRA/SLL/SRL on
/// `(IX+d)`/`(IY+d)`. Undocumented quirk: when the opcode's low 3 bits select
/// a register (i.e. aren't 6), the result is *also* copied into that
/// register -- the real B/C/D/E/H/L/A, so 4 and 5 are H and L, never
/// IXH/IXL (the prefix does not substitute here).
fn rot_shift_indexed_cb<B: Bus>(cpu: &mut Cpu, bus: &mut B, d: i8) -> u8 {
    let opcode = sub_opcode(cpu, bus);
    let sub_op = (opcode >> 3) & 0x07;
    let low3 = opcode & 0x07;

    let addr = cpu.hl_or_index().wrapping_add(d as i16 as u16);
    let value = cpu.mem_read(bus, addr);

    let (result, carry_out) = match sub_op {
        0 => (value.rotate_left(1), value & 0x80 != 0), // RLC
        1 => (value.rotate_right(1), value & 0x01 != 0), // RRC
        2 => {
            let carry_in = u8::from(cpu.f() & FLAG_C != 0);
            ((value << 1) | carry_in, value & 0x80 != 0) // RL
        }
        3 => {
            let carry_in = u8::from(cpu.f() & FLAG_C != 0);
            ((value >> 1) | (carry_in << 7), value & 0x01 != 0) // RR
        }
        4 => (value << 1, value & 0x80 != 0), // SLA
        5 => ((value >> 1) | (value & 0x80), value & 0x01 != 0), // SRA (sign-preserving)
        6 => ((value << 1) | 1, value & 0x80 != 0), // SLL, undocumented
        7 => (value >> 1, value & 0x01 != 0), // SRL
        _ => unreachable!("rotate/shift sub-op out of range: {sub_op}"),
    };

    cpu.mem_write(bus, addr, result);
    if low3 != 6 {
        real_reg_set(cpu, low3, result);
    }

    let flags = flags_byte(
        result & 0x80 != 0,
        result == 0,
        false,
        parity(result),
        false,
        carry_out,
    );
    apply_alu_flags(cpu, flags, result);

    19
}

/// BIT b,(IX+d)/(IY+d) (opcodes 0x40-0x7F). Never writes memory or a
/// register.
fn bit_indexed_cb<B: Bus>(cpu: &mut Cpu, bus: &mut B, d: i8) -> u8 {
    let opcode = sub_opcode(cpu, bus);
    let b = (opcode >> 3) & 0x07;

    let addr = cpu.hl_or_index().wrapping_add(d as i16 as u16);
    let value = cpu.mem_read(bus, addr);

    let bit_set = value & (1 << b) != 0;
    let z = !bit_set;
    let s = b == 7 && bit_set;
    let carry_preserved = cpu.f() & FLAG_C != 0;

    let flags = flags_byte(s, z, true, z, false, carry_preserved);
    // Undocumented: F3/F5 come from the high byte of the internal WZ latch,
    // which for the indexed-CB BIT form is addr+1, not from the tested
    // value or the address itself. Approximated directly from addr+1's high
    // byte (matches real hardware for this form).
    let f3f5_source = (addr.wrapping_add(1) >> 8) as u8;
    apply_alu_flags(cpu, flags, f3f5_source);

    16
}

/// RES b,(IX+d)/(IY+d) (opcodes 0x80-0xBF). Same undocumented copy-to-register
/// quirk as the rotate/shift group. No flags affected.
fn res_indexed_cb<B: Bus>(cpu: &mut Cpu, bus: &mut B, d: i8) -> u8 {
    let opcode = sub_opcode(cpu, bus);
    let b = (opcode >> 3) & 0x07;
    let low3 = opcode & 0x07;

    let addr = cpu.hl_or_index().wrapping_add(d as i16 as u16);
    let value = cpu.mem_read(bus, addr);
    let result = value & !(1 << b);

    cpu.mem_write(bus, addr, result);
    if low3 != 6 {
        real_reg_set(cpu, low3, result);
    }

    19
}

/// SET b,(IX+d)/(IY+d) (opcodes 0xC0-0xFF). Same undocumented copy-to-register
/// quirk as the rotate/shift group. No flags affected.
fn set_indexed_cb<B: Bus>(cpu: &mut Cpu, bus: &mut B, d: i8) -> u8 {
    let opcode = sub_opcode(cpu, bus);
    let b = (opcode >> 3) & 0x07;
    let low3 = opcode & 0x07;

    let addr = cpu.hl_or_index().wrapping_add(d as i16 as u16);
    let value = cpu.mem_read(bus, addr);
    let result = value | (1 << b);

    cpu.mem_write(bus, addr, result);
    if low3 != 6 {
        real_reg_set(cpu, low3, result);
    }

    19
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{FLAG_N, FLAG_PV, FLAG_S, FLAG_Z};
    use crate::opcodes::{IndexReg, indexed_cb_table};
    use crate::test_support::FlatBus;

    #[test]
    fn rlc_ix_d_updates_memory_and_copies_to_undocumented_target_register() {
        // DD CB 05 00 -- RLC (IX+5), undocumented copy into B (low3 == 0).
        let mut cpu = Cpu::new();
        cpu.active_index = Some(IndexReg::Ix);
        cpu.ix = 0x2000;
        let mut bus = FlatBus::new();
        bus.write(9, 0x00); // opcode byte, recovered via pc-1
        bus.write(0x2005, 0x81); // 1000_0001
        cpu.pc = 10;

        let cycles = indexed_cb_table::<FlatBus>()[0x00](&mut cpu, &mut bus, 5);

        assert_eq!(cycles, 19);
        assert_eq!(bus.read(0x2005), 0x03); // rotated left through carry: 0000_0011
        assert_eq!(cpu.b(), 0x03); // undocumented copy
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // bit 7 of the original value
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_S, 0);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // 0x03 has even parity
        assert_eq!(cpu.f() & FLAG_N, 0);
    }

    /// The undocumented copy target for low3 = 4 and 5 is the real H and L, also under a DD/FD prefix -- not
    /// IXH/IXL (Sean Young, "The Undocumented Z80 Documented" 4.1; FUSE's z80_ddfdcb.c writes H and L).
    #[test]
    fn the_copy_target_for_4_and_5_is_the_real_h_and_l_not_ixh_ixl() {
        for (index, sub, expect_h, expect_l) in [
            (IndexReg::Ix, 0x04u8, true, false), // RLC (IX+d),H
            (IndexReg::Ix, 0x05, false, true),   // RLC (IX+d),L
            (IndexReg::Iy, 0x84, true, false),   // RES 0,(IY+d),H
            (IndexReg::Iy, 0xC5, false, true),   // SET 0,(IY+d),L
        ] {
            let mut cpu = Cpu::new();
            cpu.active_index = Some(index);
            cpu.ix = 0x2000;
            cpu.iy = 0x2000;
            cpu.set_h(0x11);
            cpu.set_l(0x22);
            let mut bus = FlatBus::new();
            bus.write(9, sub);
            bus.write(0x2003, 0x81);
            cpu.pc = 10;

            indexed_cb_table::<FlatBus>()[sub as usize](&mut cpu, &mut bus, 3);

            let memory = bus.read(0x2003);
            assert_eq!(cpu.h() == memory, expect_h, "{index:?} {sub:#04X}: H");
            assert_eq!(cpu.l() == memory, expect_l, "{index:?} {sub:#04X}: L");
            if !expect_h {
                assert_eq!(cpu.h(), 0x11, "{index:?} {sub:#04X}: H untouched");
            }
            if !expect_l {
                assert_eq!(cpu.l(), 0x22, "{index:?} {sub:#04X}: L untouched");
            }
            // The index register itself is the address base and is never the target.
            assert_eq!((cpu.ix, cpu.iy), (0x2000, 0x2000), "{index:?} {sub:#04X}");
        }
    }

    #[test]
    fn rot_shift_with_low3_6_writes_memory_only_no_register_copy() {
        // DD CB 00 06 -- RLC (IX+0), low3 == 6 -- memory only, no copy.
        let mut cpu = Cpu::new();
        cpu.active_index = Some(IndexReg::Ix);
        cpu.ix = 0x4000;
        let mut bus = FlatBus::new();
        bus.write(9, 0x06);
        bus.write(0x4000, 0x01);
        cpu.pc = 10;

        indexed_cb_table::<FlatBus>()[0x06](&mut cpu, &mut bus, 0);

        assert_eq!(bus.read(0x4000), 0x02);
        // All registers stay at their default zero value -- nothing was
        // spuriously copied anywhere.
        assert_eq!(cpu.bc, 0);
        assert_eq!(cpu.de, 0);
        assert_eq!(cpu.hl, 0);
        assert_eq!(cpu.a(), 0);
    }

    #[test]
    fn sra_ix_d_preserves_sign_bit() {
        // DD CB d 2E -- SRA (IX+d), low3 == 6.
        let mut cpu = Cpu::new();
        cpu.active_index = Some(IndexReg::Ix);
        cpu.ix = 0x2000;
        let mut bus = FlatBus::new();
        bus.write(9, 0x2E);
        bus.write(0x2003, 0x81); // 1000_0001
        cpu.pc = 10;

        indexed_cb_table::<FlatBus>()[0x2E](&mut cpu, &mut bus, 3);

        assert_eq!(bus.read(0x2003), 0xC0); // 1100_0000 -- sign bit preserved
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // bit 0 of the original value
    }

    #[test]
    fn bit_indexed_cb_sets_zero_flag_without_writing_memory() {
        // DD CB d 5E -- BIT 3,(IX+d), bit 3 clear -> Z set.
        let mut cpu = Cpu::new();
        cpu.active_index = Some(IndexReg::Ix);
        cpu.ix = 0x5000;
        let mut bus = FlatBus::new();
        bus.write(9, 0x5E);
        bus.write(0x5002, 0xF7); // all bits set except bit 3
        cpu.pc = 10;

        let cycles = indexed_cb_table::<FlatBus>()[0x5E](&mut cpu, &mut bus, 2);

        assert_eq!(cycles, 16);
        assert_eq!(bus.read(0x5002), 0xF7); // memory untouched
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // PV mirrors Z for BIT
        assert_eq!(cpu.f() & FLAG_N, 0);
    }

    #[test]
    fn bit_indexed_cb_bit_7_set_also_sets_sign_flag() {
        // DD CB d 7E -- BIT 7,(IX+d), bit 7 set -> Z clear, S set.
        let mut cpu = Cpu::new();
        cpu.active_index = Some(IndexReg::Ix);
        cpu.ix = 0x5000;
        let mut bus = FlatBus::new();
        bus.write(9, 0x7E);
        bus.write(0x5002, 0x80);
        cpu.pc = 10;

        indexed_cb_table::<FlatBus>()[0x7E](&mut cpu, &mut bus, 2);

        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
    }

    #[test]
    fn res_clears_bit_and_copies_to_undocumented_target_register() {
        // DD CB d 87 -- RES 0,(IX+d), copy into A (low3 == 7).
        let mut cpu = Cpu::new();
        cpu.active_index = Some(IndexReg::Ix);
        cpu.ix = 0x6000;
        let mut bus = FlatBus::new();
        bus.write(9, 0x87);
        bus.write(0x6001, 0xFF);
        cpu.pc = 10;

        let cycles = indexed_cb_table::<FlatBus>()[0x87](&mut cpu, &mut bus, 1);

        assert_eq!(cycles, 19);
        assert_eq!(bus.read(0x6001), 0xFE);
        assert_eq!(cpu.a(), 0xFE);
    }

    #[test]
    fn set_sets_bit_and_copies_to_undocumented_target_register() {
        // DD CB d E1 -- SET 4,(IX+d), copy into C (low3 == 1).
        let mut cpu = Cpu::new();
        cpu.active_index = Some(IndexReg::Ix);
        cpu.ix = 0x6000;
        let mut bus = FlatBus::new();
        bus.write(9, 0xE1);
        bus.write(0x6001, 0x00);
        cpu.pc = 10;

        let cycles = indexed_cb_table::<FlatBus>()[0xE1](&mut cpu, &mut bus, 1);

        assert_eq!(cycles, 19);
        assert_eq!(bus.read(0x6001), 0x10);
        assert_eq!(cpu.c(), 0x10);
    }

    #[test]
    fn dd_cb_full_dispatch_reads_displacement_via_step() {
        // DD CB 02 EE -- SET 5,(IX+2), end-to-end through Cpu::step to prove
        // the displacement plumbing from step() (not just direct handler
        // calls) is wired correctly.
        let mut cpu = Cpu::new();
        cpu.ix = 0x3000;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0xCB);
        bus.write(2, 0x02); // d = +2
        bus.write(3, 0xEE); // SET 5,(IX+d), low3 == 6
        bus.write(0x3002, 0x00);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 23); // 4 (DD prefix) + 19
        assert_eq!(bus.read(0x3002), 0x20);
        assert_eq!(cpu.pc, 4);
    }

    #[test]
    fn fd_cb_full_dispatch_with_negative_displacement_via_step() {
        // FD CB FE 1E -- RR (IY-2), end-to-end, negative displacement.
        let mut cpu = Cpu::new();
        cpu.iy = 0x4010;
        cpu.set_f(FLAG_C); // carry-in feeds bit 7 of the result
        let mut bus = FlatBus::new();
        bus.write(0, 0xFD);
        bus.write(1, 0xCB);
        bus.write(2, 0xFE); // d = -2
        bus.write(3, 0x1E); // RR (IY+d), low3 == 6
        bus.write(0x400E, 0x00);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 23);
        assert_eq!(bus.read(0x400E), 0x80); // carry-in rotated into bit 7
        assert_eq!(cpu.f() & FLAG_C, 0); // bit 0 of the original value (0) shifted out
        assert_eq!(cpu.pc, 4);
    }
}
