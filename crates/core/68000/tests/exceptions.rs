//! Exception behaviour SingleStepTests does not cover: interrupts, STOP, trace, RESET, the illegal-instruction
//! vectors and the double bus fault. Expected values are from the User's Manual (UM section 6, Table 8-14).

use mc68000::{BusFault, Cpu, FunctionCode, IackResponse, M68kBus, RunState, Size};

const SSP: u32 = 0x1000;
const START: u32 = 0x2000;

/// 16 MB of RAM; the vector table points every vector at its own handler address `0x8000 + 4 * vector`.
struct Bus {
    mem: Vec<u8>,
    iack: IackResponse,
    acked: Vec<u8>,
    resets: u32,
    fault_at: Option<u32>,
}

impl Bus {
    fn new(program: &[u16]) -> Self {
        let mut mem = vec![0u8; 1 << 24];
        mem[0..4].copy_from_slice(&SSP.to_be_bytes());
        mem[4..8].copy_from_slice(&START.to_be_bytes());
        for v in 2..256u32 {
            let a = (v * 4) as usize;
            mem[a..a + 4].copy_from_slice(&(0x8000 + v * 4).to_be_bytes());
        }
        for (i, w) in program.iter().enumerate() {
            let a = START as usize + 2 * i;
            mem[a..a + 2].copy_from_slice(&w.to_be_bytes());
        }
        Self {
            mem,
            iack: IackResponse::Autovector,
            acked: Vec::new(),
            resets: 0,
            fault_at: None,
        }
    }

    fn word(&self, a: u32) -> u16 {
        u16::from_be_bytes([self.mem[a as usize], self.mem[a as usize + 1]])
    }

    fn long(&self, a: u32) -> u32 {
        (self.word(a) as u32) << 16 | self.word(a + 2) as u32
    }
}

impl M68kBus for Bus {
    fn read(&mut self, addr: u32, size: Size, _fc: FunctionCode) -> Result<u32, BusFault> {
        if self.fault_at == Some(addr & !1) {
            return Err(BusFault);
        }
        Ok(match size {
            Size::Byte => self.mem[addr as usize] as u32,
            Size::Word => self.word(addr) as u32,
        })
    }

    fn write(
        &mut self,
        addr: u32,
        size: Size,
        value: u32,
        _fc: FunctionCode,
    ) -> Result<(), BusFault> {
        if self.fault_at == Some(addr & !1) {
            return Err(BusFault);
        }
        match size {
            Size::Byte => self.mem[addr as usize] = value as u8,
            Size::Word => self.mem[addr as usize..addr as usize + 2]
                .copy_from_slice(&(value as u16).to_be_bytes()),
        }
        Ok(())
    }

    fn iack(&mut self, level: u8) -> IackResponse {
        self.acked.push(level);
        self.iack
    }

    fn reset_devices(&mut self) {
        self.resets += 1;
    }
}

fn boot(program: &[u16]) -> (Cpu, Bus) {
    let mut bus = Bus::new(program);
    let mut cpu = Cpu::new();
    cpu.reset(&mut bus);
    (cpu, bus)
}

#[test]
fn interrupt_above_mask_is_taken_at_instruction_boundary_with_autovector() {
    // MOVE #$2300,SR; NOP
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2300, 0x4E71]);
    cpu.step(&mut bus);
    cpu.set_ipl(4);
    let clocks = cpu.step(&mut bus);
    assert_eq!(bus.acked, [4]);
    assert_eq!(cpu.pc, 0x8000 + 28 * 4, "level 4 autovector = vector 28");
    assert_eq!(
        cpu.sr() & 0x2700,
        0x2400,
        "supervisor, mask raised to the level"
    );
    assert_eq!(
        bus.long(SSP - 4),
        START + 4,
        "the next instruction is stacked"
    );
    assert_eq!(bus.word(SSP - 6), 0x2300, "the SR before");
    assert_eq!(clocks, 44, "UM Table 8-14");
}

#[test]
fn vectored_interrupt_uses_the_iack_vector() {
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2000, 0x4E71]);
    cpu.step(&mut bus);
    bus.iack = IackResponse::Vector(0x46);
    cpu.set_ipl(6);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 0x46 * 4);
}

#[test]
fn spurious_interrupt_takes_vector_24() {
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2000, 0x4E71]);
    cpu.step(&mut bus);
    bus.iack = IackResponse::Spurious;
    cpu.set_ipl(2);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 24 * 4);
}

