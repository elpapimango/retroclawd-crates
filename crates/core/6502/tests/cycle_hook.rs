//! The 6502 core reports every bus cycle -- `read`, `write` or
//! `Bus::dummy_cycle` -- in real order, so a cycle-accurate bus (the C64's
//! VIC-II stealing read cycles) can count and stall on them. The strong
//! guard: for every one of the 256 opcodes, in every operand scenario that
//! changes the cycle count (page cross or not, branch taken or not), the
//! number of bus events equals the cycles `step` reports.

use machine::Bus;
use mos6502::Cpu;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    Read(u16),
    Write(u16, u8),
    DummyRead(u16),
    DummyWrite(u16, u8),
}

struct TraceBus {
    mem: Vec<u8>,
    events: Vec<Ev>,
}

impl TraceBus {
    fn new(fill: u8) -> Self {
        Self {
            mem: vec![fill; 0x10000],
            events: Vec::new(),
        }
    }
}

impl Bus for TraceBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.events.push(Ev::Read(addr));
        self.mem[addr as usize]
    }
    fn write(&mut self, addr: u16, val: u8) {
        self.events.push(Ev::Write(addr, val));
        self.mem[addr as usize] = val;
    }
    fn dummy_cycle(&mut self, addr: u16, write: bool, value: u8) {
        self.events.push(if write {
            Ev::DummyWrite(addr, value)
        } else {
            Ev::DummyRead(addr)
        });
    }
}

const CODE: u16 = 0x0200;

#[test]
fn nmi_hijacks_brk_and_irq_only_before_the_status_push() {
    struct NmiBus {
        trace: TraceBus,
        edge_at: usize,
        pending: bool,
        status_push: usize,
    }
    impl NmiBus {
        fn tick(&mut self) {
            self.pending |= self.trace.events.len() == self.edge_at;
        }
    }
    impl Bus for NmiBus {
        fn read(&mut self, addr: u16) -> u8 {
            let value = self.trace.read(addr);
            self.tick();
            value
        }
        fn write(&mut self, addr: u16, value: u8) {
            self.trace.write(addr, value);
            self.tick();
        }
        fn dummy_cycle(&mut self, addr: u16, write: bool, value: u8) {
            self.trace.dummy_cycle(addr, write, value);
            self.tick();
        }
        fn take_nmi_edge(&mut self) -> bool {
            std::mem::take(&mut self.pending)
        }
        fn interrupt_status_pushed(&mut self) {
            self.status_push = self.trace.events.len();
        }
    }
    for brk in [true, false] {
        for edge_at in 1..=7 {
            let mut bus = NmiBus {
                trace: TraceBus::new(0xea),
                edge_at,
                pending: false,
                status_push: 0,
            };
            bus.trace.mem[CODE as usize] = 0x00;
            bus.trace.mem[0xfffa..=0xfffb].copy_from_slice(&[0x00, 0x40]);
            bus.trace.mem[0xfffe..=0xffff].copy_from_slice(&[0x00, 0x50]);
            let mut cpu = Cpu::new();
            cpu.pc = CODE;
            cpu.p = 0x20;
            if !brk {
                cpu.request_irq();
            }
            assert_eq!(cpu.step(&mut bus), 7);
            let hijacked = edge_at <= 4;
            assert_eq!(
                cpu.pc,
                if hijacked { 0x4000 } else { 0x5000 },
                "BRK={brk}, NMI after cycle {edge_at}"
            );
            assert_eq!(bus.trace.events.len(), 7);
            assert_eq!(bus.status_push, 5);
            assert_eq!(bus.trace.mem[0x01fd], 0x02);
            assert_eq!(bus.trace.mem[0x01fc], if brk { 0x02 } else { 0x00 });
            assert_eq!(bus.trace.mem[0x01fb], if brk { 0x30 } else { 0x20 });
            let vector = if hijacked { 0xfffa } else { 0xfffe };
            assert_eq!(
                &bus.trace.events[5..],
                &[Ev::Read(vector), Ev::Read(vector + 1)]
            );
            assert_eq!(bus.pending, !hijacked, "late NMI must remain pending");
        }
    }
}

