use super::Handler;
use crate::Bus;
use crate::cpu::{Cpu, FLAG_C};

/// The 16-bit register pair selected by the `pp` field (bits 5-4 of the ED
/// sub-opcode: 00=BC, 01=DE, 10=HL, 11=SP). ED-prefixed instructions are
/// never affected by a preceding/following DD/FD prefix, so this always
/// reads/writes the *real* register pairs directly -- never `hl_or_index()`.
fn get_pp(cpu: &Cpu, pp: u8) -> u16 {
    match pp {
        0 => cpu.bc,
        1 => cpu.de,
        2 => cpu.hl,
        3 => cpu.sp,
        _ => unreachable!("pp field out of range: {pp}"),
    }
}

fn set_pp(cpu: &mut Cpu, pp: u8, val: u16) {
    match pp {
        0 => cpu.bc = val,
        1 => cpu.de = val,
        2 => cpu.hl = val,
        3 => cpu.sp = val,
        _ => unreachable!("pp field out of range: {pp}"),
    }
}

fn pop16<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u16 {
    let lo = cpu.mem_read(bus, cpu.sp);
    cpu.sp = cpu.sp.wrapping_add(1);
    let hi = cpu.mem_read(bus, cpu.sp);
    cpu.sp = cpu.sp.wrapping_add(1);
    u16::from_le_bytes([lo, hi])
}

fn adc16(cpu: &mut Cpu, operand: u16) {
    let hl = cpu.hl;
    let carry_in = u16::from(cpu.f() & FLAG_C != 0);
    let sum = hl as u32 + operand as u32 + carry_in as u32;
    let result = sum as u16;
    let carry = sum > 0xFFFF;
    let half = (hl & 0x0FFF) as u32 + (operand & 0x0FFF) as u32 + carry_in as u32 > 0x0FFF;
    let overflow = (hl ^ operand) & 0x8000 == 0 && (hl ^ result) & 0x8000 != 0;
    let flags = super::flags_byte(
        result & 0x8000 != 0,
        result == 0,
        half,
        overflow,
        false,
        carry,
    );
    super::apply_alu_flags(cpu, flags, (result >> 8) as u8);
    cpu.hl = result;
}

fn sbc16(cpu: &mut Cpu, operand: u16) {
    let hl = cpu.hl;
    let carry_in = i32::from(cpu.f() & FLAG_C != 0);
    let diff = hl as i32 - operand as i32 - carry_in;
    let result = diff as u16;
    let carry = diff < 0;
    let half = (hl & 0x0FFF) as i32 - (operand & 0x0FFF) as i32 - carry_in < 0;
    let overflow = (hl ^ operand) & 0x8000 != 0 && (hl ^ result) & 0x8000 != 0;
    let flags = super::flags_byte(
        result & 0x8000 != 0,
        result == 0,
        half,
        overflow,
        true,
        carry,
    );
    super::apply_alu_flags(cpu, flags, (result >> 8) as u8);
    cpu.hl = result;
}

fn adc_hl_ss<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let pp = (opcode >> 4) & 0x03;
    let operand = get_pp(cpu, pp);
    adc16(cpu, operand);
    15
}

fn sbc_hl_ss<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let pp = (opcode >> 4) & 0x03;
    let operand = get_pp(cpu, pp);
    sbc16(cpu, operand);
    15
}

fn ld_nn_ss<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let pp = (opcode >> 4) & 0x03;
    let lo = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    let hi = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    let addr = u16::from_le_bytes([lo, hi]);

    let val = get_pp(cpu, pp);
    cpu.mem_write(bus, addr, val as u8);
    cpu.mem_write(bus, addr.wrapping_add(1), (val >> 8) as u8);
    20
}

fn ld_ss_nn<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let pp = (opcode >> 4) & 0x03;
    let lo = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    let hi = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    let addr = u16::from_le_bytes([lo, hi]);

    let val_lo = cpu.mem_read(bus, addr);
    let val_hi = cpu.mem_read(bus, addr.wrapping_add(1));
    set_pp(cpu, pp, u16::from_le_bytes([val_lo, val_hi]));
    20
}

