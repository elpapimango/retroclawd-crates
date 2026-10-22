//! What the core needs from the machine around it.
//!
//! The MC68000 has a 16-bit data bus: every access is one byte or one word cycle (UM 5.1). A long operand is two
//! word cycles. A machine sees the cycles a real 68000 puts on the bus (count, size, function code, address) and
//! the instruction's exact total of clocks, and since Phase 3 the order of those cycles and the position of the
//! internal clocks between them equal SingleStepTests' transaction lists in every case the suite is exact for
//! (`tests/singlestep.rs` compares them), so a machine may rely on per-cycle timing. Each access carries the function code (UM 5.1.2, Table 5-1) so a
//! machine can tell user from supervisor and program from data, and answer the interrupt-acknowledge cycle.

use serde::{Deserialize, Serialize};

/// Width of one bus cycle. `Byte` drives one data strobe (UDS for an even address, LDS for an odd one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Byte,
    Word,
}

/// The FC2-FC0 lines of a bus cycle (UM Table 5-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionCode(pub u8);

impl FunctionCode {
    pub const USER_DATA: Self = Self(1);
    pub const USER_PROGRAM: Self = Self(2);
    pub const SUPERVISOR_DATA: Self = Self(5);
    pub const SUPERVISOR_PROGRAM: Self = Self(6);
    /// Interrupt acknowledge (UM 5.1.2).
    pub const CPU_SPACE: Self = Self(7);

    pub fn is_supervisor(self) -> bool {
        self.0 & 4 != 0
    }
}

/// The machine terminated a cycle with BERR: the core takes a bus error exception (UM 6.3.9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusFault;

/// How an interrupt-acknowledge cycle ended (UM 6.3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IackResponse {
    /// The device put a vector number on the data bus.
    Vector(u8),
    /// VPA was asserted: the core uses the autovector for the level (vector 24 + level).
    Autovector,
    /// BERR was asserted: the spurious interrupt vector (24).
    Spurious,
}

/// The bus the core drives. `addr` is the 24-bit address on A23-A1 (plus the strobe for a byte).
pub trait M68kBus {
    /// One read cycle; a word is big-endian, a byte comes back in the low 8 bits.
    fn read(&mut self, addr: u32, size: Size, fc: FunctionCode) -> Result<u32, BusFault>;
    /// One write cycle; a byte write uses the low 8 bits of `value`.
    fn write(
        &mut self,
        addr: u32,
        size: Size,
        value: u32,
        fc: FunctionCode,
    ) -> Result<(), BusFault>;
    /// The interrupt-acknowledge cycle for `level` (1-7).
    fn iack(&mut self, level: u8) -> IackResponse;
    /// Internal (non-bus) CPU clocks since the previous call; each bus access itself takes 4 clocks and is not
    /// included. Called before every access and at the end of every instruction. A machine that only needs the
    /// instruction's total (returned by `Cpu::step`) leaves it empty.
    fn sync(&mut self, _internal_clocks: u32) {}
    /// The RESET instruction asserted the RESET line (UM 6.3.1): reset the external devices.
    fn reset_devices(&mut self) {}
}