#[test]
fn interrupt_at_or_below_mask_is_ignored_except_level_7() {
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2500, 0x4E71, 0x4E71, 0x4E71]);
    cpu.step(&mut bus);
    cpu.set_ipl(5);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, START + 6, "level 5 with mask 5 waits");
    assert!(bus.acked.is_empty());
    cpu.set_ipl(7);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 31 * 4, "level 7 is not maskable");
}

#[test]
fn level_7_interrupts_once_per_rising_edge() {
    // The level 7 handler is RTE: MOVE #$2700,SR keeps the mask at 7.
    let (mut cpu, mut bus) = boot(&[0x4E71, 0x4E71, 0x4E71]);
    let h = 0x8000 + 31 * 4;
    bus.mem[h as usize..h as usize + 2].copy_from_slice(&0x4E73u16.to_be_bytes());
    cpu.set_ipl(7);
    cpu.step(&mut bus);
    cpu.step(&mut bus); // RTE
    cpu.step(&mut bus); // NOP: the level is still 7, no new edge
    assert_eq!(bus.acked, [7]);
    cpu.set_ipl(0);
    cpu.step(&mut bus);
    cpu.set_ipl(7);
    cpu.step(&mut bus);
    assert_eq!(bus.acked, [7, 7]);
}

#[test]
fn stop_loads_sr_and_waits_until_an_interrupt() {
    // STOP #$2000
    let (mut cpu, mut bus) = boot(&[0x4E72, 0x2000, 0x4E71]);
    cpu.step(&mut bus);
    assert_eq!(cpu.run_state(), RunState::Stopped);
    assert_eq!(cpu.sr(), 0x2000);
    for _ in 0..3 {
        assert_eq!(cpu.step(&mut bus), 4);
        assert_eq!(cpu.run_state(), RunState::Stopped);
    }
    cpu.set_ipl(3);
    cpu.step(&mut bus);
    assert_eq!(cpu.run_state(), RunState::Running);
    assert_eq!(cpu.pc, 0x8000 + 27 * 4);
    assert_eq!(
        bus.long(SSP - 4),
        START + 4,
        "the instruction after STOP is stacked"
    );
}

#[test]
fn trace_takes_vector_9_after_each_instruction_when_t_is_set() {
    // MOVE #$A000,SR (T and S); NOP
    let (mut cpu, mut bus) = boot(&[0x46FC, 0xA000, 0x4E71]);
    let clocks = cpu.step(&mut bus);
    // T was clear when MOVE to SR started: no trace yet.
    assert_eq!(cpu.pc, START + 4);
    assert!(clocks < 34);
    let clocks = cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 9 * 4);
    assert_eq!(
        bus.long(SSP - 4),
        START + 6,
        "the instruction after the traced NOP"
    );
    assert_eq!(bus.word(SSP - 6), 0xA000);
    assert_eq!(cpu.sr() & 0x8000, 0, "T cleared in the handler");
    assert_eq!(
        clocks,
        4 + 34,
        "NOP plus the trace exception (UM Table 8-14)"
    );
}

#[test]
fn reset_instruction_resets_devices_and_continues() {
    let (mut cpu, mut bus) = boot(&[0x4E70, 0x4E71]);
    let clocks = cpu.step(&mut bus);
    assert_eq!(bus.resets, 1);
    assert_eq!(cpu.pc, START + 2);
    assert_eq!(clocks, 132, "UM Table 8-12");
}

#[test]
fn illegal_word_takes_vector_4_stacking_its_address() {
    let (mut cpu, mut bus) = boot(&[0x4AFC]);
    let clocks = cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 4 * 4);
    assert_eq!(bus.long(SSP - 4), START);
    assert_eq!(clocks, 34);
}

#[test]
fn line_a_takes_vector_10_and_line_f_vector_11() {
    let (mut cpu, mut bus) = boot(&[0xA123]);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 10 * 4);
    let (mut cpu, mut bus) = boot(&[0xF123]);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 11 * 4);
}

#[test]
fn privileged_instruction_in_user_mode_takes_vector_8() {
    // MOVE #$0000,SR (to user mode); RESET
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x0000, 0x4E70]);
    cpu.step(&mut bus);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 8 * 4);
    assert_eq!(bus.resets, 0);
    assert_eq!(bus.long(SSP - 4), START + 4);
}

