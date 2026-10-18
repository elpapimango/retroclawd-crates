use super::{Handler, IndexReg, indexed_addr, reg_set};
use crate::Bus;
use crate::cpu::Cpu;

pub(crate) const fn install<B: Bus>(table: [Handler<B>; 256]) -> [Handler<B>; 256] {
    let mut table = table;

    table[0x01] = ld_bc_nn;
    table[0x11] = ld_de_nn;
    table[0x21] = ld_hl_nn;
    table[0x31] = ld_sp_nn;

    table[0x02] = ld_bc_ind_a;
    table[0x12] = ld_de_ind_a;
    table[0x32] = ld_nn_ind_a;

    table[0x0A] = ld_a_bc_ind;
    table[0x1A] = ld_a_de_ind;
    table[0x3A] = ld_a_nn_ind;

    table[0x22] = ld_nn_ind_hl;
    table[0x2A] = ld_hl_nn_ind;

    table[0x06] = ld_b_n;
    table[0x0E] = ld_c_n;
    table[0x16] = ld_d_n;
    table[0x1E] = ld_e_n;
    table[0x26] = ld_h_n;
    table[0x2E] = ld_l_n;
    table[0x36] = ld_hl_ind_n;
    table[0x3E] = ld_a_n;

    table
}

/// Fetches a little-endian 16-bit immediate at `cpu.pc`, advancing `pc` by 2.
fn fetch_imm16<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u16 {
    let lo = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    let hi = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    (lo as u16) | ((hi as u16) << 8)
}

/// Fetches an 8-bit immediate at `cpu.pc`, advancing `pc` by 1.
fn fetch_imm8<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = cpu.mem_read(bus, cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1);
    n
}

fn ld_bc_nn<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.bc = fetch_imm16(cpu, bus);
    10
}

fn ld_de_nn<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.de = fetch_imm16(cpu, bus);
    10
}

fn ld_hl_nn<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let val = fetch_imm16(cpu, bus);
    cpu.set_hl_or_index(val);
    10
}

fn ld_sp_nn<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.sp = fetch_imm16(cpu, bus);
    10
}

/// MEMPTR after `LD (addr),A`, `LD (rp),A` and `OUT (n),A`: the low byte is addr + 1, the high byte A.
pub(crate) fn memptr_after_store_a(addr: u16, a: u8) -> u16 {
    (u16::from(a) << 8) | (addr.wrapping_add(1) & 0xFF)
}

fn ld_bc_ind_a<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.memptr = memptr_after_store_a(cpu.bc, cpu.a());
    cpu.mem_write(bus, cpu.bc, cpu.a());
    7
}

fn ld_de_ind_a<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.memptr = memptr_after_store_a(cpu.de, cpu.a());
    cpu.mem_write(bus, cpu.de, cpu.a());
    7
}

fn ld_nn_ind_a<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let addr = fetch_imm16(cpu, bus);
    cpu.memptr = memptr_after_store_a(addr, cpu.a());
    cpu.mem_write(bus, addr, cpu.a());
    13
}

fn ld_a_bc_ind<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.memptr = cpu.bc.wrapping_add(1);
    let val = cpu.mem_read(bus, cpu.bc);
    cpu.set_a(val);
    7
}

fn ld_a_de_ind<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    cpu.memptr = cpu.de.wrapping_add(1);
    let val = cpu.mem_read(bus, cpu.de);
    cpu.set_a(val);
    7
}

fn ld_a_nn_ind<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let addr = fetch_imm16(cpu, bus);
    cpu.memptr = addr.wrapping_add(1);
    let val = cpu.mem_read(bus, addr);
    cpu.set_a(val);
    13
}

fn ld_nn_ind_hl<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let addr = fetch_imm16(cpu, bus);
    cpu.memptr = addr.wrapping_add(1);
    let val = cpu.hl_or_index();
    cpu.mem_write(bus, addr, val as u8);
    cpu.mem_write(bus, addr.wrapping_add(1), (val >> 8) as u8);
    16
}

