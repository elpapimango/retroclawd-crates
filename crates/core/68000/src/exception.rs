//! Exception processing (UM section 6): group 0 (bus and address errors), group 1 (trace, interrupt, illegal,
//! privilege) and group 2 (TRAP, TRAPV, CHK, divide by zero), and the STOP instruction.
//!
//! The sequence is UM 6.2.5: copy SR, enter supervisor mode with T cleared, determine the vector, stack the PC
//! and SR (group 0 also the access information, Figure 6-7), fetch the vector and refill the prefetch queue.
//! What a group 0 frame holds and when its cycles happen are checked against SingleStepTests.

use crate::bus::{FunctionCode, IackResponse, M68kBus};
use crate::cpu::{Cpu, Fault, RunState, SR_MASK, SR_S, SR_T};
use crate::opcodes::Sz;

pub(crate) const VECTOR_BUS_ERROR: u8 = 2;
pub(crate) const VECTOR_ADDRESS_ERROR: u8 = 3;
pub(crate) const VECTOR_ILLEGAL: u8 = 4;
pub(crate) const VECTOR_TRACE: u8 = 9;
pub(crate) const VECTOR_LINE_A: u8 = 10;
pub(crate) const VECTOR_LINE_F: u8 = 11;
pub(crate) const VECTOR_SPURIOUS: u8 = 24;
pub(crate) const VECTOR_AUTOVECTOR_BASE: u8 = 24;

/// The last exception the CPU took (for diagnostics; `Cpu::take_exception`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExceptionRecord {
    pub vector: u8,
    /// The PC the frame stacked.
    pub pc: u32,
    /// The SR before the exception.
    pub sr: u16,
    /// For a bus or address error: the access that faulted.
    pub fault: Option<FaultInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultInfo {
    pub addr: u32,
    pub write: bool,
    pub fc: FunctionCode,
}

impl Cpu {
    /// The bus or address error that halted the CPU (a double fault, or a fault reading the reset vectors).
    pub fn halt_reason(&self) -> Option<FaultInfo> {
        self.halt_fault
    }

    /// Halts the CPU (UM 6.3.9.1), remembering the fault.
    pub(crate) fn halt(&mut self, fault: Fault) {
        self.run = RunState::Halted;
        self.halt_fault = match fault {
            Fault::Bus {
                addr, write, fc, ..
            }
            | Fault::Address {
                addr, write, fc, ..
            } => Some(FaultInfo { addr, write, fc }),
            Fault::Exception(_) => None,
        };
    }

    /// The exception taken since the last call, if any.
    pub fn take_exception(&mut self) -> Option<ExceptionRecord> {
        self.last_exception.take()
    }

    /// Enters supervisor mode with T cleared and returns the SR before.
    fn enter_exception(&mut self) -> u16 {
        let old = self.sr();
        self.set_sr((old | SR_S) & !SR_T);
        old
    }

    /// Stacks `pc` and `sr` (group 1 and 2 frame, UM Figure 6-5), then continues at `vector`'s handler.
    pub(crate) fn short_frame<B: M68kBus>(
        &mut self,
        bus: &mut B,
        vector: u8,
        pc: u32,
        sr: u16,
    ) -> Result<(), Fault> {
        self.last_exception = Some(ExceptionRecord {
            vector,
            pc,
            sr,
            fault: None,
        });
        // SingleStepTests order: PC low word, SR, PC high word.
        let sp = self.ssp.wrapping_sub(6);
        self.ssp = sp;
        self.write_mem(bus, sp.wrapping_add(4), Sz::W, pc & 0xFFFF)?;
        self.write_mem(bus, sp, Sz::W, sr as u32)?;
        self.write_mem(bus, sp.wrapping_add(2), Sz::W, pc >> 16)?;
        self.take_vector(bus, vector)
    }

    /// Reads `vector` and refills the queue from the handler's address.
    fn take_vector<B: M68kBus>(&mut self, bus: &mut B, vector: u8) -> Result<(), Fault> {
        let fc = FunctionCode::SUPERVISOR_DATA;
        let addr = vector as u32 * 4;
        let hi = self.bus_read(bus, addr, crate::bus::Size::Word, fc, false)?;
        let lo = self.bus_read(bus, addr + 2, crate::bus::Size::Word, fc, false)?;
        self.pc = hi << 16 | lo;
        let fc = self.program_fc();
        self.prefetch[0] = self.bus_read(bus, self.pc, crate::bus::Size::Word, fc, true)? as u16;
        self.idle(2);
        self.prefetch[1] = self.bus_read(
            bus,
            self.pc.wrapping_add(2),
            crate::bus::Size::Word,
            fc,
            true,
        )? as u16;
        Ok(())
    }

    /// Raises a group 1 or 2 exception from an instruction, stacking `pc`: illegal, line A/F and privilege
    /// violations stack the address of the instruction (UM 6.3.6, 6.3.7), the traps the next one (UM 6.3.5).
    /// `idle`: internal clocks before the stacking.
    pub(crate) fn raise(&mut self, vector: u8, pc: u32, idle: u32) -> Fault {
        self.exception_pc = pc;
        self.exception_idle = idle;
        Fault::Exception(vector)
    }

