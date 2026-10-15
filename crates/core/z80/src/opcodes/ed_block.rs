use super::{Handler, apply_alu_flags, flags_byte, parity, sub_core};
use crate::Bus;
use crate::cpu::{Cpu, FLAG_C, FLAG_PV, FLAG_S, FLAG_Z};

// ---------------------------------------------------------------------
// LDI/LDD/LDIR/LDDR -- 0xA0/0xA8/0xB0/0xB8
// ---------------------------------------------------------------------

/// Shared core for LDI/LDD: copies (HL) -> (DE), advances HL/DE by `delta`,
/// decrements BC. Flags: H=0, N=0, PV=(BC!=0 after decrement), S/Z/C
/// unaffected. F3/F5 come from bits 3/1 of `transferred_byte + A` (the
/// well-known LDI/LDD "bit 1 -> F5" quirk), not from the usual result bits.
/// Returns the new BC != 0 (the repeat condition for LDIR/LDDR).
fn ld_block<B: Bus>(cpu: &mut Cpu, bus: &mut B, delta: i16) -> bool {
    let value = cpu.mem_read(bus, cpu.hl);
    cpu.mem_write(bus, cpu.de, value);

    cpu.hl = cpu.hl.wrapping_add(delta as u16);
    cpu.de = cpu.de.wrapping_add(delta as u16);
    cpu.bc = cpu.bc.wrapping_sub(1);
    let bc_nonzero = cpu.bc != 0;

    // bit3 of n -> F3, bit1 of n -> F5 (0x20); apply_alu_flags only reads
    // the FLAG_F3|FLAG_F5 bits of this source byte, so the rest don't matter.
    let n = value.wrapping_add(cpu.a());
    let f3f5_source = (n & 0x08) | ((n & 0x02) << 4);
    let preserved = cpu.f() & (FLAG_S | FLAG_Z | FLAG_C);
    let flags = preserved | if bc_nonzero { FLAG_PV } else { 0 }; // H=0, N=0 implicitly
    apply_alu_flags(cpu, flags, f3f5_source);

    bc_nonzero
}

fn ldi<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    ld_block(cpu, bus, 1);
    16
}

fn ldd<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    ld_block(cpu, bus, -1);
    16
}

fn ldir<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    if ld_block(cpu, bus, 1) {
        cpu.pc = cpu.pc.wrapping_sub(2);
        21
    } else {
        16
    }
}

fn lddr<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    if ld_block(cpu, bus, -1) {
        cpu.pc = cpu.pc.wrapping_sub(2);
        21
    } else {
        16
    }
}

// ---------------------------------------------------------------------
// CPI/CPD/CPIR/CPDR -- 0xA1/0xA9/0xB1/0xB9
// ---------------------------------------------------------------------

/// Shared core for CPI/CPD: compares A against (HL) (like CP, result
/// discarded), advances HL by `delta`, decrements BC. Flags: S/Z/H from the
/// comparison, PV=(BC!=0 after decrement), N=1, C unaffected. F3/F5 come
/// from bits 3/1 of `(A - value - H)` (same bit-1-\>F5 quirk as LDI/LDD).
/// Returns the repeat condition for CPIR/CPDR: BC!=0 && not equal.
fn cp_block<B: Bus>(cpu: &mut Cpu, bus: &mut B, delta: i16) -> bool {
    let value = cpu.mem_read(bus, cpu.hl);
    let (result, s, z, h, _pv, _c) = sub_core(cpu.a(), value, 0);

    cpu.hl = cpu.hl.wrapping_add(delta as u16);
    cpu.bc = cpu.bc.wrapping_sub(1);
    let bc_nonzero = cpu.bc != 0;

    let n = if h { result.wrapping_sub(1) } else { result };
    let f3f5_source = (n & 0x08) | ((n & 0x02) << 4);

    let old_c = cpu.f() & FLAG_C;
    let flags = flags_byte(s, z, h, bc_nonzero, true, false) | old_c;
    apply_alu_flags(cpu, flags, f3f5_source);

    bc_nonzero && !z
}

