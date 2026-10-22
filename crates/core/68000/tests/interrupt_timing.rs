//! Exception processing against the MC68000 User's Manual, section 8.12, Table 8-14 ("Exception Processing Execution
//! Times", `n(r/w)`: clock periods, read cycles, write cycles; https://www.nxp.com/docs/en/reference-manual/MC68000UM.pdf,
//! SHA-256 89b690b1923f8a3cff508567090bcab0bcd07511c2deff8ffa393a08efda18e1). The table's numbers include the stacking,
//! the vector fetch and the fetch of the handler's first instruction; the interrupt acknowledge cycle (4 clocks) counts as
//! one of the interrupt's 5 reads. The cases the SingleStepTests do cover (CHK, divide by zero, address errors on odd
//! pointers) are not repeated here. Also pinned: where the interrupt level is sampled.

use mc68000::{BusFault, Cpu, FunctionCode, IackResponse, M68kBus, RunState, Size};

const SSP: u32 = 0x1000;
const START: u32 = 0x2000;

/// 16 MB of RAM that counts its read and write cycles and records the order of everything the CPU does.
struct Bus {
    mem: Vec<u8>,
    reads: u32,
    writes: u32,
    iacks: u32,
    log: Vec<String>,
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
            reads: 0,
            writes: 0,
            iacks: 0,
            log: Vec::new(),
        }
    }

    fn counts(&mut self) -> (u32, u32) {
        let c = (self.reads + self.iacks, self.writes);
        self.reads = 0;
        self.writes = 0;
        self.iacks = 0;
        self.log.clear();
        c
    }
}

impl M68kBus for Bus {
    fn read(&mut self, addr: u32, size: Size, _fc: FunctionCode) -> Result<u32, BusFault> {
        self.reads += 1;
        self.log.push(format!("R{addr:X}"));
        let a = addr as usize;
        Ok(match size {
            Size::Byte => self.mem[a] as u32,
            Size::Word => u16::from_be_bytes([self.mem[a], self.mem[a + 1]]) as u32,
        })
    }

    fn write(
        &mut self,
        addr: u32,
        size: Size,
        value: u32,
        _fc: FunctionCode,
    ) -> Result<(), BusFault> {
        self.writes += 1;
        self.log.push(format!("W{addr:X}"));
        let a = addr as usize;
        match size {
            Size::Byte => self.mem[a] = value as u8,
            Size::Word => self.mem[a..a + 2].copy_from_slice(&(value as u16).to_be_bytes()),
        }
        Ok(())
    }

    fn iack(&mut self, _level: u8) -> IackResponse {
        self.iacks += 1;
        self.log.push("I".to_string());
        IackResponse::Autovector
    }

    fn sync(&mut self, clocks: u32) {
        match self.log.last_mut() {
            Some(last) if last.starts_with('n') => {
                let n: u32 = last[1..].parse().unwrap_or(0);
                *last = format!("n{}", n + clocks);
            }
            _ => self.log.push(format!("n{clocks}")),
        }
    }
}

fn boot(program: &[u16]) -> (Cpu, Bus) {
    let mut bus = Bus::new(program);
    let mut cpu = Cpu::new();
    cpu.reset(&mut bus);
    bus.counts();
    (cpu, bus)
}

#[test]
fn trap_takes_34_clocks_with_4_reads_and_3_writes() {
    let (mut cpu, mut bus) = boot(&[0x4E40]);
    let clocks = cpu.step(&mut bus);
    assert_eq!((clocks, bus.counts()), (34, (4, 3)), "Table 8-14: TRAP");
}

#[test]
fn an_illegal_instruction_takes_34_clocks_with_4_reads_and_3_writes() {
    let (mut cpu, mut bus) = boot(&[0x4AFC]);
    let clocks = cpu.step(&mut bus);
    assert_eq!((clocks, bus.counts()), (34, (4, 3)), "Table 8-14: illegal");
}

#[test]
fn a_privilege_violation_takes_34_clocks_with_4_reads_and_3_writes() {
    // MOVE #$0000,SR drops to user mode; STOP #$2700 is then privileged.
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x0000, 0x4E72, 0x2700]);
    cpu.step(&mut bus);
    bus.counts();
    let clocks = cpu.step(&mut bus);
    assert_eq!(
        (clocks, bus.counts()),
        (34, (4, 3)),
        "Table 8-14: privilege violation"
    );
}