#[test]
fn odd_word_read_raises_address_error_with_group0_frame() {
    // MOVE.W (A0),D0 with A0 = $1001
    let (mut cpu, mut bus) = boot(&[0x3010]);
    cpu.a[0] = 0x1001;
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 3 * 4, "vector 3");
    assert_eq!(cpu.ssp, SSP - 14);
    let frame = SSP - 14;
    assert_eq!(bus.long(frame + 2), 0x1001, "access address");
    let info = bus.word(frame);
    assert_eq!(info & 0x10, 0x10, "R/W = read");
    assert_eq!(info & 7, 5, "supervisor data");
    assert_eq!(bus.word(frame + 6), 0x3010, "instruction register");
}

#[test]
fn bus_error_takes_vector_2() {
    let (mut cpu, mut bus) = boot(&[0x3010]);
    cpu.a[0] = 0x4000;
    bus.fault_at = Some(0x4000);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8000 + 2 * 4);
    assert_eq!(bus.long(SSP - 14 + 2), 0x4000);
}

#[test]
fn double_bus_fault_halts() {
    let (mut cpu, mut bus) = boot(&[0x3010]);
    cpu.a[0] = 0x4000;
    bus.fault_at = Some(0x4000);
    cpu.ssp = 0x4000 + 14; // the frame's first write faults as well
    cpu.step(&mut bus);
    assert_eq!(cpu.run_state(), RunState::Halted);
    assert_eq!(cpu.step(&mut bus), 4);
}

#[test]
fn a_taken_exception_is_reported_once() {
    let (mut cpu, mut bus) = boot(&[0x4AFC]);
    assert_eq!(cpu.take_exception(), None);
    cpu.step(&mut bus);
    let e = cpu.take_exception().expect("recorded");
    assert_eq!((e.vector, e.pc, e.sr), (4, START, 0x2700));
    assert_eq!(e.fault, None);
    assert_eq!(cpu.take_exception(), None);
}

#[test]
fn an_address_error_reports_the_fault() {
    let (mut cpu, mut bus) = boot(&[0x3010]);
    cpu.a[0] = 0x1001;
    cpu.step(&mut bus);
    let e = cpu.take_exception().expect("recorded");
    assert_eq!(e.vector, 3);
    let f = e.fault.expect("fault details");
    assert_eq!(
        (f.addr, f.write, f.fc),
        (0x1001, false, FunctionCode::SUPERVISOR_DATA)
    );
}

#[test]
fn jumps_near_the_top_of_the_address_space_wrap() {
    // JSR $0100.W at PC $FFFFFFFC: the return address wraps past 2^32.
    let (mut cpu, mut bus) = boot(&[0x4E71]);
    cpu.pc = 0xFFFF_FFFC;
    cpu.set_prefetch([0x4EB8, 0x0100]);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x100);
    assert_eq!(
        bus.long(SSP - 4),
        0x0000_0000,
        "return address $FFFFFFFC + 4 wraps to 0"
    );
    // JSR abs.L at PC $FFFFFFFA.
    let (mut cpu, mut bus) = boot(&[0x4E71]);
    cpu.pc = 0xFFFF_FFFA;
    cpu.set_prefetch([0x4EB9, 0x0000]);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc & 0xFF_FFFF, (bus.word(0xFF_FFFE) as u32) & 0xFF_FFFE);
}

#[test]
fn a_halt_records_the_fault_that_caused_it() {
    let (mut cpu, mut bus) = boot(&[0x3010]);
    assert_eq!(cpu.halt_reason(), None);
    cpu.a[0] = 0x4000;
    bus.fault_at = Some(0x4000);
    cpu.ssp = 0x4000 + 14;
    cpu.step(&mut bus);
    assert_eq!(cpu.run_state(), RunState::Halted);
    let f = cpu.halt_reason().expect("why it halted");
    assert_eq!(f.addr & 0xFF_FFFE, 0x4000);
}

#[test]
fn an_odd_reset_pc_halts_with_the_fault_recorded() {
    let mut bus = Bus::new(&[]);
    bus.mem[4..8].copy_from_slice(&0x0000_2001u32.to_be_bytes());
    let mut cpu = Cpu::new();
    cpu.reset(&mut bus);
    assert_eq!(cpu.run_state(), RunState::Halted);
    assert_eq!(cpu.halt_reason().map(|f| f.addr), Some(0x2001));
}