fn cpi<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cp_block(cpu, bus, 1);
    16
}

fn cpd<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cp_block(cpu, bus, -1);
    16
}

fn cpir<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    if cp_block(cpu, bus, 1) {
        cpu.pc = cpu.pc.wrapping_sub(2);
        21
    } else {
        16
    }
}

fn cpdr<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    if cp_block(cpu, bus, -1) {
        cpu.pc = cpu.pc.wrapping_sub(2);
        21
    } else {
        16
    }
}

// ---------------------------------------------------------------------
// INI/IND/INIR/INDR -- 0xA2/0xAA/0xB2/0xBA
// ---------------------------------------------------------------------

/// Shared core for INI/IND: reads a byte from port BC into (HL), advances
/// HL by `delta`, decrements B. Z and the repeat condition (B!=0) are exact
/// per the Z80 spec. S/H/PV/N/C/F3/F5 follow the documented "undocumented
/// flags" formula (Sean Young's "The Undocumented Z80 Documented"): letting
/// `k = value + ((C +/- 1) & 0xFF)` (the same direction as `delta`),
/// H=C=(k>255), PV=parity((k&7)^B'), N=bit7 of value, S/Z/F3/F5 from the new
/// B. This core doesn't independently verify that formula against real
/// hardware/ZEXALL; flag it for extra scrutiny if exact undocumented-flag
/// accuracy on I/O block ops turns out to matter.
fn in_block<B: Bus>(cpu: &mut Cpu, bus: &mut B, delta: i16) -> bool {
    // INI/IND's second M1 is five T-states long; the extra one precedes the
    // port read.
    cpu.internal_cycles(bus, 1);
    let value = cpu.io_read(bus, cpu.bc);
    cpu.mem_write(bus, cpu.hl, value);
    cpu.hl = cpu.hl.wrapping_add(delta as u16);

    let new_b = cpu.b().wrapping_sub(1);
    cpu.set_b(new_b);

    let c_adj = (cpu.c() as i16 + delta) as u8;
    let k = value as u16 + c_adj as u16;
    let hc = k > 0xFF;
    let pv = parity(((k as u8) & 0x07) ^ new_b);
    let s = new_b & 0x80 != 0;
    let z = new_b == 0;

    // N is documented (officially, not just as an undocumented refinement)
    // to always be set for INI/IND/OUTI/OUTD.
    let flags = flags_byte(s, z, hc, pv, true, hc);
    apply_alu_flags(cpu, flags, new_b);

    new_b != 0
}

fn ini<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    in_block(cpu, bus, 1);
    16
}

fn ind<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    in_block(cpu, bus, -1);
    16
}

fn inir<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    if in_block(cpu, bus, 1) {
        cpu.pc = cpu.pc.wrapping_sub(2);
        21
    } else {
        16
    }
}

fn indr<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    if in_block(cpu, bus, -1) {
        cpu.pc = cpu.pc.wrapping_sub(2);
        21
    } else {
        16
    }
}

// ---------------------------------------------------------------------
// OUTI/OUTD/OTIR/OTDR -- 0xA3/0xAB/0xB3/0xBB
// ---------------------------------------------------------------------