fn neg<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let a = cpu.a();
    let result = 0u8.wrapping_sub(a);
    let s = result & 0x80 != 0;
    let z = result == 0;
    let half = (a & 0x0F) != 0; // borrow into bit 4 whenever the low nibble was non-zero
    let overflow = a == 0x80; // the only value whose negation overflows an i8
    let carry = a != 0; // NEG clears carry only when A was already 0
    let flags = super::flags_byte(s, z, half, overflow, true, carry);
    super::apply_alu_flags(cpu, flags, result);
    cpu.set_a(result);
    8
}

/// Every `ED xx` the CPU doesn't define executes as a two-byte NOP (8
/// T-states: the `ED` prefix fetch plus the opcode fetch) -- real silicon
/// never traps on them, and software (copy protections, junk after a crash)
/// can run them. Only the entries `install` overrides below do anything.
pub(crate) fn undefined_ed<B: Bus>(_cpu: &mut Cpu, _bus: &mut B) -> u8 {
    8
}

fn retn<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.pc = pop16(cpu, bus);
    cpu.iff1 = cpu.iff2;
    14
}

fn reti<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.pc = pop16(cpu, bus);
    14
}

fn im0<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.im = 0;
    8
}

fn im1<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.im = 1;
    8
}

fn im2<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.im = 2;
    8
}

fn ld_i_a<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.i = cpu.a();
    9
}

fn ld_r_a<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    cpu.r = cpu.a();
    9
}

fn ld_a_i<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let i = cpu.i;
    let carry = cpu.f() & FLAG_C != 0;
    cpu.set_a(i);
    let flags = super::flags_byte(i & 0x80 != 0, i == 0, false, cpu.iff2, false, carry);
    super::apply_alu_flags(cpu, flags, i);
    9
}

fn ld_a_r<B: Bus>(cpu: &mut Cpu, _bus: &mut B) -> u8 {
    let r = cpu.r;
    let carry = cpu.f() & FLAG_C != 0;
    cpu.set_a(r);
    let flags = super::flags_byte(r & 0x80 != 0, r == 0, false, cpu.iff2, false, carry);
    super::apply_alu_flags(cpu, flags, r);
    9
}

fn rrd<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let addr = cpu.hl;
    let mem = cpu.mem_read(bus, addr);
    let a = cpu.a();

    // new mem-high = old A-low; new mem-low = old mem-high; new A-low = old mem-low
    let new_mem = ((a & 0x0F) << 4) | (mem >> 4);
    let new_a = (a & 0xF0) | (mem & 0x0F);

    cpu.mem_write(bus, addr, new_mem);
    let carry = cpu.f() & FLAG_C != 0;
    cpu.set_a(new_a);
    let flags = super::flags_byte(
        new_a & 0x80 != 0,
        new_a == 0,
        false,
        super::parity(new_a),
        false,
        carry,
    );
    super::apply_alu_flags(cpu, flags, new_a);
    18
}

fn rld<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let addr = cpu.hl;
    let mem = cpu.mem_read(bus, addr);
    let a = cpu.a();

    // new mem-high = old mem-low; new mem-low = old A-low; new A-low = old mem-high
    let new_mem = ((mem & 0x0F) << 4) | (a & 0x0F);
    let new_a = (a & 0xF0) | (mem >> 4);

    cpu.mem_write(bus, addr, new_mem);
    let carry = cpu.f() & FLAG_C != 0;
    cpu.set_a(new_a);
    let flags = super::flags_byte(
        new_a & 0x80 != 0,
        new_a == 0,
        false,
        super::parity(new_a),
        false,
        carry,
    );
    super::apply_alu_flags(cpu, flags, new_a);
    18
}

/// 0x70: the undocumented "IN (C)" / "IN F,(C)" -- reads the port and sets
/// flags exactly like IN r,(C), but discards the value instead of storing it
/// to any register.
fn in_c<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let val = cpu.io_read(bus, cpu.bc);
    let carry = cpu.f() & FLAG_C != 0;
    let flags = super::flags_byte(
        val & 0x80 != 0,
        val == 0,
        false,
        super::parity(val),
        false,
        carry,
    );
    super::apply_alu_flags(cpu, flags, val);
    12
}

