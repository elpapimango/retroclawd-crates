use super::Handler;
use crate::Bus;
use crate::cpu::{Cpu, FLAG_C, FLAG_F3, FLAG_F5, FLAG_H, FLAG_N, FLAG_PV, FLAG_S, FLAG_Z};

pub(crate) const fn install<B: Bus>(table: [Handler<B>; 256]) -> [Handler<B>; 256] {
    let mut table = table;

    table[0x76] = halt;

    table[0x08] = ex_af_af;
    table[0xD9] = exx;
    table[0xEB] = ex_de_hl;
    table[0xE3] = ex_sp_hl;

    table[0xC5] = push_bc;
    table[0xD5] = push_de;
    table[0xE5] = push_hl;
    table[0xF5] = push_af;

    table[0xC1] = pop_bc;
    table[0xD1] = pop_de;
    table[0xE1] = pop_hl;
    table[0xF1] = pop_af;

    table[0x07] = rlca;
    table[0x0F] = rrca;
    table[0x17] = rla;
    table[0x1F] = rra;

    table[0x27] = daa;
    table[0x2F] = cpl;
    table[0x37] = scf;
    table[0x3F] = ccf;

    table[0xF3] = di;
    table[0xFB] = ei;

    table[0xF9] = ld_sp_hl;

    table[0xC6] = add_a_n;
    table[0xCE] = adc_a_n;
    table[0xD6] = sub_n;
    table[0xDE] = sbc_a_n;
    table[0xE6] = and_n;
    table[0xEE] = xor_n;
    table[0xF6] = or_n;
    table[0xFE] = cp_n;

    table[0xD3] = out_n_a;
    table[0xDB] = in_a_n;

    table
}

fn fetch_byte<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let val = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    val
}

fn push16<B: Bus>(cpu: &mut Cpu, bus: &mut B, val: u16) {
    // PUSH spends one internal T-state (the extended M1) before the first
    // stack write.
    cpu.internal_cycles(bus, 1);
    let hi = (val >> 8) as u8;
    let lo = val as u8;
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
    ((hi as u16) << 8) | lo as u16
}

fn halt<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.halted = true;
    4
}

fn ex_af_af<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    std::mem::swap(&mut cpu.af, &mut cpu.af_);
    4
}

fn exx<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    std::mem::swap(&mut cpu.bc, &mut cpu.bc_);
    std::mem::swap(&mut cpu.de, &mut cpu.de_);
    std::mem::swap(&mut cpu.hl, &mut cpu.hl_);
    4
}

fn ex_de_hl<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    // Documented exception: always the real DE/HL, never IX/IY even under
    // an active DD/FD prefix.
    std::mem::swap(&mut cpu.de, &mut cpu.hl);
    4
}

fn ex_sp_hl<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let sp = cpu.sp;
    let mem_lo = cpu.mem_read(bus, sp);
    let mem_hi = cpu.mem_read(bus, sp.wrapping_add(1));
    let mem_val = ((mem_hi as u16) << 8) | mem_lo as u16;

    let reg_val = cpu.hl_or_index();
    cpu.mem_write(bus, sp, reg_val as u8);
    cpu.mem_write(bus, sp.wrapping_add(1), (reg_val >> 8) as u8);
    cpu.set_hl_or_index(mem_val);

    19
}

fn push_bc<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    push16(cpu, bus, cpu.bc);
    11
}

fn push_de<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    push16(cpu, bus, cpu.de);
    11
}

fn push_hl<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let val = cpu.hl_or_index();
    push16(cpu, bus, val);
    11
}

fn push_af<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    push16(cpu, bus, cpu.af);
    11
}

fn pop_bc<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.bc = pop16(cpu, bus);
    10
}

fn pop_de<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.de = pop16(cpu, bus);
    10
}

fn pop_hl<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let val = pop16(cpu, bus);
    cpu.set_hl_or_index(val);
    10
}

fn pop_af<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.af = pop16(cpu, bus);
    10
}

fn rlca<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let a = cpu.a();
    let carry = a & 0x80 != 0;
    let result = a.rotate_left(1);
    cpu.set_a(result);

    let mut f = cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV);
    if carry {
        f |= FLAG_C;
    }
    f |= result & (FLAG_F3 | FLAG_F5);
    cpu.set_f(f);
    4
}

