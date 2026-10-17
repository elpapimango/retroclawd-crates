use super::Handler;
use crate::Bus;
use crate::cpu::{Cpu, FLAG_C, FLAG_PV, FLAG_S, FLAG_Z};

/// Decodes a 3-bit condition-code field (as used by JR/JP/CALL/RET cc) into
/// whether the condition currently holds. Mapping: 0=NZ,1=Z,2=NC,3=C,
/// 4=PO,5=PE,6=P,7=M -- even values test "flag clear", odd values test
/// "flag set", against FLAG_Z/FLAG_C/FLAG_PV/FLAG_S respectively.
fn condition_holds(cpu: &Cpu, cc: u8) -> bool {
    let f = cpu.f();
    match cc {
        0 => f & FLAG_Z == 0,
        1 => f & FLAG_Z != 0,
        2 => f & FLAG_C == 0,
        3 => f & FLAG_C != 0,
        4 => f & FLAG_PV == 0,
        5 => f & FLAG_PV != 0,
        6 => f & FLAG_S == 0,
        7 => f & FLAG_S != 0,
        _ => unreachable!("condition code out of range: {cc}"),
    }
}

/// Fetches the signed displacement byte following a JR-family opcode,
/// advancing `pc` past it, and returns it. The caller applies it relative to
/// the already-advanced `pc` (i.e. relative to the byte *after* the
/// displacement, per Z80 semantics).
fn fetch_displacement<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> i8 {
    let d = cpu.mem_read(bus, cpu.pc) as i8;
    cpu.pc = cpu.pc.wrapping_add(1);
    d
}

fn fetch_addr16<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u16 {
    let lo = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    let hi = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    u16::from_le_bytes([lo, hi])
}

fn push16<B: Bus>(cpu: &mut Cpu, bus: &mut B, val: u16) {
    // PUSH/RST spend one internal T-state (the extended M1) and CALL one (after
    // reading the high address byte) immediately before the first stack write.
    cpu.internal_cycles(bus, 1);
    let [lo, hi] = val.to_le_bytes();
    cpu.sp = cpu.sp.wrapping_sub(1);
    cpu.mem_write(bus, cpu.sp, hi);
    cpu.sp = cpu.sp.wrapping_sub(1);
    cpu.mem_write(bus, cpu.sp, lo);
}

fn pop16<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u16 {
    let lo = cpu.mem_read(bus, cpu.sp);
    cpu.sp = cpu.sp.wrapping_add(1);
    let hi = cpu.mem_read(bus, cpu.sp);
    cpu.sp = cpu.sp.wrapping_add(1);
    u16::from_le_bytes([lo, hi])
}

// --- 0x18 JR e ---

fn jr_e<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let d = fetch_displacement(cpu, bus);
    cpu.pc = cpu.pc.wrapping_add(d as i16 as u16);
    12
}

// --- 0x20/0x28/0x30/0x38 JR cc,e ---

fn jr_cc<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let cc = (opcode >> 3) & 0x03;
    let d = fetch_displacement(cpu, bus);
    if condition_holds(cpu, cc) {
        cpu.pc = cpu.pc.wrapping_add(d as i16 as u16);
        12
    } else {
        7
    }
}

// --- 0x10 DJNZ e ---

fn djnz<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.set_b(cpu.b().wrapping_sub(1));
    // DJNZ's M1 is five T-states long; the extra one comes before the
    // displacement read.
    cpu.internal_cycles(bus, 1);
    let d = fetch_displacement(cpu, bus);
    if cpu.b() != 0 {
        cpu.pc = cpu.pc.wrapping_add(d as i16 as u16);
        13
    } else {
        8
    }
}

// --- 0xC3 JP nn ---

fn jp_nn<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.pc = fetch_addr16(cpu, bus);
    10
}

// --- 0xC2/0xCA/0xD2/0xDA/0xE2/0xEA/0xF2/0xFA JP cc,nn ---

fn jp_cc<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let cc = (opcode >> 3) & 0x07;
    let addr = fetch_addr16(cpu, bus);
    if condition_holds(cpu, cc) {
        cpu.pc = addr;
    }
    10
}

// --- 0xE9 JP (HL)/(IX)/(IY) ---

fn jp_hl<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.pc = cpu.hl_or_index();
    4
}

// --- 0xCD CALL nn ---

fn call_nn<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let addr = fetch_addr16(cpu, bus);
    let ret = cpu.pc;
    push16(cpu, bus, ret);
    cpu.pc = addr;
    17
}

// --- 0xC4/0xCC/0xD4/0xDC/0xE4/0xEC/0xF4/0xFC CALL cc,nn ---