/// 0x71: the undocumented "OUT (C),0" -- writes a constant 0 to the port
/// (the NMOS Z80 behavior; some CMOS variants write 0xFF instead). No flags
/// affected.
fn out_c_0<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.io_write(bus, cpu.bc, 0);
    12
}

/// IN r,(C): reads port `cpu.bc` into the register selected by the
/// opcode's `r` field. Always uses the real B/C/D/E/H/L/A registers directly
/// (never `reg_get`/`reg_set`'s DD/FD-aware H/L substitution) since ED
/// instructions must not be affected by a leaked `active_index` from an
/// (invalid but not unrepresentable in this dispatcher) preceding DD/FD.
fn in_r_c<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let r = (opcode >> 3) & 0x07;
    let val = cpu.io_read(bus, cpu.bc);
    match r {
        0 => cpu.set_b(val),
        1 => cpu.set_c(val),
        2 => cpu.set_d(val),
        3 => cpu.set_e(val),
        4 => cpu.set_h(val),
        5 => cpu.set_l(val),
        7 => cpu.set_a(val),
        _ => unreachable!("IN r,(C) with unsupported r field: {r}"),
    }
    let carry = cpu.f() & FLAG_C != 0;
    let flags = super::flags_byte(
        val & 0x80 != 0,
        val == 0,
        false,
        super::parity(val),
        false,
        carry,
    );
    super::apply_alu_flags(cpu, flags, val);
    12
}

/// OUT (C),r: writes the register selected by the opcode's `r` field to
/// port `cpu.bc`. See `in_r_c` for why this bypasses `reg_get`/`reg_set`.
fn out_c_r<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let r = (opcode >> 3) & 0x07;
    let val = match r {
        0 => cpu.b(),
        1 => cpu.c(),
        2 => cpu.d(),
        3 => cpu.e(),
        4 => cpu.h(),
        5 => cpu.l(),
        7 => cpu.a(),
        _ => unreachable!("OUT (C),r with unsupported r field: {r}"),
    };
    cpu.io_write(bus, cpu.bc, val);
    12
}