fn rrca<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let a = cpu.a();
    let carry = a & 0x01 != 0;
    let result = a.rotate_right(1);
    cpu.set_a(result);

    let mut f = cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV);
    if carry {
        f |= FLAG_C;
    }
    f |= result & (FLAG_F3 | FLAG_F5);
    cpu.set_f(f);
    4
}

fn rla<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let a = cpu.a();
    let old_carry = cpu.f() & FLAG_C != 0;
    let new_carry = a & 0x80 != 0;
    let result = (a << 1) | u8::from(old_carry);
    cpu.set_a(result);

    let mut f = cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV);
    if new_carry {
        f |= FLAG_C;
    }
    f |= result & (FLAG_F3 | FLAG_F5);
    cpu.set_f(f);
    4
}

fn rra<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let a = cpu.a();
    let old_carry = cpu.f() & FLAG_C != 0;
    let new_carry = a & 0x01 != 0;
    let result = (a >> 1) | (u8::from(old_carry) << 7);
    cpu.set_a(result);

    let mut f = cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV);
    if new_carry {
        f |= FLAG_C;
    }
    f |= result & (FLAG_F3 | FLAG_F5);
    cpu.set_f(f);
    4
}

fn daa<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let a = cpu.a();
    let f = cpu.f();
    let n_flag = f & FLAG_N != 0;
    let h_flag = f & FLAG_H != 0;
    let c_flag = f & FLAG_C != 0;

    let mut correction: u8 = 0;
    let mut carry = c_flag;

    if h_flag || (a & 0x0F) > 9 {
        correction |= 0x06;
    }
    if c_flag || a > 0x99 {
        correction |= 0x60;
        carry = true;
    }

    let (result, half) = if n_flag {
        (a.wrapping_sub(correction), h_flag && (a & 0x0F) < 6)
    } else {
        (a.wrapping_add(correction), (a & 0x0F) > 9)
    };

    cpu.set_a(result);
    let flags = super::flags_byte(
        result & 0x80 != 0,
        result == 0,
        half,
        super::parity(result),
        n_flag,
        carry,
    );
    super::apply_alu_flags(cpu, flags, result);
    4
}

fn cpl<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let result = !cpu.a();
    cpu.set_a(result);

    let mut f = cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV | FLAG_C);
    f |= FLAG_H | FLAG_N;
    f |= result & (FLAG_F3 | FLAG_F5);
    cpu.set_f(f);
    4
}

fn scf<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let mut f = cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV);
    f |= FLAG_C;
    f |= cpu.a() & (FLAG_F3 | FLAG_F5);
    cpu.set_f(f);
    4
}

fn ccf<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let old_carry = cpu.f() & FLAG_C != 0;
    let mut f = cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV);
    if old_carry {
        // H takes the OLD carry value.
        f |= FLAG_H;
    } else {
        f |= FLAG_C;
    }
    f |= cpu.a() & (FLAG_F3 | FLAG_F5);
    cpu.set_f(f);
    4
}

fn di<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.iff1 = false;
    cpu.iff2 = false;
    4
}

fn ei<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.iff1 = true;
    cpu.iff2 = true;
    cpu.ei_delay = true;
    4
}

fn ld_sp_hl<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.sp = cpu.hl_or_index();
    6
}

fn add_a_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    super::add_a(cpu, n, 0);
    7
}

fn adc_a_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    let carry_in = u8::from(cpu.f() & FLAG_C != 0);
    super::add_a(cpu, n, carry_in);
    7
}

fn sub_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    super::sub_a(cpu, n, 0);
    7
}

fn sbc_a_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    let carry_in = u8::from(cpu.f() & FLAG_C != 0);
    super::sub_a(cpu, n, carry_in);
    7
}

fn and_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    super::and_a(cpu, n);
    7
}

fn xor_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    super::xor_a(cpu, n);
    7
}

fn or_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    super::or_a(cpu, n);
    7
}

fn cp_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    super::cp_a(cpu, n);
    7
}

fn out_n_a<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    let port = ((cpu.a() as u16) << 8) | n as u16;
    cpu.io_write(bus, port, cpu.a());
    11
}