fn call_cc<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let cc = (opcode >> 3) & 0x07;
    let addr = fetch_addr16(cpu, bus);
    if condition_holds(cpu, cc) {
        let ret = cpu.pc;
        push16(cpu, bus, ret);
        cpu.pc = addr;
        17
    } else {
        10
    }
}

// --- 0xC9 RET ---

fn ret<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.pc = pop16(cpu, bus);
    10
}

// --- 0xC0/0xC8/0xD0/0xD8/0xE0/0xE8/0xF0/0xF8 RET cc ---

fn ret_cc<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let cc = (opcode >> 3) & 0x07;
    if condition_holds(cpu, cc) {
        // RET cc's M1 is five T-states long; the extra one precedes the pops.
        cpu.internal_cycles(bus, 1);
        cpu.pc = pop16(cpu, bus);
        11
    } else {
        5
    }
}

// --- 0xC7/0xCF/0xD7/0xDF/0xE7/0xEF/0xF7/0xFF RST n ---

fn rst<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let target = match opcode {
        0xC7 => 0x00,
        0xCF => 0x08,
        0xD7 => 0x10,
        0xDF => 0x18,
        0xE7 => 0x20,
        0xEF => 0x28,
        0xF7 => 0x30,
        0xFF => 0x38,
        _ => unreachable!("RST opcode out of range: 0x{opcode:02X}"),
    };
    let ret = cpu.pc;
    push16(cpu, bus, ret);
    cpu.pc = target;
    11
}