fn ld_hl_nn_ind<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let addr = fetch_imm16(cpu, bus);
    cpu.memptr = addr.wrapping_add(1);
    let lo = cpu.mem_read(bus, addr);
    let hi = cpu.mem_read(bus, addr.wrapping_add(1));
    cpu.set_hl_or_index((lo as u16) | ((hi as u16) << 8));
    16
}

fn ld_b_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_imm8(cpu, bus);
    reg_set(cpu, bus, 0, n);
    7
}

fn ld_c_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_imm8(cpu, bus);
    reg_set(cpu, bus, 1, n);
    7
}

fn ld_d_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_imm8(cpu, bus);
    reg_set(cpu, bus, 2, n);
    7
}

fn ld_e_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_imm8(cpu, bus);
    reg_set(cpu, bus, 3, n);
    7
}

fn ld_h_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_imm8(cpu, bus);
    reg_set(cpu, bus, 4, n);
    7
}

fn ld_l_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_imm8(cpu, bus);
    reg_set(cpu, bus, 5, n);
    7
}

fn ld_hl_ind_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    // NOTE: can't use the plain fetch-immediate-then-reg_set pattern here.
    // Under a DD/FD prefix this opcode's encoding is `DD/FD 36 d n` --
    // displacement byte *before* the immediate -- but reg_set's (HL) path
    // fetches its own displacement byte from cpu.pc internally. Fetching `n`
    // first (as for the plain 8-bit LD r,n opcodes) would hand it the
    // displacement byte as data and consume the real `n` as the
    // displacement, silently swapping the two. So the address is resolved
    // first here, then the immediate.
    let addr = match cpu.active_index {
        None => cpu.hl,
        Some(IndexReg::Ix) => indexed_addr(cpu, bus, cpu.ix),
        Some(IndexReg::Iy) => indexed_addr(cpu, bus, cpu.iy),
    };
    let n = fetch_imm8(cpu, bus);
    cpu.mem_write(bus, addr, n);
    10
}

