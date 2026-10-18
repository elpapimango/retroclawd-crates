//! MEMPTR (WZ), the hidden address register: the value each instruction leaves in it, per Boo-boo's "MEMPTR, esoteric register of
//! the Zilog Z80 CPU" (draft English translation by Vladimir Kladov, <https://github.com/floooh/emu-info/blob/master/z80/
//! memptr_eng.txt>), checked on the real chip by the authors and by Patrik Rak's z80test (`z80memptr`). BIT n,(HL) shows bits
//! 11 and 13 of it in F3/F5.

use z80::{Bus, Cpu};

struct Ram {
    mem: Vec<u8>,
    io: Vec<u8>,
}

impl Ram {
    fn new() -> Ram {
        Ram {
            mem: vec![0; 0x10000],
            io: vec![0; 0x10000],
        }
    }
}

impl Bus for Ram {
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
    }
}

/// Runs the instruction bytes at 0x0100 once from `setup`, returns the CPU.
fn run(code: &[u8], setup: impl FnOnce(&mut Cpu, &mut Ram)) -> Cpu {
    let mut cpu = Cpu::new();
    let mut bus = Ram::new();
    bus.mem[0x100..0x100 + code.len()].copy_from_slice(code);
    cpu.pc = 0x100;
    cpu.sp = 0x8000;
    cpu.set_memptr(0xABCD);
    setup(&mut cpu, &mut bus);
    cpu.step(&mut bus);
    cpu
}

#[test]
fn loads_and_stores_through_an_address() {
    // LD A,(addr): addr + 1
    assert_eq!(run(&[0x3A, 0x34, 0x12], |_, _| {}).memptr(), 0x1235);
    // LD (addr),A: low = addr + 1, high = A
    let cpu = run(&[0x32, 0xFF, 0x12], |c, _| c.af = 0x5600);
    assert_eq!(cpu.memptr(), 0x5600);
    // LD A,(BC) / LD A,(DE): rp + 1
    assert_eq!(run(&[0x0A], |c, _| c.bc = 0x2000).memptr(), 0x2001);
    assert_eq!(run(&[0x1A], |c, _| c.de = 0x20FF).memptr(), 0x2100);
    // LD (BC),A / LD (DE),A: low = rp + 1, high = A
    assert_eq!(
        run(&[0x02], |c, _| {
            c.bc = 0x20FF;
            c.af = 0x7700
        })
        .memptr(),
        0x7700
    );
    assert_eq!(
        run(&[0x12], |c, _| {
            c.de = 0x2010;
            c.af = 0x7700
        })
        .memptr(),
        0x7711
    );
    // LD HL,(addr) / LD (addr),HL and the ED forms: addr + 1
    assert_eq!(run(&[0x2A, 0x00, 0x30], |_, _| {}).memptr(), 0x3001);
    assert_eq!(run(&[0x22, 0xFF, 0xFF], |_, _| {}).memptr(), 0x0000);
    assert_eq!(run(&[0xED, 0x4B, 0x10, 0x40], |_, _| {}).memptr(), 0x4011);
    assert_eq!(run(&[0xED, 0x73, 0x10, 0x40], |_, _| {}).memptr(), 0x4011);
}

#[test]
fn jumps_calls_and_returns_set_the_target_taken_or_not() {
    // JP / CALL / conditional ones, even when the condition fails
    assert_eq!(run(&[0xC3, 0x78, 0x56], |_, _| {}).memptr(), 0x5678);
    assert_eq!(run(&[0xCD, 0x78, 0x56], |_, _| {}).memptr(), 0x5678);
    assert_eq!(
        run(&[0xCA, 0x78, 0x56], |c, _| c.set_f(0)).memptr(),
        0x5678,
        "JP Z not taken"
    );
    assert_eq!(
        run(&[0xCC, 0x78, 0x56], |c, _| c.set_f(0)).memptr(),
        0x5678,
        "CALL Z not taken"
    );
    // JR / DJNZ taken: the address jumped to; not taken: unchanged
    assert_eq!(run(&[0x18, 0x10], |_, _| {}).memptr(), 0x0112);
    assert_eq!(
        run(&[0x28, 0x10], |c, _| c.set_f(0)).memptr(),
        0xABCD,
        "JR Z not taken"
    );
    assert_eq!(run(&[0x10, 0x10], |c, _| c.bc = 0x0200).memptr(), 0x0112);
    assert_eq!(
        run(&[0x10, 0x10], |c, _| c.bc = 0x0100).memptr(),
        0xABCD,
        "DJNZ falls through"
    );
    // RET, RETI, RET cc taken / not taken, RST
    let ret = |code: &[u8], f: u8| {
        run(code, |c, b| {
            c.set_f(f);
            b.mem[0x8000] = 0x21;
            b.mem[0x8001] = 0x43;
        })
    };
    assert_eq!(ret(&[0xC9], 0).memptr(), 0x4321);
    assert_eq!(ret(&[0xED, 0x4D], 0).memptr(), 0x4321);
    assert_eq!(ret(&[0xC8], 0x40).memptr(), 0x4321, "RET Z taken");
    assert_eq!(ret(&[0xC8], 0).memptr(), 0xABCD, "RET Z not taken");
    assert_eq!(run(&[0xEF], |_, _| {}).memptr(), 0x0028);
}

