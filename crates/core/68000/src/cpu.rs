//! Registers, the prefetch queue, bus cycles and exception processing.

use crate::bus::{FunctionCode, M68kBus, Size};
use crate::opcodes::{Op, decode};
use serde::{Deserialize, Serialize};

pub(crate) const SR_T: u16 = 0x8000;
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
    /// A group 1 or 2 exception the instruction raised (privilege violation, TRAP, CHK, ...): its vector.
    Exception(u8),
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
    pub(crate) prefetch: [u16; 2],
    pub(crate) run: RunState,
    /// IPL2-IPL0, sampled at instruction boundaries.
    pub(crate) ipl: u8,
    /// A level-7 request already taken (level 7 interrupts on each rising edge, UM 6.3.2).
    #[serde(default)]
    pub(crate) nmi_seen: bool,
    /// The opcode of the instruction being executed (the IR stacked by a group 0 frame).
    #[serde(skip)]
    pub(crate) ir: u16,
    /// Address of the instruction being executed.
    #[serde(skip)]
    pub(crate) instruction_pc: u32,
    /// Processing an instruction (or a group 2 exception) rather than a group 0/1 exception: the I/N bit.
    #[serde(skip)]
    pub(crate) in_instruction: bool,
    /// Inside group 0 processing: another bus or address error halts the CPU.
    #[serde(skip)]
    pub(crate) in_group0: bool,
    /// The last exception taken (`Cpu::take_exception`).
    #[serde(skip)]
    pub(crate) last_exception: Option<crate::exception::ExceptionRecord>,
    /// The PC a raised group 1/2 exception stacks, and its internal clocks before stacking (`Cpu::raise`).
    #[serde(skip)]
    pub(crate) exception_pc: u32,
    #[serde(skip)]
    pub(crate) exception_idle: u32,
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
            nmi_seen: false,
            ir: 0,
            instruction_pc: 0,
            in_instruction: false,
            in_group0: false,
            last_exception: None,
            exception_pc: 0,
            exception_idle: 0,
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
        bus.read(addr & ADDR_MASK, size, fc)
            .map_err(|_| Fault::Bus {
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
        bus.write(addr & ADDR_MASK, size, value, fc)
            .map_err(|_| Fault::Bus {
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
        match self.run {
            RunState::Halted => self.idle(4),
            RunState::Stopped => match self.interrupt_due() {
                Some(level) => self.interrupt(bus, level),
                None => self.idle(4),
            },
            RunState::Running => match self.interrupt_due() {
                Some(level) => self.interrupt(bus, level),
                None => {
                    let tracing = self.sr & SR_T != 0;
                    self.ir = self.prefetch[0];
                    self.instruction_pc = self.pc;
                    self.in_instruction = true;
                    match self.execute(bus, self.ir) {
                        Ok(()) if tracing => {
                            self.run = RunState::Running;
                            self.trace(bus);
                        }
                        Ok(()) => {}
                        Err(fault) => self.group0(bus, fault),
                    }
                }
            },
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
            Op::Bcc => self.op_bcc(bus, opcode),
            Op::Dbcc => self.op_dbcc(bus, opcode),
            Op::Scc => self.op_scc(bus, opcode),
            Op::Jmp => self.op_jmp(bus, opcode),
            Op::Jsr => self.op_jsr(bus, opcode),
            Op::Rts => self.op_rts(bus),
            Op::Rtr => self.op_rtr(bus),
            Op::Rte => self.op_rte(bus),
            Op::Link => self.op_link(bus, opcode),
            Op::Unlk => self.op_unlk(bus, opcode),
            Op::LogicSr(logic, to_sr) => self.op_logic_sr(bus, logic, to_sr),
            Op::MoveToSr(to_sr) => self.op_move_to_sr(bus, opcode, to_sr),
            Op::MoveFromSr => self.op_move_from_sr(bus, opcode),
            Op::MoveUsp => self.op_move_usp(bus, opcode),
            Op::Reset => self.op_reset(bus),
            Op::MovemToMem(sz) => self.op_movem_to_mem(bus, opcode, sz),
            Op::MovemToReg(sz) => self.op_movem_to_reg(bus, opcode, sz),
            Op::Movep => self.op_movep(bus, opcode),
            Op::Bit(is_static) => self.op_bit(bus, opcode, is_static),
            Op::Tas => self.op_tas(bus, opcode),
            Op::Mul(signed) => self.op_mul(bus, opcode, signed),
            Op::Div(signed) => self.op_div(bus, opcode, signed),
            Op::Bcd(add) => self.op_bcd(bus, opcode, add),
            Op::Nbcd => self.op_nbcd(bus, opcode),
            Op::Chk => self.op_chk(bus, opcode),
            Op::Trap => self.op_trap(opcode),
            Op::Trapv => self.op_trapv(bus),
            Op::Stop => self.op_stop(bus),
            Op::Illegal => {
                let vector = match opcode >> 12 {
                    0xA => crate::exception::VECTOR_LINE_A,
                    0xF => crate::exception::VECTOR_LINE_F,
                    _ => crate::exception::VECTOR_ILLEGAL,
                };
                Err(self.raise(vector, self.pc, 4))
            }
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