    /// Processes the exception `raise` prepared.
    pub(crate) fn instruction_exception<B: M68kBus>(&mut self, bus: &mut B, vector: u8) {
        let pc = self.exception_pc;
        let sr = self.enter_exception();
        self.idle(self.exception_idle);
        if let Err(fault) = self.short_frame(bus, vector, pc, sr) {
            self.group0(bus, fault);
        }
    }

    /// A bus or address error (UM 6.3.9.1, 6.3.10): the 7-word frame of Figure 6-7. A second group 0 fault while
    /// processing one halts the CPU.
    pub(crate) fn group0<B: M68kBus>(&mut self, bus: &mut B, fault: Fault) {
        if self.in_group0 {
            self.halt(fault);
            return;
        }
        let (vector, addr, write, fc, fetch) = match fault {
            Fault::Bus {
                addr,
                write,
                fc,
                instruction,
            } => (VECTOR_BUS_ERROR, addr, write, fc, instruction),
            Fault::Address {
                addr,
                write,
                fc,
                instruction,
            } => (VECTOR_ADDRESS_ERROR, addr, write, fc, instruction),
            Fault::Exception(v) => return self.instruction_exception(bus, v),
        };
        self.in_group0 = true;
        let sr = self.enter_exception();
        let info = (self.ir & 0xFFE0)
            | if write { 0 } else { 0x10 }
            | if fetch || !self.in_instruction {
                0x08
            } else {
                0
            }
            | fc.0 as u16;
        // SingleStepTests: the PC register as it stands (the last word the instruction consumed; for a fetch at
        // a jump target, the target - 4), 4 internal clocks, then PC low, SR, PC high, IR, access address low,
        // the information word, access address high. The I/N bit is set for a program fetch.
        let pc = if fetch { addr.wrapping_sub(4) } else { self.pc };
        self.last_exception = Some(ExceptionRecord {
            vector,
            pc,
            sr,
            fault: Some(FaultInfo { addr, write, fc }),
        });
        self.idle(4);
        let result = (|| {
            let sp = self.ssp.wrapping_sub(14);
            self.ssp = sp;
            self.write_mem(bus, sp.wrapping_add(12), Sz::W, pc & 0xFFFF)?;
            self.write_mem(bus, sp.wrapping_add(8), Sz::W, sr as u32)?;
            self.write_mem(bus, sp.wrapping_add(10), Sz::W, pc >> 16)?;
            self.write_mem(bus, sp.wrapping_add(6), Sz::W, self.ir as u32)?;
            self.write_mem(bus, sp.wrapping_add(4), Sz::W, addr & 0xFFFF)?;
            self.write_mem(bus, sp, Sz::W, info as u32)?;
            self.write_mem(bus, sp.wrapping_add(2), Sz::W, addr >> 16)?;
            self.take_vector(bus, vector)
        })();
        if let Err(fault) = result {
            self.halt(fault);
        }
        self.in_group0 = false;
    }

    /// Trace (UM 6.3.8): after an instruction that started with T set and completed.
    pub(crate) fn trace<B: M68kBus>(&mut self, bus: &mut B) {
        let sr = self.enter_exception();
        self.in_instruction = false;
        self.idle(4);
        let pc = self.pc;
        if let Err(fault) = self.short_frame(bus, VECTOR_TRACE, pc, sr) {
            self.group0(bus, fault);
        }
    }

    /// Whether an interrupt is to be taken now: the requested level is above the mask, or a new level 7.
    pub(crate) fn interrupt_due(&mut self) -> Option<u8> {
        let level = self.ipl;
        let mask = ((self.sr() & SR_MASK) >> 8) as u8;
        let nmi_edge = level == 7 && !self.nmi_seen;
        if level != 7 {
            self.nmi_seen = false;
        }
        if level > mask || nmi_edge {
            if level == 7 {
                self.nmi_seen = true;
            }
            Some(level)
        } else {
            None
        }
    }

    /// Interrupt exception processing (UM 6.3.2): the stacked PC is the next instruction; the mask becomes the
    /// level; the vector comes from the acknowledge cycle. UM Table 8-14: 44 clocks with a 4-clock acknowledge.
    pub(crate) fn interrupt<B: M68kBus>(&mut self, bus: &mut B, level: u8) {
        let sr = self.enter_exception();
        self.set_sr((self.sr() & !SR_MASK) | ((level as u16) << 8));
        self.run = RunState::Running;
        self.in_instruction = false;
        self.idle(6);
        self.flush_sync(bus);
        self.idle(4);
        let vector = match bus.iack(level) {
            IackResponse::Vector(v) => v,
            IackResponse::Autovector => VECTOR_AUTOVECTOR_BASE + level,
            IackResponse::Spurious => VECTOR_SPURIOUS,
        };
        self.idle(4);
        let pc = self.pc;
        if let Err(fault) = self.short_frame(bus, vector, pc, sr) {
            self.group0(bus, fault);
        }
    }

    /// STOP #imm (privileged): SR = imm, then wait for an interrupt (or a trace) (PRM 4: STOP).
    pub(crate) fn op_stop<B: M68kBus>(&mut self, bus: &mut B) -> Result<(), Fault> {
        self.require_supervisor()?;
        let imm = self.prefetch()[1];
        self.set_sr(imm);
        self.pc = self.pc.wrapping_add(4);
        self.run = RunState::Stopped;
        // UM Table 8-12: STOP is 4(0/0); the clocks that follow, while the CPU waits, are counted by `step`.
        self.idle(4);
        let _ = bus;
        Ok(())
    }
}