#[test]
fn irq_poll_uses_old_i_for_cli_sei_and_plp_but_restored_i_for_rti() {
    for opcode in [0x58, 0x28] {
        let mut bus = TraceBus::new(0xea);
        bus.mem[CODE as usize] = opcode;
        bus.mem[0x01fe] = 0x20;
        bus.mem[0xfffe..=0xffff].copy_from_slice(&[0x00, 0x40]);
        let mut cpu = Cpu::new();
        cpu.pc = CODE;
        cpu.request_irq();
        cpu.step(&mut bus);
        cpu.request_irq();
        assert_eq!(cpu.step(&mut bus), 2, "opcode {opcode:02x} must defer IRQ");
        assert_eq!(cpu.pc, CODE + 2);
        cpu.request_irq();
        bus.events.clear();
        assert_eq!(cpu.step(&mut bus), 7);
        assert_eq!(cpu.pc, 0x4000);
        assert_eq!(bus.events.len(), 7);
    }

    for opcode in [0x78, 0x28] {
        let mut bus = TraceBus::new(0xea);
        bus.mem[CODE as usize..CODE as usize + 2].copy_from_slice(&[0x58, opcode]);
        bus.mem[0x01fe] = 0x24;
        bus.mem[0xfffe..=0xffff].copy_from_slice(&[0x00, 0x40]);
        let mut cpu = Cpu::new();
        cpu.pc = CODE;
        cpu.request_irq();
        cpu.step(&mut bus);
        cpu.request_irq();
        cpu.step(&mut bus);
        assert_eq!(cpu.pc, CODE + 2, "CLI must allow SEI/PLP to execute");
        cpu.request_irq();
        assert_eq!(cpu.step(&mut bus), 7);
        assert_eq!(cpu.pc, 0x4000);
        assert_eq!(
            bus.mem[0x0100 | cpu.sp.wrapping_add(1) as usize] & 0x14,
            0x04
        );
    }

    let mut bus = TraceBus::new(0xea);
    bus.mem[CODE as usize] = 0x40;
    bus.mem[0x01fe] = 0x20;
    bus.mem[0x01ff] = 0x00;
    bus.mem[0x0100] = 0x30;
    bus.mem[0xfffe..=0xffff].copy_from_slice(&[0x00, 0x40]);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.request_irq();
    assert_eq!(cpu.step(&mut bus), 6);
    assert_eq!(cpu.pc, 0x3000);
    cpu.request_irq();
    assert_eq!(cpu.step(&mut bus), 7);
    assert_eq!(cpu.pc, 0x4000);
    assert_eq!(bus.mem[0x01fe] & 0x14, 0);
}

#[test]
fn reset_nmi_and_status_edits_discard_a_previous_unmasked_irq_poll() {
    for action in ["reset", "nmi", "P", "SR"] {
        let mut bus = TraceBus::new(0xea);
        bus.mem[CODE as usize] = 0x78;
        let mut cpu = Cpu::new();
        cpu.pc = CODE;
        cpu.p = 0x20;
        cpu.step(&mut bus);
        match action {
            "reset" => cpu.reset(&mut bus),
            "nmi" => {
                cpu.request_nmi();
                assert_eq!(cpu.step(&mut bus), 7);
            }
            register => cpu.set_register(register, 0x24).unwrap(),
        }
        cpu.request_irq();
        assert_eq!(
            cpu.step(&mut bus),
            2,
            "{action} must use its current I flag"
        );
    }
}

/// Runs one instruction at `CODE` and returns (cycles, events).
fn run(opcode: u8, fill: u8, xy: u8, p: u8, sp: u8) -> (u32, Vec<Ev>) {
    let mut bus = TraceBus::new(fill);
    bus.mem[CODE as usize] = opcode;
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.x = xy;
    cpu.y = xy;
    cpu.sp = sp;
    cpu.p = p;
    bus.events.clear();
    let cycles = cpu.step(&mut bus);
    (cycles, bus.events)
}