/// Shared core for OUTI/OUTD: decrements B *first* (per the documented
/// ordering quirk, unlike INI), then writes (HL) out to port BC, advances
/// HL by `delta`. Z and the repeat condition (B!=0) are exact. S/H/PV/N/C/
/// F3/F5 follow the same documented formula as INI/IND but keyed off the
/// new L instead of C (`k = value + L'`) -- see `in_block`'s doc comment
/// for the same accuracy caveat.
fn out_block<B: Bus>(cpu: &mut Cpu, bus: &mut B, delta: i16) {
    let new_b = cpu.b().wrapping_sub(1);
    cpu.set_b(new_b);
    // OUTI/OUTD's second M1 is five T-states long; the extra one precedes the
    // memory read.
    cpu.internal_cycles(bus, 1);

    let value = cpu.mem_read(bus, cpu.hl);
    cpu.io_write(bus, cpu.bc, value);
    cpu.hl = cpu.hl.wrapping_add(delta as u16);

    let l = cpu.hl as u8;
    let k = value as u16 + l as u16;
    let hc = k > 0xFF;
    let pv = parity(((k as u8) & 0x07) ^ new_b);
    let s = new_b & 0x80 != 0;
    let z = new_b == 0;

    // N is documented (officially, not just as an undocumented refinement)
    // to always be set for INI/IND/OUTI/OUTD.
    let flags = flags_byte(s, z, hc, pv, true, hc);
    apply_alu_flags(cpu, flags, new_b);
}

fn outi<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    out_block(cpu, bus, 1);
    16
}

fn outd<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    out_block(cpu, bus, -1);
    16
}

fn otir<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    out_block(cpu, bus, 1);
    if cpu.b() != 0 {
        cpu.pc = cpu.pc.wrapping_sub(2);
        21
    } else {
        16
    }
}

fn otdr<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    out_block(cpu, bus, -1);
    if cpu.b() != 0 {
        cpu.pc = cpu.pc.wrapping_sub(2);
        21
    } else {
        16
    }
}