pub(crate) const fn install<B: Bus>(table: [Handler<B>; 256]) -> [Handler<B>; 256] {
    let mut table = table;

    table[0x10] = djnz;
    table[0x18] = jr_e;
    table[0x20] = jr_cc;
    table[0x28] = jr_cc;
    table[0x30] = jr_cc;
    table[0x38] = jr_cc;

    table[0xC3] = jp_nn;
    table[0xC2] = jp_cc;
    table[0xCA] = jp_cc;
    table[0xD2] = jp_cc;
    table[0xDA] = jp_cc;
    table[0xE2] = jp_cc;
    table[0xEA] = jp_cc;
    table[0xF2] = jp_cc;
    table[0xFA] = jp_cc;
    table[0xE9] = jp_hl;

    table[0xCD] = call_nn;
    table[0xC4] = call_cc;
    table[0xCC] = call_cc;
    table[0xD4] = call_cc;
    table[0xDC] = call_cc;
    table[0xE4] = call_cc;
    table[0xEC] = call_cc;
    table[0xF4] = call_cc;
    table[0xFC] = call_cc;

    table[0xC9] = ret;
    table[0xC0] = ret_cc;
    table[0xC8] = ret_cc;
    table[0xD0] = ret_cc;
    table[0xD8] = ret_cc;
    table[0xE0] = ret_cc;
    table[0xE8] = ret_cc;
    table[0xF0] = ret_cc;
    table[0xF8] = ret_cc;

    table[0xC7] = rst;
    table[0xCF] = rst;
    table[0xD7] = rst;
    table[0xDF] = rst;
    table[0xE7] = rst;
    table[0xEF] = rst;
    table[0xF7] = rst;
    table[0xFF] = rst;

    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FlatBus;

    #[test]
    fn jr_e_jumps_relative_forward_and_takes_12_cycles() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x0000;
        let mut bus = FlatBus::new();
        bus.write(0, 0x18); // JR e
        bus.write(1, 0x05); // d = +5

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 12);
        // pc after opcode+operand = 2, +5 = 7
        assert_eq!(cpu.pc, 7);
    }

    #[test]
    fn jr_e_jumps_relative_backward() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x0010;
        let mut bus = FlatBus::new();
        bus.write(0x10, 0x18); // JR e
        bus.write(0x11, (-5i8) as u8); // d = -5

        cpu.step(&mut bus);

        // pc after opcode+operand = 0x12, -5 = 0x0D
        assert_eq!(cpu.pc, 0x0D);
    }

    fn jr_nz_taken_when_zero_flag_clear() {
        let mut cpu = Cpu::new();
        cpu.set_f(0); // Z clear
        let mut bus = FlatBus::new();
        bus.write(0, 0x20); // JR NZ,e
        bus.write(1, 0x03);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 12);
        assert_eq!(cpu.pc, 5);
    }

    fn jr_nz_not_taken_when_zero_flag_set() {
        let mut cpu = Cpu::new();
        cpu.set_f(FLAG_Z);
        let mut bus = FlatBus::new();
        bus.write(0, 0x20); // JR NZ,e
        bus.write(1, 0x03);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(cpu.pc, 2); // just past the instruction, no jump
    }

    fn jr_z_taken_when_zero_flag_set() {
        let mut cpu = Cpu::new();
        cpu.set_f(FLAG_Z);
        let mut bus = FlatBus::new();
        bus.write(0, 0x28); // JR Z,e
        bus.write(1, 0x03);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 12);
        assert_eq!(cpu.pc, 5);
    }

    fn jr_nc_and_c_respect_carry_flag() {
        let mut cpu = Cpu::new();
        cpu.set_f(FLAG_C);
        let mut bus = FlatBus::new();
        bus.write(0, 0x30); // JR NC,e -- not taken, carry set
        bus.write(1, 0x03);
        bus.write(2, 0x38); // JR C,e -- taken, carry set
        bus.write(3, 0x03);

        let c1 = cpu.step(&mut bus);
        assert_eq!(c1, 7);
        assert_eq!(cpu.pc, 2);

        let c2 = cpu.step(&mut bus);
        assert_eq!(c2, 12);
        assert_eq!(cpu.pc, 4 + 3);
    }
    /// Merged (every check kept): `jr_nz_taken_when_zero_flag_clear`, `jr_nz_not_taken_when_zero_flag_set`, `jr_z_taken_when_zero_flag_set`, `jr_nc_and_c_respect_carry_flag`.
    #[test]
    fn jr_conditions() {
        jr_nz_taken_when_zero_flag_clear();
        jr_nz_not_taken_when_zero_flag_set();
        jr_z_taken_when_zero_flag_set();
        jr_nc_and_c_respect_carry_flag();
    }

    #[test]
    fn djnz_decrements_b_and_jumps_when_nonzero() {
        let mut cpu = Cpu::new();
        cpu.set_b(2);
        let mut bus = FlatBus::new();
        bus.write(0, 0x10); // DJNZ e
        bus.write(1, 0x03);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 13);
        assert_eq!(cpu.b(), 1);
        assert_eq!(cpu.pc, 5);
    }

    #[test]
    fn djnz_does_not_jump_when_b_reaches_zero() {
        let mut cpu = Cpu::new();
        cpu.set_b(1);
        let mut bus = FlatBus::new();
        bus.write(0, 0x10); // DJNZ e
        bus.write(1, 0x03);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 8);
        assert_eq!(cpu.b(), 0);
        assert_eq!(cpu.pc, 2);
    }

    #[test]
    fn djnz_does_not_affect_flags() {
        let mut cpu = Cpu::new();
        cpu.set_b(2);
        cpu.set_f(0xFF);
        let mut bus = FlatBus::new();
        bus.write(0, 0x10);
        bus.write(1, 0x00);

        cpu.step(&mut bus);

        assert_eq!(cpu.f(), 0xFF);
    }

    #[test]
    fn jp_nn_sets_pc_unconditionally_and_takes_10_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0xC3); // JP nn
        bus.write(1, 0x34);
        bus.write(2, 0x12);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10);
        assert_eq!(cpu.pc, 0x1234);
    }

    #[test]
    fn jp_cc_always_takes_10_cycles_whether_taken_or_not() {
        let mut cpu = Cpu::new();
        cpu.set_f(FLAG_Z); // NZ condition false
        let mut bus = FlatBus::new();
        bus.write(0, 0xC2); // JP NZ,nn
        bus.write(1, 0x34);
        bus.write(2, 0x12);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10);
        assert_eq!(cpu.pc, 3); // not taken -- pc just past instruction

        let mut cpu2 = Cpu::new();
        cpu2.set_f(0); // NZ condition true
        let mut bus2 = FlatBus::new();
        bus2.write(0, 0xC2);
        bus2.write(1, 0x34);
        bus2.write(2, 0x12);

        let cycles2 = cpu2.step(&mut bus2);

        assert_eq!(cycles2, 10);
        assert_eq!(cpu2.pc, 0x1234);
    }

    #[test]
    fn jp_hl_jumps_to_hl_and_takes_4_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0xABCD;
        let mut bus = FlatBus::new();
        bus.write(0, 0xE9); // JP (HL)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.pc, 0xABCD);
    }

    #[test]
    fn jp_ix_substitutes_index_register_under_dd_prefix() {
        let mut cpu = Cpu::new();
        cpu.ix = 0x9876;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0xE9); // JP (IX)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 8); // 4 base + 4 prefix
        assert_eq!(cpu.pc, 0x9876);
    }

    #[test]
    fn call_nn_pushes_return_address_and_jumps() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x0000;
        cpu.sp = 0x2000;
        let mut bus = FlatBus::new();
        bus.write(0, 0xCD); // CALL nn
        bus.write(1, 0x34);
        bus.write(2, 0x12);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 17);
        assert_eq!(cpu.pc, 0x1234);
        assert_eq!(cpu.sp, 0x1FFE);
        assert_eq!(bus.read(0x1FFE), 0x03); // low byte of return addr
        assert_eq!(bus.read(0x1FFF), 0x00); // high byte of return addr
    }

    #[test]
    fn call_cc_not_taken_skips_push_but_advances_past_operand() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x2000;
        cpu.set_f(FLAG_Z); // NZ false
        let mut bus = FlatBus::new();
        bus.write(0, 0xC4); // CALL NZ,nn
        bus.write(1, 0x34);
        bus.write(2, 0x12);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10);
        assert_eq!(cpu.pc, 3);
        assert_eq!(cpu.sp, 0x2000); // untouched
    }

    #[test]
    fn call_cc_taken_pushes_and_jumps() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x2000;
        cpu.set_f(0); // NZ true
        let mut bus = FlatBus::new();
        bus.write(0, 0xC4); // CALL NZ,nn
        bus.write(1, 0x34);
        bus.write(2, 0x12);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 17);
        assert_eq!(cpu.pc, 0x1234);
        assert_eq!(cpu.sp, 0x1FFE);
    }

    #[test]
    fn ret_pops_return_address_and_takes_10_cycles() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x1FFE;
        let mut bus = FlatBus::new();
        bus.write(0x1FFE, 0x34);
        bus.write(0x1FFF, 0x12);
        bus.write(0, 0xC9); // RET

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10);
        assert_eq!(cpu.pc, 0x1234);
        assert_eq!(cpu.sp, 0x2000);
    }

    #[test]
    fn ret_cc_not_taken_does_not_pop() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x1FFE;
        cpu.set_f(FLAG_Z); // NZ false
        let mut bus = FlatBus::new();
        bus.write(0x1FFE, 0x34);
        bus.write(0x1FFF, 0x12);
        bus.write(0, 0xC0); // RET NZ

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 5);
        assert_eq!(cpu.pc, 1); // just past the 1-byte instruction
        assert_eq!(cpu.sp, 0x1FFE); // untouched
    }

    #[test]
    fn ret_cc_taken_pops_and_jumps() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x1FFE;
        cpu.set_f(0); // NZ true
        let mut bus = FlatBus::new();
        bus.write(0x1FFE, 0x34);
        bus.write(0x1FFF, 0x12);
        bus.write(0, 0xC0); // RET NZ

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 11);
        assert_eq!(cpu.pc, 0x1234);
        assert_eq!(cpu.sp, 0x2000);
    }

    #[test]
    fn rst_pushes_return_address_and_jumps_to_fixed_vector() {
        let cases: [(u8, u16); 8] = [
            (0xC7, 0x00),
            (0xCF, 0x08),
            (0xD7, 0x10),
            (0xDF, 0x18),
            (0xE7, 0x20),
            (0xEF, 0x28),
            (0xF7, 0x30),
            (0xFF, 0x38),
        ];

        for (opcode, target) in cases {
            let mut cpu = Cpu::new();
            cpu.pc = 0x4000;
            cpu.sp = 0x2000;
            let mut bus = FlatBus::new();
            bus.write(0x4000, opcode);

            let cycles = cpu.step(&mut bus);

            assert_eq!(cycles, 11, "opcode 0x{opcode:02X}");
            assert_eq!(cpu.pc, target, "opcode 0x{opcode:02X}");
            assert_eq!(cpu.sp, 0x1FFE, "opcode 0x{opcode:02X}");
            assert_eq!(bus.read(0x1FFE), 0x01, "opcode 0x{opcode:02X}"); // low byte: 0x4001
            assert_eq!(bus.read(0x1FFF), 0x40, "opcode 0x{opcode:02X}"); // high byte: 0x4001
        }
    }

    #[test]
    fn all_condition_codes_map_correctly() {
        // cc: 0=NZ,1=Z,2=NC,3=C,4=PO,5=PE,6=P,7=M
        let cases: [(u8, u8, bool); 8] = [
            (0, 0, true),       // NZ, Z clear -> true
            (1, FLAG_Z, true),  // Z, Z set -> true
            (2, 0, true),       // NC, C clear -> true
            (3, FLAG_C, true),  // C, C set -> true
            (4, 0, true),       // PO, PV clear -> true
            (5, FLAG_PV, true), // PE, PV set -> true
            (6, 0, true),       // P, S clear -> true
            (7, FLAG_S, true),  // M, S set -> true
        ];

        for (cc, f, expected) in cases {
            let mut cpu = Cpu::new();
            cpu.set_f(f);
            assert_eq!(condition_holds(&cpu, cc), expected, "cc={cc} f=0x{f:02X}");
        }
    }
}