fn in_a_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_byte(cpu, bus);
    let port = ((cpu.a() as u16) << 8) | n as u16;
    let val = cpu.io_read(bus, port);
    cpu.set_a(val);
    11
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FlatBus;

    /// A FlatBus variant that also records I/O reads/writes, for OUT/IN tests.
    struct IoBus {
        mem: [u8; 0x10000],
        io: [u8; 0x10000],
        last_out_port: Option<u16>,
        last_out_val: Option<u8>,
    }

    impl IoBus {
        fn new() -> Self {
            Self {
                mem: [0; 0x10000],
                io: [0; 0x10000],
                last_out_port: None,
                last_out_val: None,
            }
        }
    }

    impl Bus for IoBus {
        fn read(&mut self, addr: u16) -> u8 {
            self.mem[addr as usize]
        }
        fn write(&mut self, addr: u16, val: u8) {
            self.mem[addr as usize] = val;
        }
        fn read_io(&mut self, port: u16) -> u8 {
            self.io[port as usize]
        }
        fn write_io(&mut self, port: u16, val: u8) {
            self.io[port as usize] = val;
            self.last_out_port = Some(port);
            self.last_out_val = Some(val);
        }
    }

    #[test]
    fn halt_sets_halted_and_takes_4_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x76);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert!(cpu.halted);
        assert_eq!(cpu.pc, 1);
    }

    #[test]
    fn ex_af_af_swaps_shadow_registers() {
        let mut cpu = Cpu::new();
        cpu.af = 0x1234;
        cpu.af_ = 0x5678;
        let mut bus = FlatBus::new();
        bus.write(0, 0x08);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.af, 0x5678);
        assert_eq!(cpu.af_, 0x1234);
    }

    #[test]
    fn exx_swaps_bc_de_hl_only() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x1111;
        cpu.de = 0x2222;
        cpu.hl = 0x3333;
        cpu.bc_ = 0xAAAA;
        cpu.de_ = 0xBBBB;
        cpu.hl_ = 0xCCCC;
        cpu.af = 0x9999;
        let mut bus = FlatBus::new();
        bus.write(0, 0xD9);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.bc, 0xAAAA);
        assert_eq!(cpu.de, 0xBBBB);
        assert_eq!(cpu.hl, 0xCCCC);
        assert_eq!(cpu.bc_, 0x1111);
        assert_eq!(cpu.de_, 0x2222);
        assert_eq!(cpu.hl_, 0x3333);
        assert_eq!(cpu.af, 0x9999); // AF untouched by EXX
    }

    #[test]
    fn exx_under_dd_prefix_still_swaps_real_hl_not_ix() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x3333;
        cpu.hl_ = 0xCCCC;
        cpu.ix = 0x4444;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0xD9);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 8); // 4 prefix + 4
        assert_eq!(cpu.hl, 0xCCCC);
        assert_eq!(cpu.hl_, 0x3333);
        assert_eq!(cpu.ix, 0x4444); // IX untouched
    }

    #[test]
    fn ex_de_hl_swaps_real_registers_even_under_dd_prefix() {
        let mut cpu = Cpu::new();
        cpu.de = 0x1234;
        cpu.hl = 0x5678;
        cpu.ix = 0x9ABC;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0xEB); // "EX DE,IX" undocumented -- still real DE/HL

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 8);
        assert_eq!(cpu.de, 0x5678);
        assert_eq!(cpu.hl, 0x1234);
        assert_eq!(cpu.ix, 0x9ABC); // IX untouched, exception applies
    }

    #[test]
    fn ex_sp_hl_swaps_with_memory_and_takes_19_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x7012;
        cpu.sp = 0x8000;
        let mut bus = FlatBus::new();
        bus.write(0x8000, 0x34);
        bus.write(0x8001, 0x12);
        bus.write(0, 0xE3);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 19);
        assert_eq!(cpu.hl, 0x1234);
        assert_eq!(bus.read(0x8000), 0x12);
        assert_eq!(bus.read(0x8001), 0x70);
    }

    #[test]
    fn ex_sp_hl_under_fd_prefix_swaps_iy() {
        let mut cpu = Cpu::new();
        cpu.iy = 0x7012;
        cpu.hl = 0x0000;
        cpu.sp = 0x8000;
        let mut bus = FlatBus::new();
        bus.write(0x8000, 0x34);
        bus.write(0x8001, 0x12);
        bus.write(0, 0xFD);
        bus.write(1, 0xE3);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 23); // 4 prefix + 19
        assert_eq!(cpu.iy, 0x1234);
        assert_eq!(cpu.hl, 0x0000); // real HL untouched
    }

    #[test]
    fn push_pop_bc_de_af_round_trip() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x1122;
        cpu.de = 0x3344;
        cpu.af = 0x55C3; // C3 -> arbitrary flags byte
        cpu.sp = 0xFFF0;
        let mut bus = FlatBus::new();
        // PUSH BC; PUSH DE; PUSH AF; POP AF; POP DE; POP BC (LIFO order)
        bus.write(0, 0xC5);
        bus.write(1, 0xD5);
        bus.write(2, 0xF5);
        bus.write(3, 0xF1);
        bus.write(4, 0xD1);
        bus.write(5, 0xC1);

        assert_eq!(cpu.step(&mut bus), 11);
        assert_eq!(cpu.step(&mut bus), 11);
        assert_eq!(cpu.step(&mut bus), 11);
        assert_eq!(cpu.sp, 0xFFF0 - 6);

        cpu.bc = 0;
        cpu.de = 0;
        cpu.af = 0;

        assert_eq!(cpu.step(&mut bus), 10); // pops AF (was pushed last)
        assert_eq!(cpu.af, 0x55C3);
        assert_eq!(cpu.step(&mut bus), 10); // pops DE
        assert_eq!(cpu.de, 0x3344);
        assert_eq!(cpu.step(&mut bus), 10); // pops BC
        assert_eq!(cpu.bc, 0x1122);
        assert_eq!(cpu.sp, 0xFFF0);
    }

    #[test]
    fn push_pop_hl_uses_index_register_under_dd_prefix() {
        let mut cpu = Cpu::new();
        cpu.ix = 0xBEEF;
        cpu.hl = 0x0000;
        cpu.sp = 0xFFF0;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0xE5); // PUSH IX
        bus.write(2, 0xDD);
        bus.write(3, 0xE1); // POP IX

        let push_cycles = cpu.step(&mut bus);
        assert_eq!(push_cycles, 15); // 4 prefix + 11
        assert_eq!(bus.read(cpu.sp), 0xEF);
        assert_eq!(bus.read(cpu.sp.wrapping_add(1)), 0xBE);
        assert_eq!(cpu.hl, 0x0000); // real HL never touched

        cpu.ix = 0;
        let pop_cycles = cpu.step(&mut bus);
        assert_eq!(pop_cycles, 14); // 4 prefix + 10
        assert_eq!(cpu.ix, 0xBEEF);
        assert_eq!(cpu.hl, 0x0000);
    }

    fn rlca_rotates_left_through_bit7_into_carry_and_bit0() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x85); // 1000_0101
        cpu.set_f(FLAG_S | FLAG_Z); // should be preserved
        let mut bus = FlatBus::new();
        bus.write(0, 0x07);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.a(), 0x0B); // 0000_1011
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_H, 0);
        assert_eq!(cpu.f() & FLAG_N, 0);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S); // preserved
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z); // preserved
    }

    fn rrca_rotates_right_through_bit0_into_carry_and_bit7() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x01);
        let mut bus = FlatBus::new();
        bus.write(0, 0x0F);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
    }

    fn rla_rotates_through_carry_flag() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x80); // bit7 set -> goes to carry
        cpu.set_f(FLAG_C); // old carry -> goes to bit0
        let mut bus = FlatBus::new();
        bus.write(0, 0x17);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x01); // old bit7 gone, old carry into bit0
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // new carry = old bit7
    }

    fn rra_rotates_through_carry_flag() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x01); // bit0 set -> goes to carry
        cpu.set_f(FLAG_C); // old carry -> goes to bit7
        let mut bus = FlatBus::new();
        bus.write(0, 0x1F);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
    }
    /// Merged (every check kept): `rlca_rotates_left_through_bit7_into_carry_and_bit0`, `rrca_rotates_right_through_bit0_into_carry_and_bit7`, `rla_rotates_through_carry_flag`, `rra_rotates_through_carry_flag`.
    #[test]
    fn accumulator_rotates() {
        rlca_rotates_left_through_bit7_into_carry_and_bit0();
        rrca_rotates_right_through_bit0_into_carry_and_bit7();
        rla_rotates_through_carry_flag();
        rra_rotates_through_carry_flag();
    }

    #[test]
    fn daa_corrects_bcd_addition_with_half_carry() {
        // 9 + 8 = 17 decimal; ADD A,B produces 0x11 with H set (nibble carry).
        let mut cpu = Cpu::new();
        cpu.set_a(0x11);
        cpu.set_f(FLAG_H); // N=0 (addition), H set, C clear
        let mut bus = FlatBus::new();
        bus.write(0, 0x27);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x17);
        assert_eq!(cpu.f() & FLAG_C, 0);
    }

    #[test]
    fn daa_corrects_bcd_subtraction_with_borrow() {
        // 0x00 - 0x01 via SUB leaves A=0xFF, N=1, H=1, C=1.
        let mut cpu = Cpu::new();
        cpu.set_a(0xFF);
        cpu.set_f(FLAG_N | FLAG_H | FLAG_C);
        let mut bus = FlatBus::new();
        bus.write(0, 0x27);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x99);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N); // N unaffected by DAA
    }

    #[test]
    fn cpl_complements_a_and_sets_h_n() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x00);
        cpu.set_f(FLAG_S | FLAG_Z | FLAG_PV | FLAG_C); // should be preserved
        let mut bus = FlatBus::new();
        bus.write(0, 0x2F);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0xFF);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
        assert_eq!(
            cpu.f() & (FLAG_S | FLAG_Z | FLAG_PV | FLAG_C),
            FLAG_S | FLAG_Z | FLAG_PV | FLAG_C
        );
    }

    #[test]
    fn scf_sets_carry_and_copies_f3_f5_from_a() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x28); // bits 3 and 5 set
        cpu.set_f(FLAG_S | FLAG_H | FLAG_N); // H/N should be cleared, S preserved
        let mut bus = FlatBus::new();
        bus.write(0, 0x37);

        cpu.step(&mut bus);

        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_H, 0);
        assert_eq!(cpu.f() & FLAG_N, 0);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & (FLAG_F3 | FLAG_F5), FLAG_F3 | FLAG_F5);
    }

    #[test]
    fn ccf_toggles_carry_and_moves_old_carry_into_half_carry() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x00);
        cpu.set_f(FLAG_C);
        let mut bus = FlatBus::new();
        bus.write(0, 0x3F);

        cpu.step(&mut bus);

        assert_eq!(cpu.f() & FLAG_C, 0); // toggled off
        assert_eq!(cpu.f() & FLAG_H, FLAG_H); // old C moved into H
        assert_eq!(cpu.f() & FLAG_N, 0);
    }

    fn di_clears_both_iff_flags() {
        let mut cpu = Cpu::new();
        cpu.iff1 = true;
        cpu.iff2 = true;
        let mut bus = FlatBus::new();
        bus.write(0, 0xF3);

        cpu.step(&mut bus);

        assert!(!cpu.iff1);
        assert!(!cpu.iff2);
    }

    fn ei_sets_both_iff_flags() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0xFB);

        cpu.step(&mut bus);

        assert!(cpu.iff1);
        assert!(cpu.iff2);
    }
    /// Merged (every check kept): `di_clears_both_iff_flags`, `ei_sets_both_iff_flags`.
    #[test]
    fn di_and_ei() {
        di_clears_both_iff_flags();
        ei_sets_both_iff_flags();
    }

    #[test]
    fn ld_sp_hl_copies_hl_into_sp() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x4321;
        let mut bus = FlatBus::new();
        bus.write(0, 0xF9);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 6);
        assert_eq!(cpu.sp, 0x4321);
    }

    #[test]
    fn ld_sp_hl_under_fd_prefix_copies_iy() {
        let mut cpu = Cpu::new();
        cpu.iy = 0x8765;
        cpu.hl = 0x0000;
        let mut bus = FlatBus::new();
        bus.write(0, 0xFD);
        bus.write(1, 0xF9);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10); // 4 prefix + 6
        assert_eq!(cpu.sp, 0x8765);
    }

    fn add_a_n_adds_immediate() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x10);
        let mut bus = FlatBus::new();
        bus.write(0, 0xC6);
        bus.write(1, 0x05);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(cpu.a(), 0x15);
        assert_eq!(cpu.pc, 2);
    }

    fn adc_a_n_includes_carry_in() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x10);
        cpu.set_f(FLAG_C);
        let mut bus = FlatBus::new();
        bus.write(0, 0xCE);
        bus.write(1, 0x05);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x16);
    }

    fn sub_n_subtracts_immediate() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x10);
        let mut bus = FlatBus::new();
        bus.write(0, 0xD6);
        bus.write(1, 0x01);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(cpu.a(), 0x0F);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
    }

    fn sbc_a_n_includes_carry_in() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x10);
        cpu.set_f(FLAG_C);
        let mut bus = FlatBus::new();
        bus.write(0, 0xDE);
        bus.write(1, 0x01);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x0E);
    }

    fn and_n_masks_immediate() {
        let mut cpu = Cpu::new();
        cpu.set_a(0xFF);
        let mut bus = FlatBus::new();
        bus.write(0, 0xE6);
        bus.write(1, 0x0F);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x0F);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
    }

    fn xor_n_xors_immediate() {
        let mut cpu = Cpu::new();
        cpu.set_a(0xFF);
        let mut bus = FlatBus::new();
        bus.write(0, 0xEE);
        bus.write(1, 0xFF);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
    }

    fn or_n_ors_immediate() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x0F);
        let mut bus = FlatBus::new();
        bus.write(0, 0xF6);
        bus.write(1, 0xF0);

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0xFF);
    }

    fn cp_n_compares_without_storing_result() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x10);
        let mut bus = FlatBus::new();
        bus.write(0, 0xFE);
        bus.write(1, 0x10);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(cpu.a(), 0x10); // unchanged
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
    }
    /// Merged (every check kept): `add_a_n_adds_immediate`, `adc_a_n_includes_carry_in`, `sub_n_subtracts_immediate`, `sbc_a_n_includes_carry_in`, `and_n_masks_immediate`, `xor_n_xors_immediate`, `or_n_ors_immediate`, `cp_n_compares_without_storing_result`.
    #[test]
    fn alu_immediate_operations() {
        add_a_n_adds_immediate();
        adc_a_n_includes_carry_in();
        sub_n_subtracts_immediate();
        sbc_a_n_includes_carry_in();
        and_n_masks_immediate();
        xor_n_xors_immediate();
        or_n_ors_immediate();
        cp_n_compares_without_storing_result();
    }

    #[test]
    fn out_n_a_writes_port_addressed_with_a_in_upper_byte() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x42);
        let mut bus = IoBus::new();
        bus.write(0, 0xD3);
        bus.write(1, 0x10);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 11);
        assert_eq!(bus.last_out_port, Some(0x4210));
        assert_eq!(bus.last_out_val, Some(0x42));
    }

    #[test]
    fn in_a_n_reads_port_into_a() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x42);
        let mut bus = IoBus::new();
        bus.io[0x4210] = 0x99;
        bus.write(0, 0xDB);
        bus.write(1, 0x10);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 11);
        assert_eq!(cpu.a(), 0x99);
    }

    #[test]
    fn active_index_reset_does_not_leak_between_instructions() {
        // Sanity check that a DD-prefixed PUSH HL followed by a plain PUSH HL
        // in the next step() only substitutes IX on the first instruction.
        let mut cpu = Cpu::new();
        cpu.ix = 0x1111;
        cpu.hl = 0x2222;
        cpu.sp = 0xFFF0;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0xE5); // PUSH IX
        bus.write(2, 0xE5); // PUSH HL

        cpu.step(&mut bus);
        let sp_after_first = cpu.sp;
        assert_eq!(bus.read(sp_after_first), 0x11);
        assert_eq!(bus.read(sp_after_first.wrapping_add(1)), 0x11);

        let cycles = cpu.step(&mut bus);
        assert_eq!(cycles, 11);
        assert_eq!(bus.read(cpu.sp), 0x22);
        assert_eq!(bus.read(cpu.sp.wrapping_add(1)), 0x22);
    }
}