#[test]
fn trapv_takes_34_clocks_with_5_reads_and_3_writes() {
    // MOVE #$2702,SR (V set); TRAPV
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2702, 0x4E76]);
    cpu.step(&mut bus);
    bus.counts();
    let clocks = cpu.step(&mut bus);
    assert_eq!((clocks, bus.counts()), (34, (5, 3)), "Table 8-14: TRAPV");
}

#[test]
fn trace_takes_34_clocks_more_than_the_traced_instruction() {
    // MOVE #$A700,SR (T set); NOP; NOP
    let (mut cpu, mut bus) = boot(&[0x46FC, 0xA700, 0x4E71, 0x4E71]);
    cpu.step(&mut bus);
    bus.counts();
    let clocks = cpu.step(&mut bus);
    assert_eq!(clocks, 4 + 34, "NOP, then Table 8-14: trace");
    assert_eq!(
        bus.counts(),
        (1 + 4, 3),
        "the NOP's fetch, then 4 reads, 3 writes"
    );
}

#[test]
fn an_autovector_interrupt_takes_44_clocks_and_its_acknowledge_is_the_fifth_read() {
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2300, 0x4E71]);
    cpu.step(&mut bus);
    bus.counts();
    cpu.set_ipl(4);
    let clocks = cpu.step(&mut bus);
    let (reads, writes) = bus.counts();
    assert_eq!((clocks, reads, writes), (44, 5, 3), "Table 8-14: interrupt");
}

#[test]
fn the_interrupt_sequence_is_6_internal_clocks_the_acknowledge_then_the_stacking() {
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2300, 0x4E71]);
    cpu.step(&mut bus);
    bus.counts();
    cpu.set_ipl(4);
    cpu.step(&mut bus);
    // 6 internal clocks, then the acknowledge (its 4 clocks and 4 internal ones follow as 8), the stacked PC low word,
    // the status word, the PC high word; the two vector words; the first word of the handler, 2 internal clocks and
    // the second word. Table 8-14 gives the total and the cycle counts, not this order; it is the order of the
    // 68000's documented microcode sequence (np before and after the internal pair).
    let expected = [
        "n6", "I", "n8", "WFFE", "WFFA", "WFFC", "R70", "R72", "R8070", "n2", "R8072",
    ];
    assert_eq!(bus.log, expected);
}

#[test]
fn the_level_is_sampled_once_before_the_next_instruction_not_during_one() {
    // Raising the request between two steps is seen by the second; a request that is withdrawn again before the
    // step that would have taken it is never taken.
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2300, 0x4E71, 0x4E71]);
    cpu.step(&mut bus);
    cpu.set_ipl(4);
    cpu.set_ipl(0);
    let clocks = cpu.step(&mut bus);
    assert_eq!(
        clocks, 4,
        "the NOP ran: nothing was pending at the sampling point"
    );
    cpu.set_ipl(5);
    let clocks = cpu.step(&mut bus);
    assert_eq!(
        clocks, 44,
        "the request pending at the sampling point is taken"
    );
}

#[test]
fn stop_costs_4_clocks_a_step_while_waiting_and_44_to_wake_on_an_interrupt() {
    // MOVE #$2000,SR; STOP #$2000
    let (mut cpu, mut bus) = boot(&[0x46FC, 0x2000, 0x4E72, 0x2000]);
    cpu.step(&mut bus);
    let stop = cpu.step(&mut bus);
    assert_eq!(stop, 4, "Table 8-12: STOP");
    assert_eq!(cpu.run_state(), RunState::Stopped);
    assert_eq!(
        cpu.step(&mut bus),
        4,
        "waiting: the clock runs, no bus cycle"
    );
    cpu.set_ipl(3);
    assert_eq!(cpu.step(&mut bus), 44, "the interrupt that ends the wait");
}

#[test]
fn the_reset_instruction_costs_132_clocks() {
    let (mut cpu, mut bus) = boot(&[0x4E70]);
    assert_eq!(cpu.step(&mut bus), 132, "Table 8-12: RESET");
}