#[test]
fn every_opcode_emits_one_bus_event_per_reported_cycle() {
    let mut checked = 0;
    for opcode in 0..=255u8 {
        // fill 0xF0: abs operand $F0F0, zp pointer -> $F0F0, branch -16
        // (page cross); fill 0x04: small operands, no cross, branch +4.
        for fill in [0xF0u8, 0x04] {
            // index 0x80 crosses a page from $F0F0; 0x00 never does.
            for xy in [0x80u8, 0x00] {
                // flag sets chosen so every conditional branch is both
                // taken and not taken across the two.
                for p in [0x20u8, 0xE3, 0x20 | 0xC3, 0x2C] {
                    let (cycles, events) = run(opcode, fill, xy, p, 0xFD);
                    assert_eq!(
                        events.len() as u32,
                        cycles,
                        "opcode {opcode:#04x} fill {fill:#04x} xy {xy:#04x} p {p:#04x}: \
                         {cycles} cycles but {} events: {events:?}",
                        events.len()
                    );
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 256 * 16);
}

#[test]
fn interrupt_sequences_and_a_jammed_cpu_also_emit_one_event_per_cycle() {
    for nmi in [true, false] {
        let mut bus = TraceBus::new(0xEA);
        let mut cpu = Cpu::new();
        cpu.pc = CODE;
        cpu.p = 0x20; // I clear so the IRQ is taken
        if nmi {
            cpu.request_nmi();
        } else {
            cpu.request_irq();
        }
        let cycles = cpu.step(&mut bus);
        assert_eq!(cycles, 7);
        assert_eq!(bus.events.len(), 7, "{:?}", bus.events);
        // Two throwaway reads, three stack writes, two vector reads.
        assert_eq!(bus.events[0], Ev::DummyRead(CODE));
        assert_eq!(bus.events[1], Ev::DummyRead(CODE));
        assert!(matches!(bus.events[2], Ev::Write(0x01FD, _)));
        assert!(matches!(bus.events[3], Ev::Write(0x01FC, _)));
        assert!(matches!(bus.events[4], Ev::Write(0x01FB, _)));
    }
    // JAM, then the stuck CPU's idle steps.
    let mut bus = TraceBus::new(0x02);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    let c1 = cpu.step(&mut bus);
    assert!(cpu.halted);
    let c2 = cpu.step(&mut bus);
    assert_eq!(bus.events.len() as u32, c1 + c2);
}

#[test]
fn read_modify_write_writes_the_old_value_back_before_the_new_one() {
    // INC $1234 with $1234 = 0x41: opcode, lo, hi, read, dummy write 0x41,
    // write 0x42.
    let mut bus = TraceBus::new(0x00);
    bus.mem[CODE as usize..CODE as usize + 3].copy_from_slice(&[0xEE, 0x34, 0x12]);
    bus.mem[0x1234] = 0x41;
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.step(&mut bus);
    assert_eq!(
        bus.events,
        vec![
            Ev::Read(CODE),
            Ev::Read(CODE + 1),
            Ev::Read(CODE + 2),
            Ev::Read(0x1234),
            Ev::DummyWrite(0x1234, 0x41),
            Ev::Write(0x1234, 0x42),
        ]
    );
}

#[test]
fn indexed_accesses_read_the_uncorrected_address_first() {
    // LDA $12F0,X with X = $20: crosses into $1310; the wasted read is at
    // $1210 (high byte not yet corrected).
    let mut bus = TraceBus::new(0x00);
    bus.mem[CODE as usize..CODE as usize + 3].copy_from_slice(&[0xBD, 0xF0, 0x12]);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.x = 0x20;
    let cycles = cpu.step(&mut bus);
    assert_eq!(cycles, 5);
    assert_eq!(&bus.events[3..], &[Ev::DummyRead(0x1210), Ev::Read(0x1310)]);

    // Without a cross a read pays no extra cycle ...
    let mut bus = TraceBus::new(0x00);
    bus.mem[CODE as usize..CODE as usize + 3].copy_from_slice(&[0xBD, 0x10, 0x12]);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.x = 0x20;
    assert_eq!(cpu.step(&mut bus), 4);
    assert_eq!(bus.events[3], Ev::Read(0x1230));

    // ... but STA abs,X always does, crossing or not.
    let mut bus = TraceBus::new(0x00);
    bus.mem[CODE as usize..CODE as usize + 3].copy_from_slice(&[0x9D, 0x10, 0x12]);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.x = 0x20;
    cpu.a = 0x77;
    assert_eq!(cpu.step(&mut bus), 5);
    assert_eq!(
        &bus.events[3..],
        &[Ev::DummyRead(0x1230), Ev::Write(0x1230, 0x77)]
    );
}

#[test]
fn zero_page_indexing_reads_the_unindexed_byte() {
    // LDA $F0,X with X = $20 wraps to $10; the wasted read is at $F0.
    let mut bus = TraceBus::new(0x00);
    bus.mem[CODE as usize..CODE as usize + 2].copy_from_slice(&[0xB5, 0xF0]);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.x = 0x20;
    cpu.step(&mut bus);
    assert_eq!(&bus.events[2..], &[Ev::DummyRead(0x00F0), Ev::Read(0x0010)]);
}

#[test]
fn high_byte_stores_mask_the_base_and_corrupt_the_write_address_on_carry() {
    for opcode in [0x9c, 0x9e, 0x9f, 0x93, 0x9b] {
        for (base, index, source, target, value) in [
            (0x3fceu16, 0xffu8, 0xbfu8, 0x00cdu16, 0x00u8),
            (0x3fce, 0xff, 0xff, 0x40cd, 0x40),
            (0x12f0, 0x20, 0x01, 0x0110, 0x01),
            (0x12e0, 0x1f, 0xbf, 0x12ff, 0x13),
            (0xfff0, 0x20, 0xff, 0x0010, 0x00),
        ] {
            let mut bus = TraceBus::new(0xaa);
            bus.mem[CODE as usize] = opcode;
            if opcode == 0x93 {
                bus.mem[CODE as usize + 1] = 0xff;
                bus.mem[0xff] = base as u8;
                bus.mem[0] = (base >> 8) as u8;
            } else {
                bus.mem[CODE as usize + 1..CODE as usize + 3].copy_from_slice(&base.to_le_bytes());
            }
            let mut cpu = Cpu::new();
            cpu.pc = CODE;
            cpu.a = 0xff;
            cpu.x = if opcode == 0x9c { index } else { source };
            cpu.y = if opcode == 0x9c { source } else { index };
            cpu.p = 0xeb;
            let cycles = cpu.step(&mut bus);
            let indexed = base.wrapping_add(u16::from(index));
            assert_eq!(
                &bus.events[bus.events.len() - 2..],
                &[
                    Ev::DummyRead((base & 0xff00) | (indexed & 0xff)),
                    Ev::Write(target, value),
                ],
                "opcode {opcode:02x}, base {base:04x}, index {index:02x}, source {source:02x}"
            );
            assert_eq!(cycles, if opcode == 0x93 { 6 } else { 5 });
            assert_eq!(bus.events.len() as u32, cycles);
            assert_eq!(bus.mem[usize::from(target)], value);
            assert_eq!(cpu.a, 0xff);
            assert_eq!(cpu.x, if opcode == 0x9c { index } else { source });
            assert_eq!(cpu.y, if opcode == 0x9c { source } else { index });
            assert_eq!(cpu.p, 0xeb);
            assert_eq!(cpu.sp, if opcode == 0x9b { source } else { 0xfd });
        }
    }
}

#[test]
fn stack_instructions_follow_the_real_cycle_order() {
    // JSR $1234: opcode, lo, stack read, push PCH, push PCL, hi.
    let mut bus = TraceBus::new(0x00);
    bus.mem[CODE as usize..CODE as usize + 3].copy_from_slice(&[0x20, 0x34, 0x12]);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.sp = 0xFD;
    assert_eq!(cpu.step(&mut bus), 6);
    assert_eq!(cpu.pc, 0x1234);
    assert_eq!(
        bus.events,
        vec![
            Ev::Read(CODE),
            Ev::Read(CODE + 1),
            Ev::DummyRead(0x01FD),
            Ev::Write(0x01FD, 0x02),
            Ev::Write(0x01FC, 0x02),
            Ev::Read(CODE + 2),
        ]
    );
    // The pushed return address is the last byte of the JSR ($0202) ...
    assert_eq!(bus.mem[0x01FC], 0x02);

    // ... which RTS pops and increments: opcode, 2 dummy reads, 2 pulls,
    // one read at the popped address.
    bus.mem[0x1234] = 0x60;
    bus.events.clear();
    assert_eq!(cpu.step(&mut bus), 6);
    assert_eq!(cpu.pc, 0x0203);
    assert_eq!(
        bus.events,
        vec![
            Ev::Read(0x1234),
            Ev::DummyRead(0x1235),
            Ev::DummyRead(0x01FB),
            Ev::Read(0x01FC),
            Ev::Read(0x01FD),
            Ev::DummyRead(0x0202),
        ]
    );
}

#[test]
fn a_taken_branch_wastes_a_read_and_a_page_cross_a_second_one() {
    // BNE +4 at $0200 -> $0206, no cross.
    let mut bus = TraceBus::new(0x00);
    bus.mem[CODE as usize..CODE as usize + 2].copy_from_slice(&[0xD0, 0x04]);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.p = 0x20; // Z clear: taken
    assert_eq!(cpu.step(&mut bus), 3);
    assert_eq!(bus.events[2], Ev::DummyRead(0x0202));

    // BNE -$10 at $0200 -> $01F2: crosses; second wasted read has the old
    // high byte with the new low byte.
    let mut bus = TraceBus::new(0x00);
    bus.mem[CODE as usize..CODE as usize + 2].copy_from_slice(&[0xD0, 0xF0]);
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.p = 0x20;
    assert_eq!(cpu.step(&mut bus), 4);
    assert_eq!(bus.events[2], Ev::DummyRead(0x0202));
    assert_eq!(bus.events[3], Ev::DummyRead(0x02F2));
}