pub(crate) const fn install<B: Bus>(table: [Handler<B>; 256]) -> [Handler<B>; 256] {
    let mut table = table;

    table[0x4A] = adc_hl_ss;
    table[0x5A] = adc_hl_ss;
    table[0x6A] = adc_hl_ss;
    table[0x7A] = adc_hl_ss;

    table[0x42] = sbc_hl_ss;
    table[0x52] = sbc_hl_ss;
    table[0x62] = sbc_hl_ss;
    table[0x72] = sbc_hl_ss;

    table[0x43] = ld_nn_ss;
    table[0x53] = ld_nn_ss;
    table[0x63] = ld_nn_ss;
    table[0x73] = ld_nn_ss;

    table[0x4B] = ld_ss_nn;
    table[0x5B] = ld_ss_nn;
    table[0x6B] = ld_ss_nn;
    table[0x7B] = ld_ss_nn;

    table[0x44] = neg;
    // Undocumented mirrors of NEG, RETN and the IM instructions (Sean Young,
    // "The Undocumented Z80 Documented", section 4.1): the decoder only looks
    // at some of the opcode's bits, so these behave exactly like their
    // documented twins. `ED 77`/`ED 7F` and every other undefined `ED xx` are
    // NOPs (`undefined_ed`).
    table[0x4C] = neg;
    table[0x54] = neg;
    table[0x5C] = neg;
    table[0x64] = neg;
    table[0x6C] = neg;
    table[0x74] = neg;
    table[0x7C] = neg;

    table[0x45] = retn;
    table[0x55] = retn;
    table[0x5D] = retn;
    table[0x65] = retn;
    table[0x6D] = retn;
    table[0x75] = retn;
    table[0x7D] = retn;
    table[0x4D] = reti;

    table[0x46] = im0;
    table[0x4E] = im0;
    table[0x66] = im0;
    table[0x6E] = im0;
    table[0x56] = im1;
    table[0x76] = im1;
    table[0x5E] = im2;
    table[0x7E] = im2;

    table[0x47] = ld_i_a;
    table[0x4F] = ld_r_a;
    table[0x57] = ld_a_i;
    table[0x5F] = ld_a_r;

    table[0x67] = rrd;
    table[0x6F] = rld;

    table[0x40] = in_r_c;
    table[0x48] = in_r_c;
    table[0x50] = in_r_c;
    table[0x58] = in_r_c;
    table[0x60] = in_r_c;
    table[0x68] = in_r_c;
    table[0x70] = in_c;
    table[0x78] = in_r_c;

    table[0x41] = out_c_r;
    table[0x49] = out_c_r;
    table[0x51] = out_c_r;
    table[0x59] = out_c_r;
    table[0x61] = out_c_r;
    table[0x69] = out_c_r;
    table[0x71] = out_c_0;
    table[0x79] = out_c_r;

    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{FLAG_H, FLAG_N, FLAG_PV, FLAG_S, FLAG_Z};
    use crate::test_support::FlatBus;

    /// A FlatBus-alike that additionally records IO reads/writes, since
    /// `test_support::FlatBus`'s `read_io`/`write_io` are no-op stubs and
    /// this file can't edit that shared file.
    struct IoBus {
        mem: [u8; 0x10000],
        io: [u8; 0x10000],
        last_out: Option<(u16, u8)>,
    }

    impl IoBus {
        fn new() -> Self {
            Self {
                mem: [0; 0x10000],
                io: [0; 0x10000],
                last_out: None,
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
            self.last_out = Some((port, val));
        }
    }

    fn ed_prog(bus: &mut FlatBus, sub_opcode: u8) {
        bus.write(0, 0xED);
        bus.write(1, sub_opcode);
    }

    #[test]
    fn adc_hl_bc_signed_overflow_sets_pv_h_and_clears_c() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x7FFF;
        cpu.bc = 0x0001;
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x4A); // ADC HL,BC

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 15);
        assert_eq!(cpu.hl, 0x8000);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV);
        assert_eq!(cpu.f() & FLAG_N, 0);
        assert_eq!(cpu.f() & FLAG_C, 0);
    }

    #[test]
    fn adc_hl_de_consumes_carry_in_and_wraps_to_zero() {
        let mut cpu = Cpu::new();
        cpu.hl = 0xFFFF;
        cpu.de = 0x0000;
        cpu.set_f(FLAG_C);
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x5A); // ADC HL,DE

        cpu.step(&mut bus);

        assert_eq!(cpu.hl, 0x0000);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_PV, 0);
    }

    #[test]
    fn adc_hl_hl_doubles_real_hl_even_with_dd_prefix_active() {
        // DD ED 6A -- ADC HL,HL. The DD prefix must not leak into the ED
        // instruction: this must operate on real HL, not IX.
        let mut cpu = Cpu::new();
        cpu.hl = 0x1234;
        cpu.ix = 0x9999;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0xED);
        bus.write(2, 0x6A);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4 + 15);
        assert_eq!(cpu.hl, 0x2468);
        assert_eq!(cpu.ix, 0x9999);
    }

    #[test]
    fn sbc_hl_de_borrow_sets_carry_and_half_borrow_without_overflow() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x0000;
        cpu.de = 0x0001;
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x52); // SBC HL,DE

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 15);
        assert_eq!(cpu.hl, 0xFFFF);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_PV, 0);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
    }

    #[test]
    fn sbc_hl_sp_signed_overflow() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x8000;
        cpu.sp = 0x0001;
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x72); // SBC HL,SP

        cpu.step(&mut bus);

        assert_eq!(cpu.hl, 0x7FFF);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV);
        assert_eq!(cpu.f() & FLAG_C, 0);
    }

    #[test]
    fn ld_nn_hl_and_ld_hl_nn_round_trip() {
        let mut cpu = Cpu::new();
        cpu.hl = 0xBEEF;
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x63); // LD (nn),HL
        bus.write(2, 0x00);
        bus.write(3, 0x40); // nn = 0x4000

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 20);
        assert_eq!(bus.read(0x4000), 0xEF);
        assert_eq!(bus.read(0x4001), 0xBE);

        let mut cpu2 = Cpu::new();
        ed_prog(&mut bus, 0x6B); // LD HL,(nn)
        bus.write(2, 0x00);
        bus.write(3, 0x40);

        let cycles2 = cpu2.step(&mut bus);

        assert_eq!(cycles2, 20);
        assert_eq!(cpu2.hl, 0xBEEF);
    }

    #[test]
    fn ld_nn_sp_stores_real_sp() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x1122;
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x73); // LD (nn),SP
        bus.write(2, 0x10);
        bus.write(3, 0x00); // nn = 0x0010

        cpu.step(&mut bus);

        assert_eq!(bus.read(0x0010), 0x22);
        assert_eq!(bus.read(0x0011), 0x11);
    }

    #[test]
    fn undefined_ed_opcodes_are_eight_t_state_nops_that_never_panic() {
        // Everything below is undefined on a real Z80 (a gap in the ED map).
        let mut undefined: Vec<u8> = (0x00..=0x3F).collect();
        undefined.extend([0x77, 0x7F]);
        undefined.extend(0x80..=0x9F);
        undefined.extend([0xA4, 0xA5, 0xA6, 0xA7, 0xAC, 0xAD, 0xAE, 0xAF]);
        undefined.extend([0xB4, 0xB5, 0xB6, 0xB7, 0xBC, 0xBD, 0xBE, 0xBF]);
        undefined.extend(0xC0..=0xFF);
        for sub in undefined {
            let mut cpu = Cpu::new();
            cpu.set_a(0x5A);
            cpu.set_f(0xA5);
            let mut bus = FlatBus::new();
            ed_prog(&mut bus, sub);

            let cycles = cpu.step(&mut bus);

            assert_eq!(cycles, 8, "ED {sub:02X} is an 8-T-state NOP");
            assert_eq!(cpu.pc, 2, "ED {sub:02X} is two bytes long");
            assert_eq!(cpu.a(), 0x5A, "ED {sub:02X} must not touch A");
            assert_eq!(cpu.f(), 0xA5, "ED {sub:02X} must not touch the flags");
        }
    }

    #[test]
    fn undocumented_neg_retn_and_im_mirrors_match_their_documented_twins() {
        // NEG mirrors: identical result, flags and timing to ED 44.
        for mirror in [0x4C, 0x54, 0x5C, 0x64, 0x6C, 0x74, 0x7C] {
            for a in [0x00u8, 0x01, 0x80, 0xFF] {
                let run = |sub: u8| {
                    let mut cpu = Cpu::new();
                    cpu.set_a(a);
                    let mut bus = FlatBus::new();
                    ed_prog(&mut bus, sub);
                    let cycles = cpu.step(&mut bus);
                    (cpu.a(), cpu.f(), cpu.pc, cycles)
                };
                assert_eq!(run(mirror), run(0x44), "ED {mirror:02X} (A={a:02X}) vs NEG");
            }
        }

        // RETN mirrors: pop PC and copy IFF2 into IFF1, like ED 45.
        for mirror in [0x55, 0x5D, 0x65, 0x6D, 0x75, 0x7D] {
            let mut cpu = Cpu::new();
            cpu.iff1 = false;
            cpu.iff2 = true;
            cpu.sp = 0x8000;
            let mut bus = FlatBus::new();
            bus.write(0x8000, 0x34);
            bus.write(0x8001, 0x12);
            ed_prog(&mut bus, mirror);

            let cycles = cpu.step(&mut bus);

            assert_eq!(cycles, 14, "ED {mirror:02X}");
            assert_eq!(cpu.pc, 0x1234, "ED {mirror:02X} pops the return address");
            assert!(cpu.iff1, "ED {mirror:02X} restores IFF1 from IFF2");
        }

        // IM mirrors.
        for (sub, mode) in [
            (0x46, 0),
            (0x4E, 0),
            (0x66, 0),
            (0x6E, 0),
            (0x56, 1),
            (0x76, 1),
            (0x5E, 2),
            (0x7E, 2),
        ] {
            let mut cpu = Cpu::new();
            cpu.im = 3 - mode; // anything different from the target
            let mut bus = FlatBus::new();
            ed_prog(&mut bus, sub);

            let cycles = cpu.step(&mut bus);

            assert_eq!(cycles, 8, "ED {sub:02X}");
            assert_eq!(cpu.im, mode, "ED {sub:02X} selects IM {mode}");
        }
    }

    #[test]
    fn neg_of_0x80_overflows_and_keeps_carry_set() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x80);
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x44); // NEG

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 8);
        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
        assert_eq!(cpu.f() & FLAG_H, 0);
    }

    #[test]
    fn neg_of_zero_clears_carry_and_sets_zero() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x00);
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x44); // NEG

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_C, 0);
        assert_eq!(cpu.f() & FLAG_PV, 0);
    }

    #[test]
    fn neg_of_one_sets_half_borrow_and_carry() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x01);
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x44); // NEG

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0xFF);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
    }

    #[test]
    fn retn_pops_pc_and_restores_iff1_from_iff2() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x2000;
        cpu.iff1 = false;
        cpu.iff2 = true;
        let mut bus = FlatBus::new();
        bus.write(0x2000, 0x34);
        bus.write(0x2001, 0x12);
        ed_prog(&mut bus, 0x45); // RETN

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 14);
        assert_eq!(cpu.pc, 0x1234);
        assert_eq!(cpu.sp, 0x2002);
        assert!(cpu.iff1);
    }

    #[test]
    fn reti_pops_pc_without_touching_iff() {
        let mut cpu = Cpu::new();
        cpu.sp = 0x2000;
        cpu.iff1 = false;
        cpu.iff2 = true;
        let mut bus = FlatBus::new();
        bus.write(0x2000, 0x78);
        bus.write(0x2001, 0x56);
        ed_prog(&mut bus, 0x4D); // RETI

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 14);
        assert_eq!(cpu.pc, 0x5678);
        assert!(!cpu.iff1); // RETI does not restore iff1
    }

    #[test]
    fn im_instructions_set_interrupt_mode() {
        let mut bus = FlatBus::new();

        let mut cpu = Cpu::new();
        ed_prog(&mut bus, 0x46);
        let c = cpu.step(&mut bus);
        assert_eq!(c, 8);
        assert_eq!(cpu.im, 0);

        let mut cpu = Cpu::new();
        ed_prog(&mut bus, 0x56);
        cpu.step(&mut bus);
        assert_eq!(cpu.im, 1);

        let mut cpu = Cpu::new();
        ed_prog(&mut bus, 0x5E);
        cpu.step(&mut bus);
        assert_eq!(cpu.im, 2);
    }

    #[test]
    fn ld_i_a_and_ld_r_a_copy_without_touching_flags() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x42);
        cpu.set_f(0xFF);
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x47); // LD I,A

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 9);
        assert_eq!(cpu.i, 0x42);
        assert_eq!(cpu.f(), 0xFF);

        let mut cpu2 = Cpu::new();
        cpu2.set_a(0x77);
        ed_prog(&mut bus, 0x4F); // LD R,A
        cpu2.step(&mut bus);
        assert_eq!(cpu2.r, 0x77);
    }

    #[test]
    fn ld_a_i_reflects_iff2_in_pv_and_preserves_carry() {
        let mut cpu = Cpu::new();
        cpu.i = 0x80;
        cpu.iff2 = true;
        cpu.set_f(FLAG_C);
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x57); // LD A,I

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 9);
        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV);
        assert_eq!(cpu.f() & FLAG_H, 0);
        assert_eq!(cpu.f() & FLAG_N, 0);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // unaffected, still set
    }

    #[test]
    fn ld_a_r_reflects_zero_and_clears_pv_when_iff2_false() {
        let mut cpu = Cpu::new();
        // `ED 5F` is two M1 fetches, so R reads back 2 more than it was: start
        // 2 below the 7-bit wrap to read exactly zero.
        cpu.r = 0x7E;
        cpu.iff2 = false;
        let mut bus = FlatBus::new();
        ed_prog(&mut bus, 0x5F); // LD A,R

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_PV, 0);
    }

    #[test]
    fn rrd_rotates_nibbles_between_a_and_memory() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x3000;
        cpu.set_a(0x84);
        let mut bus = FlatBus::new();
        bus.write(0x3000, 0x20);
        ed_prog(&mut bus, 0x67); // RRD

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 18);
        // A high nibble unchanged (8); A low nibble <- old (HL) low nibble (0)
        assert_eq!(cpu.a(), 0x80);
        // (HL) high nibble <- old A low nibble (4); (HL) low nibble <- old (HL) high nibble (2)
        assert_eq!(bus.read(0x3000), 0x42);
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
    }

    #[test]
    fn rld_rotates_nibbles_between_a_and_memory() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x3000;
        cpu.set_a(0x84);
        let mut bus = FlatBus::new();
        bus.write(0x3000, 0x20);
        ed_prog(&mut bus, 0x6F); // RLD

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 18);
        // A high nibble unchanged (8); A low nibble <- old (HL) high nibble (2)
        assert_eq!(cpu.a(), 0x82);
        // (HL) high nibble <- old (HL) low nibble (0); (HL) low nibble <- old A low nibble (4)
        assert_eq!(bus.read(0x3000), 0x04);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // 0x82 has even parity
    }

    #[test]
    fn in_r_c_reads_port_bc_into_register_and_sets_flags() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x1234;
        cpu.set_f(FLAG_C); // must be left unaffected
        let mut bus = IoBus::new();
        bus.io[0x1234] = 0x81; // even parity (two set bits), sign set
        bus.mem[0] = 0xED;
        bus.mem[1] = 0x40; // IN B,(C)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 12);
        assert_eq!(cpu.b(), 0x81);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // 0x81 has even parity
        assert_eq!(cpu.f() & FLAG_C, FLAG_C); // unaffected
    }

    #[test]
    fn in_a_c_targets_a_register() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x0055;
        let mut bus = IoBus::new();
        bus.io[0x0055] = 0x00;
        bus.mem[0] = 0xED;
        bus.mem[1] = 0x78; // IN A,(C)

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // 0 has even parity
    }

    #[test]
    fn out_c_r_writes_register_to_port_bc() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x4321;
        cpu.set_e(0x99);
        let mut bus = IoBus::new();
        bus.mem[0] = 0xED;
        bus.mem[1] = 0x59; // OUT (C),E

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 12);
        assert_eq!(bus.last_out, Some((0x4321, 0x99)));
    }

    #[test]
    fn in_c_reads_port_and_sets_flags_without_storing_to_any_register() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x0012;
        cpu.set_a(0x55);
        let mut bus = IoBus::new();
        bus.io[0x0012] = 0x81; // bit 7 set -> S; two bits set -> even parity -> PV
        bus.mem[0] = 0xED;
        bus.mem[1] = 0x70; // IN (C)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 12);
        assert_eq!(cpu.a(), 0x55); // no register written
        assert_eq!(cpu.bc, 0x0012); // BC untouched
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // 0x81 has even parity
    }

    #[test]
    fn out_c_0_writes_zero_to_port_bc() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x8001;
        let mut bus = IoBus::new();
        bus.mem[0] = 0xED;
        bus.mem[1] = 0x71; // OUT (C),0

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 12);
        assert_eq!(bus.last_out, Some((0x8001, 0)));
    }

    #[test]
    fn in_r_c_ignores_leaked_dd_prefix_and_targets_real_h() {
        // DD ED 60 -- IN H,(C). The DD prefix's active_index must not leak
        // into the ED instruction: this must set real H, not IXH.
        let mut cpu = Cpu::new();
        cpu.bc = 0x9000;
        cpu.ix = 0xAABB;
        let mut bus = IoBus::new();
        bus.io[0x9000] = 0x77;
        bus.mem[0] = 0xDD;
        bus.mem[1] = 0xED;
        bus.mem[2] = 0x60; // IN H,(C)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4 + 12);
        assert_eq!(cpu.h(), 0x77);
        assert_eq!(cpu.ix, 0xAABB); // untouched
    }
}