#[test]
fn arithmetic_exchange_and_rotate_rules() {
    // ADD HL,rp / ADC / SBC: HL before + 1
    assert_eq!(run(&[0x09], |c, _| c.hl = 0x1234).memptr(), 0x1235);
    assert_eq!(run(&[0xED, 0x4A], |c, _| c.hl = 0x1234).memptr(), 0x1235);
    assert_eq!(run(&[0xED, 0x42], |c, _| c.hl = 0x1234).memptr(), 0x1235);
    // EX (SP),HL: HL after
    let cpu = run(&[0xE3], |c, b| {
        c.hl = 0x1111;
        b.mem[0x8000] = 0x66;
        b.mem[0x8001] = 0x55;
    });
    assert_eq!(cpu.memptr(), 0x5566);
    // RLD / RRD: HL + 1
    assert_eq!(run(&[0xED, 0x6F], |c, _| c.hl = 0x3000).memptr(), 0x3001);
    assert_eq!(run(&[0xED, 0x67], |c, _| c.hl = 0x3000).memptr(), 0x3001);
    // Any (IX+d): IX + d
    assert_eq!(
        run(&[0xDD, 0x7E, 0xFE], |c, _| c.ix = 0x4000).memptr(),
        0x3FFE
    );
    assert_eq!(
        run(&[0xFD, 0x86, 0x05], |c, _| c.iy = 0x4000).memptr(),
        0x4005
    );
}

#[test]
fn port_instructions() {
    // IN A,(n): (A << 8) + n + 1
    let cpu = run(&[0xDB, 0xFE], |c, _| c.af = 0x1200);
    assert_eq!(cpu.memptr(), 0x12FF);
    // OUT (n),A: low = n + 1, high = A
    let cpu = run(&[0xD3, 0xFF], |c, _| c.af = 0x1200);
    assert_eq!(cpu.memptr(), 0x1200);
    // IN r,(C) / OUT (C),r: BC + 1
    assert_eq!(run(&[0xED, 0x78], |c, _| c.bc = 0x1234).memptr(), 0x1235);
    assert_eq!(run(&[0xED, 0x79], |c, _| c.bc = 0x1234).memptr(), 0x1235);
}

#[test]
fn block_instructions() {
    // LDI leaves it alone; LDIR repeating = the instruction's address + 1, on its last pass unchanged
    assert_eq!(run(&[0xED, 0xA0], |c, _| c.bc = 5).memptr(), 0xABCD);
    assert_eq!(run(&[0xED, 0xB0], |c, _| c.bc = 5).memptr(), 0x0101);
    assert_eq!(run(&[0xED, 0xB0], |c, _| c.bc = 1).memptr(), 0xABCD);
    // CPI + 1, CPD - 1; CPIR repeating = PC + 1
    assert_eq!(run(&[0xED, 0xA1], |c, _| c.bc = 5).memptr(), 0xABCE);
    assert_eq!(run(&[0xED, 0xA9], |c, _| c.bc = 5).memptr(), 0xABCC);
    assert_eq!(
        run(&[0xED, 0xB1], |c, _| {
            c.bc = 5;
            c.af = 0x0100
        })
        .memptr(),
        0x0101
    );
    // INI/IND: BC before B-- plus/minus 1; OUTI/OUTD: BC after B-- plus/minus 1
    assert_eq!(run(&[0xED, 0xA2], |c, _| c.bc = 0x0510).memptr(), 0x0511);
    assert_eq!(run(&[0xED, 0xAA], |c, _| c.bc = 0x0510).memptr(), 0x050F);
    assert_eq!(run(&[0xED, 0xA3], |c, _| c.bc = 0x0510).memptr(), 0x0411);
    assert_eq!(run(&[0xED, 0xAB], |c, _| c.bc = 0x0510).memptr(), 0x040F);
}

#[test]
fn bit_n_hl_shows_bits_11_and_13_of_memptr() {
    // BIT 0,(HL) with MEMPTR = 0x2800 (bits 11 and 13 set) -> F3 and F5 set; with 0 -> clear.
    let cpu = run(&[0xCB, 0x46], |c, _| {
        c.hl = 0x0200;
        c.set_memptr(0x2800);
    });
    assert_eq!(cpu.f() & 0x28, 0x28);
    let cpu = run(&[0xCB, 0x46], |c, _| {
        c.hl = 0x0200;
        c.set_memptr(0x0000);
    });
    assert_eq!(cpu.f() & 0x28, 0x00);
}