fn ld_a_n<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let n = fetch_imm8(cpu, bus);
    reg_set(cpu, bus, 7, n);
    7
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FlatBus;

    fn ld_bc_nn_loads_little_endian_immediate_and_takes_10_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x01); // LD BC,nn
        bus.write(1, 0x34);
        bus.write(2, 0x12);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10);
        assert_eq!(cpu.bc, 0x1234);
        assert_eq!(cpu.pc, 3);
    }

    fn ld_de_nn_loads_immediate() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x11); // LD DE,nn
        bus.write(1, 0x78);
        bus.write(2, 0x56);

        cpu.step(&mut bus);

        assert_eq!(cpu.de, 0x5678);
    }

    fn ld_hl_nn_loads_hl_normally() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x21); // LD HL,nn
        bus.write(1, 0xCD);
        bus.write(2, 0xAB);

        cpu.step(&mut bus);

        assert_eq!(cpu.hl, 0xABCD);
    }

    #[test]
    fn dd_ld_hl_nn_loads_ix_instead_and_costs_14_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x21); // LD IX,nn
        bus.write(2, 0xCD);
        bus.write(3, 0xAB);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 14); // 4 prefix + 10
        assert_eq!(cpu.ix, 0xABCD);
        assert_eq!(cpu.hl, 0); // real HL untouched
    }

    fn ld_sp_nn_loads_immediate() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x31); // LD SP,nn
        bus.write(1, 0x00);
        bus.write(2, 0x80);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10);
        assert_eq!(cpu.sp, 0x8000);
    }
    /// Merged (every check kept): `ld_bc_nn_loads_little_endian_immediate_and_takes_10_cycles`, `ld_de_nn_loads_immediate`, `ld_hl_nn_loads_hl_normally`, `ld_sp_nn_loads_immediate`.
    #[test]
    fn ld_rr_nn_loads_immediate() {
        ld_bc_nn_loads_little_endian_immediate_and_takes_10_cycles();
        ld_de_nn_loads_immediate();
        ld_hl_nn_loads_hl_normally();
        ld_sp_nn_loads_immediate();
    }

    fn ld_bc_ind_a_writes_a_to_bc_address_and_takes_7_cycles() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x2000;
        cpu.set_a(0x42);
        let mut bus = FlatBus::new();
        bus.write(0, 0x02); // LD (BC),A

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(bus.read(0x2000), 0x42);
    }

    fn ld_de_ind_a_writes_a_to_de_address() {
        let mut cpu = Cpu::new();
        cpu.de = 0x3000;
        cpu.set_a(0x99);
        let mut bus = FlatBus::new();
        bus.write(0, 0x12); // LD (DE),A

        cpu.step(&mut bus);

        assert_eq!(bus.read(0x3000), 0x99);
    }
    /// Merged (every check kept): `ld_bc_ind_a_writes_a_to_bc_address_and_takes_7_cycles`, `ld_de_ind_a_writes_a_to_de_address`.
    #[test]
    fn ld_bc_and_de_ind_a_write_a() {
        ld_bc_ind_a_writes_a_to_bc_address_and_takes_7_cycles();
        ld_de_ind_a_writes_a_to_de_address();
    }

    #[test]
    fn ld_nn_ind_a_writes_a_to_immediate_address_and_takes_13_cycles() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x55);
        let mut bus = FlatBus::new();
        bus.write(0, 0x32); // LD (nn),A
        bus.write(1, 0x00);
        bus.write(2, 0x40);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 13);
        assert_eq!(bus.read(0x4000), 0x55);
    }

    fn ld_a_bc_ind_reads_from_bc_address_and_takes_7_cycles() {
        let mut cpu = Cpu::new();
        cpu.bc = 0x2000;
        let mut bus = FlatBus::new();
        bus.write(0x2000, 0x77);
        bus.write(0, 0x0A); // LD A,(BC)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(cpu.a(), 0x77);
    }

    fn ld_a_de_ind_reads_from_de_address() {
        let mut cpu = Cpu::new();
        cpu.de = 0x3000;
        let mut bus = FlatBus::new();
        bus.write(0x3000, 0x88);
        bus.write(0, 0x1A); // LD A,(DE)

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x88);
    }
    /// Merged (every check kept): `ld_a_bc_ind_reads_from_bc_address_and_takes_7_cycles`, `ld_a_de_ind_reads_from_de_address`.
    #[test]
    fn ld_a_bc_and_de_ind_read_a() {
        ld_a_bc_ind_reads_from_bc_address_and_takes_7_cycles();
        ld_a_de_ind_reads_from_de_address();
    }

    #[test]
    fn ld_a_nn_ind_reads_from_immediate_address_and_takes_13_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0x4000, 0x66);
        bus.write(0, 0x3A); // LD A,(nn)
        bus.write(1, 0x00);
        bus.write(2, 0x40);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 13);
        assert_eq!(cpu.a(), 0x66);
    }

    #[test]
    fn ld_nn_ind_hl_writes_little_endian_and_takes_16_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0xBEEF;
        let mut bus = FlatBus::new();
        bus.write(0, 0x22); // LD (nn),HL
        bus.write(1, 0x00);
        bus.write(2, 0x50);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 16);
        assert_eq!(bus.read(0x5000), 0xEF);
        assert_eq!(bus.read(0x5001), 0xBE);
    }

    #[test]
    fn ld_hl_nn_ind_reads_little_endian_and_takes_16_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0x6000, 0xCD);
        bus.write(0x6001, 0xAB);
        bus.write(0, 0x2A); // LD HL,(nn)
        bus.write(1, 0x00);
        bus.write(2, 0x60);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 16);
        assert_eq!(cpu.hl, 0xABCD);
    }

    #[test]
    fn fd_ld_iy_nn_ind_reads_into_iy_not_hl() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0x6000, 0x11);
        bus.write(0x6001, 0x22);
        bus.write(0, 0xFD);
        bus.write(1, 0x2A); // LD IY,(nn)
        bus.write(2, 0x00);
        bus.write(3, 0x60);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 20); // 4 prefix + 16
        assert_eq!(cpu.iy, 0x2211);
        assert_eq!(cpu.hl, 0);
    }

    fn ld_b_n_loads_immediate_and_takes_7_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x06); // LD B,n
        bus.write(1, 0x42);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(cpu.b(), 0x42);
    }

    fn ld_c_n_loads_immediate() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x0E); // LD C,n
        bus.write(1, 0x11);

        cpu.step(&mut bus);

        assert_eq!(cpu.c(), 0x11);
    }

    fn ld_d_n_loads_immediate() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x16); // LD D,n
        bus.write(1, 0x22);

        cpu.step(&mut bus);

        assert_eq!(cpu.d(), 0x22);
    }

    fn ld_e_n_loads_immediate() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x1E); // LD E,n
        bus.write(1, 0x33);

        cpu.step(&mut bus);

        assert_eq!(cpu.e(), 0x33);
    }

    fn ld_h_n_loads_immediate() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x26); // LD H,n
        bus.write(1, 0x44);

        cpu.step(&mut bus);

        assert_eq!(cpu.h(), 0x44);
    }

    fn ld_l_n_loads_immediate() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x2E); // LD L,n
        bus.write(1, 0x55);

        cpu.step(&mut bus);

        assert_eq!(cpu.l(), 0x55);
    }
    /// Merged (every check kept): `ld_b_n_loads_immediate_and_takes_7_cycles`, `ld_c_n_loads_immediate`, `ld_d_n_loads_immediate`, `ld_e_n_loads_immediate`, `ld_h_n_loads_immediate`, `ld_l_n_loads_immediate`.
    #[test]
    fn ld_r_n_loads_immediate() {
        ld_b_n_loads_immediate_and_takes_7_cycles();
        ld_c_n_loads_immediate();
        ld_d_n_loads_immediate();
        ld_e_n_loads_immediate();
        ld_h_n_loads_immediate();
        ld_l_n_loads_immediate();
    }

    #[test]
    fn ld_a_n_loads_immediate_and_takes_7_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x3E); // LD A,n
        bus.write(1, 0x99);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(cpu.a(), 0x99);
    }

    #[test]
    fn ld_hl_ind_n_writes_immediate_to_hl_address_and_takes_10_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x7000;
        let mut bus = FlatBus::new();
        bus.write(0, 0x36); // LD (HL),n
        bus.write(1, 0xAB);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 10);
        assert_eq!(bus.read(0x7000), 0xAB);
    }

    #[test]
    fn dd_ld_hl_ind_n_writes_to_ix_plus_d_and_does_not_swap_d_and_n() {
        // DD 36 d n -- LD (IX+d),n. Regression test for a d/n ordering bug:
        // the displacement byte must be consumed as the address offset and
        // the immediate byte as the value written, not the reverse.
        let mut cpu = Cpu::new();
        cpu.ix = 0x8000;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x36);
        bus.write(2, 0x05); // d = +5
        bus.write(3, 0x7F); // n

        let cycles = cpu.step(&mut bus);

        // 4 (DD prefix) + 10 (base LD (HL),n cost) + 8 (indexed_addr's
        // flat displacement/address-calc charge) = 22 T-states under this
        // core's dispatch model.
        assert_eq!(cycles, 22);
        assert_eq!(bus.read(0x8005), 0x7F);
        assert_eq!(cpu.pc, 4);
    }

    #[test]
    fn fd_ld_hl_ind_n_writes_to_iy_plus_negative_d() {
        // FD 36 d n with a negative displacement, to confirm sign handling
        // and that plain (HL) is untouched.
        let mut cpu = Cpu::new();
        cpu.iy = 0x9010;
        cpu.hl = 0x1234;
        let mut bus = FlatBus::new();
        bus.write(0, 0xFD);
        bus.write(1, 0x36);
        bus.write(2, 0xFB); // d = -5
        bus.write(3, 0x11); // n

        cpu.step(&mut bus);

        assert_eq!(bus.read(0x900B), 0x11);
        assert_eq!(bus.read(0x1234), 0x00); // real HL address untouched
    }

    #[test]
    fn dd_ld_ixh_n_loads_ixh_not_h_and_takes_11_cycles() {
        // DD 26 n -- undocumented LD IXH,n
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x26);
        bus.write(2, 0x42);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 11); // 4 prefix + 7
        assert_eq!(cpu.ixh(), 0x42);
        assert_eq!(cpu.h(), 0);
    }
}