pub(crate) const fn install<B: Bus>(table: [Handler<B>; 256]) -> [Handler<B>; 256] {
    let mut table = table;

    table[0xA0] = ldi;
    table[0xA8] = ldd;
    table[0xB0] = ldir;
    table[0xB8] = lddr;

    table[0xA1] = cpi;
    table[0xA9] = cpd;
    table[0xB1] = cpir;
    table[0xB9] = cpdr;

    table[0xA2] = ini;
    table[0xAA] = ind;
    table[0xB2] = inir;
    table[0xBA] = indr;

    table[0xA3] = outi;
    table[0xAB] = outd;
    table[0xB3] = otir;
    table[0xBB] = otdr;

    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{FLAG_H, FLAG_N};
    use crate::test_support::FlatBus;

    fn run_ed(cpu: &mut Cpu, bus: &mut FlatBus, ed_opcode: u8) -> u8 {
        bus.write(cpu.pc, 0xED);
        bus.write(cpu.pc.wrapping_add(1), ed_opcode);
        cpu.step(bus)
    }

    #[test]
    fn ldi_copies_byte_advances_pointers_and_decrements_bc() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x1000;
        cpu.de = 0x2000;
        cpu.bc = 0x0002;
        cpu.set_a(0x00);
        let mut bus = FlatBus::new();
        bus.write(0x1000, 0x42);

        let cycles = run_ed(&mut cpu, &mut bus, 0xA0);

        assert_eq!(cycles, 16);
        assert_eq!(bus.read(0x2000), 0x42);
        assert_eq!(cpu.hl, 0x1001);
        assert_eq!(cpu.de, 0x2001);
        assert_eq!(cpu.bc, 0x0001);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // bc != 0
        assert_eq!(cpu.f() & FLAG_H, 0);
        assert_eq!(cpu.f() & FLAG_N, 0);
    }

    #[test]
    fn ldi_clears_pv_when_bc_reaches_zero() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x1000;
        cpu.de = 0x2000;
        cpu.bc = 0x0001;
        let mut bus = FlatBus::new();
        bus.write(0x1000, 0x99);

        run_ed(&mut cpu, &mut bus, 0xA0);

        assert_eq!(cpu.bc, 0x0000);
        assert_eq!(cpu.f() & FLAG_PV, 0);
    }

    #[test]
    fn ldd_moves_pointers_backward() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x1000;
        cpu.de = 0x2000;
        cpu.bc = 0x0002;
        let mut bus = FlatBus::new();
        bus.write(0x1000, 0x55);

        run_ed(&mut cpu, &mut bus, 0xA8);

        assert_eq!(bus.read(0x2000), 0x55);
        assert_eq!(cpu.hl, 0x0FFF);
        assert_eq!(cpu.de, 0x1FFF);
        assert_eq!(cpu.bc, 0x0001);
    }

    #[test]
    fn ldir_repeats_across_multiple_bytes_to_completion() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x1000;
        cpu.de = 0x2000;
        cpu.bc = 0x0003;
        let mut bus = FlatBus::new();
        bus.write(0x1000, 0xAA);
        bus.write(0x1001, 0xBB);
        bus.write(0x1002, 0xCC);
        cpu.pc = 0;
        bus.write(0, 0xED);
        bus.write(1, 0xB0); // LDIR

        // First execution: bc goes 3->2, repeats (pc rewound to 0).
        let c1 = cpu.step(&mut bus);
        assert_eq!(c1, 21);
        assert_eq!(cpu.pc, 0);
        assert_eq!(cpu.bc, 2);

        // Second: bc 2->1, repeats again.
        let c2 = cpu.step(&mut bus);
        assert_eq!(c2, 21);
        assert_eq!(cpu.pc, 0);
        assert_eq!(cpu.bc, 1);

        // Third: bc 1->0, does NOT repeat -- pc left past the instruction.
        let c3 = cpu.step(&mut bus);
        assert_eq!(c3, 16);
        assert_eq!(cpu.pc, 2);
        assert_eq!(cpu.bc, 0);

        assert_eq!(bus.read(0x2000), 0xAA);
        assert_eq!(bus.read(0x2001), 0xBB);
        assert_eq!(bus.read(0x2002), 0xCC);
        assert_eq!(cpu.hl, 0x1003);
        assert_eq!(cpu.de, 0x2003);
    }

    #[test]
    fn cpi_sets_zero_flag_on_match_and_advances_hl() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x42);
        cpu.hl = 0x1000;
        cpu.bc = 0x0002;
        let mut bus = FlatBus::new();
        bus.write(0x1000, 0x42);

        let cycles = run_ed(&mut cpu, &mut bus, 0xA1);

        assert_eq!(cycles, 16);
        assert_eq!(cpu.hl, 0x1001);
        assert_eq!(cpu.bc, 0x0001);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // bc != 0
        assert_eq!(cpu.a(), 0x42); // A unchanged, unlike CP writing back
    }

    #[test]
    fn cpir_finds_match_and_stops_early() {
        let mut cpu = Cpu::new();
        cpu.set_a(0xCC);
        cpu.hl = 0x1000;
        cpu.bc = 0x0003;
        let mut bus = FlatBus::new();
        bus.write(0x1000, 0xAA);
        bus.write(0x1001, 0xBB);
        bus.write(0x1002, 0xCC); // match on 3rd byte
        cpu.pc = 0;
        bus.write(0, 0xED);
        bus.write(1, 0xB1); // CPIR

        let c1 = cpu.step(&mut bus); // no match, bc 3->2, repeats
        assert_eq!(c1, 21);
        assert_eq!(cpu.pc, 0);

        let c2 = cpu.step(&mut bus); // no match, bc 2->1, repeats
        assert_eq!(c2, 21);
        assert_eq!(cpu.pc, 0);

        let c3 = cpu.step(&mut bus); // match! bc 1->0, stops regardless
        assert_eq!(c3, 16);
        assert_eq!(cpu.pc, 2);
        assert_eq!(cpu.bc, 0);
        assert_eq!(cpu.hl, 0x1003);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
    }

    #[test]
    fn cpir_stops_when_bc_exhausted_without_match() {
        let mut cpu = Cpu::new();
        cpu.set_a(0xFF); // never matches
        cpu.hl = 0x1000;
        cpu.bc = 0x0001;
        let mut bus = FlatBus::new();
        bus.write(0x1000, 0x00);
        cpu.pc = 0;
        bus.write(0, 0xED);
        bus.write(1, 0xB1);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 16); // bc reached 0, no repeat
        assert_eq!(cpu.pc, 2);
        assert_eq!(cpu.f() & FLAG_Z, 0);
    }

    #[test]
    fn ini_reads_port_writes_memory_and_decrements_b() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x3000;
        cpu.set_b(0x02);
        cpu.set_c(0x10);
        let mut bus = FlatBus::new(); // read_io always returns 0

        let cycles = run_ed(&mut cpu, &mut bus, 0xA2);

        assert_eq!(cycles, 16);
        assert_eq!(bus.read(0x3000), 0x00);
        assert_eq!(cpu.hl, 0x3001);
        assert_eq!(cpu.b(), 0x01);
        assert_eq!(cpu.f() & FLAG_Z, 0); // b != 0
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
    }

    #[test]
    fn inir_repeats_until_b_reaches_zero() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x3000;
        cpu.set_b(0x02);
        cpu.pc = 0;
        let mut bus = FlatBus::new();
        bus.write(0, 0xED);
        bus.write(1, 0xB2); // INIR

        let c1 = cpu.step(&mut bus);
        assert_eq!(c1, 21);
        assert_eq!(cpu.pc, 0);
        assert_eq!(cpu.b(), 1);

        let c2 = cpu.step(&mut bus);
        assert_eq!(c2, 16);
        assert_eq!(cpu.pc, 2);
        assert_eq!(cpu.b(), 0);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
    }

    #[test]
    fn outi_decrements_b_first_and_advances_hl() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x4000;
        cpu.set_b(0x05);
        cpu.set_c(0x20);
        let mut bus = FlatBus::new();
        bus.write(0x4000, 0x77);

        let cycles = run_ed(&mut cpu, &mut bus, 0xA3);

        assert_eq!(cycles, 16);
        assert_eq!(cpu.hl, 0x4001);
        assert_eq!(cpu.b(), 0x04); // decremented
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
    }

    #[test]
    fn otir_repeats_until_b_reaches_zero() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x4000;
        cpu.set_b(0x02);
        cpu.pc = 0;
        let mut bus = FlatBus::new();
        bus.write(0x4000, 0x11);
        bus.write(0x4001, 0x22);
        bus.write(0, 0xED);
        bus.write(1, 0xB3); // OTIR

        let c1 = cpu.step(&mut bus);
        assert_eq!(c1, 21);
        assert_eq!(cpu.pc, 0);
        assert_eq!(cpu.b(), 1);

        let c2 = cpu.step(&mut bus);
        assert_eq!(c2, 16);
        assert_eq!(cpu.pc, 2);
        assert_eq!(cpu.b(), 0);
    }

    #[test]
    fn ldd_lddr_and_ind_outd_share_the_same_decrement_direction() {
        // Sanity check the -1 delta path once more, via LDDR this time.
        let mut cpu = Cpu::new();
        cpu.hl = 0x1002;
        cpu.de = 0x2002;
        cpu.bc = 0x0002;
        cpu.pc = 0;
        let mut bus = FlatBus::new();
        bus.write(0x1001, 0x01);
        bus.write(0x1002, 0x02);
        bus.write(0, 0xED);
        bus.write(1, 0xB8); // LDDR

        let c1 = cpu.step(&mut bus);
        assert_eq!(c1, 21);
        assert_eq!(cpu.pc, 0);

        let c2 = cpu.step(&mut bus);
        assert_eq!(c2, 16);
        assert_eq!(cpu.pc, 2);
        assert_eq!(bus.read(0x2002), 0x02);
        assert_eq!(bus.read(0x2001), 0x01);
        assert_eq!(cpu.hl, 0x1000);
        assert_eq!(cpu.de, 0x2000);
    }
}
