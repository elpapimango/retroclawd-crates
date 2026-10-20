//! Registers, the prefetch queue, bus cycles and exception processing.

use crate::bus::{FunctionCode, M68kBus, Size};
use crate::opcodes::{Op, decode};
use serde::{Deserialize, Serialize};

pub(crate) const SR_S: u16 = 0x2000;
pub(crate) const SR_MASK: u16 = 0x0700;
/// The bits of SR that exist on the 68000: T, S, I2-I0, X, N, Z, V, C (UM 2.3).
pub(crate) const SR_IMPLEMENTED: u16 = 0xA71F;

/// The address bus is 24 bits wide (A23-A1 plus the data strobes).
pub(crate) const ADDR_MASK: u32 = 0x00FF_FFFF;

/// Why the CPU is or is not executing instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    Running,
    /// After STOP, until an interrupt above the mask, a trace or a reset.
    Stopped,
    /// After a double bus fault; only a reset restarts it (UM 6.3.9.1).
    Halted,
}

/// A group 0 exception raised by a bus cycle, which ends the instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    Bus {
        addr: u32,
        write: bool,
        fc: FunctionCode,
        instruction: bool,
    },
    Address {
        addr: u32,
        write: bool,
        fc: FunctionCode,
        instruction: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cpu {
    pub d: [u32; 8],
    /// A0-A6; A7 is `usp` or `ssp` depending on S.
    pub a: [u32; 7],
    pub usp: u32,
    pub ssp: u32,
    /// Address of the instruction whose opcode is `prefetch[0]`.
    pub pc: u32,
    sr: u16,
    /// The two-word prefetch queue: the words at `pc` and `pc + 2`.
    prefetch: [u16; 2],
    run: RunState,
    /// IPL2-IPL0, sampled at instruction boundaries.
    ipl: u8,
    /// Clocks spent by the current `step` or `reset`.
    #[serde(skip)]
    clocks: u32,
    /// Internal clocks not yet passed to `M68kBus::sync`.
    #[serde(skip)]
    unsynced: u32,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            d: [0; 8],
            a: [0; 7],
            usp: 0,
            ssp: 0,
            pc: 0,
            sr: SR_S | SR_MASK,
            prefetch: [0; 2],
            run: RunState::Running,
            ipl: 0,
            clocks: 0,
            unsynced: 0,
        }
    }

    pub fn sr(&self) -> u16 {
        self.sr
    }

    /// Sets SR; unimplemented bits read as zero, and A7 follows S.
    pub fn set_sr(&mut self, sr: u16) {
        self.sr = sr & SR_IMPLEMENTED;
    }

    pub(crate) fn supervisor(&self) -> bool {
        self.sr & SR_S != 0
    }

    /// The active stack pointer.
    pub fn a7(&self) -> u32 {
        if self.supervisor() {
            self.ssp
        } else {
            self.usp
        }
    }

    pub(crate) fn set_a7(&mut self, v: u32) {
        if self.supervisor() {
            self.ssp = v
        } else {
            self.usp = v
        }
    }

    /// Address register `n` (0-7, 7 = the active stack pointer).
    pub(crate) fn an(&self, n: usize) -> u32 {
        if n == 7 { self.a7() } else { self.a[n] }
    }

    pub(crate) fn set_an(&mut self, n: usize, v: u32) {
        if n == 7 {
            self.set_a7(v)
        } else {
            self.a[n] = v
        }
    }

    pub fn prefetch(&self) -> [u16; 2] {
        self.prefetch
    }

    pub fn set_prefetch(&mut self, q: [u16; 2]) {
        self.prefetch = q;
    }

    pub fn run_state(&self) -> RunState {
        self.run
    }

    /// Drives IPL2-IPL0 (0 = no request, 7 = non-maskable).
    pub fn set_ipl(&mut self, level: u8) {
        self.ipl = level & 7;
    }

    pub(crate) fn data_fc(&self) -> FunctionCode {
        if self.supervisor() {
            FunctionCode::SUPERVISOR_DATA
        } else {
            FunctionCode::USER_DATA
        }
    }

    pub(crate) fn program_fc(&self) -> FunctionCode {
        if self.supervisor() {
            FunctionCode::SUPERVISOR_PROGRAM
        } else {
            FunctionCode::USER_PROGRAM
        }
    }

    /// `n` internal clocks with no bus cycle.
    pub(crate) fn idle(&mut self, n: u32) {
        self.clocks += n;
        self.unsynced += n;
    }

    pub(crate) fn flush_sync<B: M68kBus>(&mut self, bus: &mut B) {
        if self.unsynced != 0 {
            bus.sync(self.unsynced);
            self.unsynced = 0;
        }
    }

    /// One read cycle (4 clocks). A word at an odd address is an address error, raised before the cycle.
    pub(crate) fn bus_read<B: M68kBus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        fc: FunctionCode,
        instruction: bool,
    ) -> Result<u32, Fault> {
        let addr = addr & ADDR_MASK;
        if size == Size::Word && addr & 1 != 0 {
            return Err(Fault::Address {
                addr,
                write: false,
                fc,
                instruction,
            });
        }
        self.flush_sync(bus);
        self.clocks += 4;
        bus.read(addr, size, fc).map_err(|_| Fault::Bus {
            addr,
            write: false,
            fc,
            instruction,
        })
    }

    /// One write cycle (4 clocks).
    pub(crate) fn bus_write<B: M68kBus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        value: u32,
        fc: FunctionCode,
    ) -> Result<(), Fault> {
        let addr = addr & ADDR_MASK;
        if size == Size::Word && addr & 1 != 0 {
            return Err(Fault::Address {
                addr,
                write: true,
                fc,
                instruction: false,
            });
        }
        self.flush_sync(bus);
        self.clocks += 4;
        bus.write(addr, size, value, fc).map_err(|_| Fault::Bus {
            addr,
            write: true,
            fc,
            instruction: false,
        })
    }

    /// The reset exception (UM 6.3.1): S set, T cleared, mask 7; the SSP from vector long 0 and the PC from long 1,
    /// both in supervisor program space; nothing is stacked. Then the prefetch queue is filled from the new PC.
    /// UM Table 8-14: 40 clocks, 6 reads. UM 6.3.1 does not specify the condition codes; they are kept.
    pub fn reset<B: M68kBus>(&mut self, bus: &mut B) -> u32 {
        self.clocks = 0;
        self.unsynced = 0;
        self.run = RunState::Running;
        self.sr = (self.sr & 0x001F) | SR_S | SR_MASK;
        if self.reset_reads(bus).is_err() {
            // A fault while reading the reset vectors cannot be processed: the CPU halts (UM 6.3.9.1).
            self.run = RunState::Halted;
        }
        self.flush_sync(bus);
        self.clocks
    }

    fn reset_reads<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        let fc = FunctionCode::SUPERVISOR_PROGRAM;
        self.idle(16);
        let hi = self.bus_read(bus, 0, Size::Word, fc, true)?;
        let lo = self.bus_read(bus, 2, Size::Word, fc, true)?;
        self.ssp = hi << 16 | lo;
        let hi = self.bus_read(bus, 4, Size::Word, fc, true)?;
        let lo = self.bus_read(bus, 6, Size::Word, fc, true)?;
        self.pc = hi << 16 | lo;
        self.fill_prefetch(bus)
    }

    /// Executes one instruction (or, when stopped or halted, lets 4 clocks pass). Returns the clocks it took.
    pub fn step<B: M68kBus>(&mut self, bus: &mut B) -> u32 {
        self.clocks = 0;
        if self.run != RunState::Running {
            self.idle(4);
        } else {
            let opcode = self.prefetch[0];
            if let Err(_fault) = self.execute(bus, opcode) {
                // Group 0 exception processing arrives with the exception work.
            }
        }
        self.flush_sync(bus);
        self.clocks
    }

    fn execute<B: M68kBus>(&mut self, bus: &mut B, opcode: u16) -> Result<(), Fault> {
        match decode(opcode) {
            Op::Nop => self.prefetch_next(bus),
            Op::Move(sz) => self.op_move(bus, opcode, sz),
            Op::Movea(sz) => self.op_movea(bus, opcode, sz),
            Op::Moveq => self.op_moveq(bus, opcode),
            Op::Lea => self.op_lea(bus, opcode),
            Op::Pea => self.op_pea(bus, opcode),
            Op::Clr(sz) => self.op_clr(bus, opcode, sz),
            Op::Tst(sz) => self.op_tst(bus, opcode, sz),
            Op::Exg => self.op_exg(bus, opcode),
            Op::Swap => self.op_swap(bus, opcode),
            Op::Ext(sz) => self.op_ext(bus, opcode, sz),
            Op::Arith(alu, sz) => self.op_arith(bus, opcode, alu, sz),
            Op::ArithMem(alu, sz) => self.op_arith_mem(bus, opcode, alu, sz),
            Op::ArithA(alu, sz) => self.op_arith_a(bus, opcode, alu, sz),
            Op::ArithX(alu, sz) => self.op_arith_x(bus, opcode, alu, sz),
            Op::ArithI(alu, sz) => self.op_arith_i(bus, opcode, alu, sz),
            Op::ArithQ(alu, sz) => self.op_arith_q(bus, opcode, alu, sz),
            Op::Cmp(sz) => self.op_cmp(bus, opcode, sz),
            Op::Cmpa(sz) => self.op_cmpa(bus, opcode, sz),
            Op::Cmpm(sz) => self.op_cmpm(bus, opcode, sz),
            Op::Cmpi(sz) => self.op_cmpi(bus, opcode, sz),
            Op::Neg(sz) => self.op_neg(bus, opcode, sz, false),
            Op::Negx(sz) => self.op_neg(bus, opcode, sz, true),
            Op::Logic(logic, sz) => self.op_logic(bus, opcode, logic, sz),
            Op::LogicMem(logic, sz) => self.op_logic_mem(bus, opcode, logic, sz),
            Op::LogicI(logic, sz) => self.op_logic_i(bus, opcode, logic, sz),
            Op::Not(sz) => self.op_not(bus, opcode, sz),
            Op::Shift(kind, sz) => self.op_shift(bus, opcode, kind, sz),
            Op::ShiftMem(kind) => self.op_shift_mem(bus, opcode, kind),
            // Exception processing arrives with the exception work.
            Op::Illegal => self.prefetch_next(bus),
        }
    }

    /// The fetch that ends an instruction: the queue moves up one word and the word after it is read.
    pub(crate) fn prefetch_next<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        self.pc = self.pc.wrapping_add(2);
        self.prefetch[0] = self.prefetch[1];
        let fc = self.program_fc();
        self.prefetch[1] =
            self.bus_read(bus, self.pc.wrapping_add(2), Size::Word, fc, true)? as u16;
        Ok(())
    }

    /// Takes the extension word at the head of the queue and refills it.
    pub(crate) fn extension<B: M68kBus>(&mut self, bus: &mut B) -> Result<u16, Fault> {
        let word = self.prefetch[1];
        self.pc = self.pc.wrapping_add(2);
        let fc = self.program_fc();
        self.prefetch[1] =
            self.bus_read(bus, self.pc.wrapping_add(2), Size::Word, fc, true)? as u16;
        Ok(word)
    }

    /// Reloads both prefetch words from `pc` (after a jump or an exception).
    pub(crate) fn fill_prefetch<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        let fc = self.program_fc();
        self.prefetch[0] = self.bus_read(bus, self.pc, Size::Word, fc, true)? as u16;
        self.prefetch[1] =
            self.bus_read(bus, self.pc.wrapping_add(2), Size::Word, fc, true)? as u16;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BusFault, IackResponse};

    /// 16 MB of flat RAM recording every bus cycle.
    struct Ram {
        mem: Vec<u8>,
        cycles: Vec<(bool, u32, Size, FunctionCode)>,
    }

    impl M68kBus for Ram {
        fn read(&mut self, addr: u32, size: Size, fc: FunctionCode) -> Result<u32, BusFault> {
            self.cycles.push((false, addr, size, fc));
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
            fc: FunctionCode,
        ) -> Result<(), BusFault> {
            self.cycles.push((true, addr, size, fc));
            let a = addr as usize;
            match size {
                Size::Byte => self.mem[a] = value as u8,
                Size::Word => self.mem[a..a + 2].copy_from_slice(&(value as u16).to_be_bytes()),
            }
            Ok(())
        }
        fn iack(&mut self, _level: u8) -> IackResponse {
            IackResponse::Autovector
        }
    }

    #[test]
    fn reset_loads_ssp_and_pc_from_vectors_in_supervisor_mode() {
        let mut mem = vec![0u8; 1 << 24];
        mem[0..4].copy_from_slice(&0x0000_0400u32.to_be_bytes());
        mem[4..8].copy_from_slice(&0x00FC_0030u32.to_be_bytes());
        let mut bus = Ram {
            mem,
            cycles: Vec::new(),
        };
        let mut cpu = Cpu::new();
        let clocks = cpu.reset(&mut bus);
        assert_eq!(cpu.a7(), 0x400);
        assert_eq!(cpu.pc, 0xFC0030);
        assert_eq!(cpu.sr() & 0xA700, 0x2700, "S set, T clear, mask 7");
        assert_eq!(clocks, 40, "UM Table 8-14: RESET 40(6/0)");
        let vector_reads: Vec<_> = bus.cycles.iter().take(4).collect();
        assert!(
            vector_reads
                .iter()
                .all(|c| !c.0 && c.2 == Size::Word && c.3 == FunctionCode::SUPERVISOR_PROGRAM)
        );
        assert_eq!(
            vector_reads.iter().map(|c| c.1).collect::<Vec<_>>(),
            [0, 2, 4, 6]
        );
    }
}
