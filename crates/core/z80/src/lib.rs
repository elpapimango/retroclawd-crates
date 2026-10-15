mod cpu;
pub mod disassemble;
mod opcodes;
#[cfg(test)]
pub(crate) mod test_support;

/// The bus trait the core drives. It lives in the CPU-agnostic `machine`
/// crate (nothing about it is Z80-specific) and is re-exported here so
/// `z80::Bus` stays a valid import path for anything driving this core.
pub use machine::Bus;

pub use cpu::Cpu;
