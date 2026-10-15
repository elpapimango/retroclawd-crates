mod cpu;
pub mod disassemble;
mod opcodes;
#[cfg(test)]
pub(crate) mod test_support;

/// The bus trait the core drives. It lives in the CPU-agnostic `machine`
/// crate and is re-exported here so `mos6502::Bus` stays a valid import
/// path for anything driving this core -- same convention `z80::Bus`
/// already established. Unlike `z80`, this core never calls `read_io`/
/// `write_io` at all (the 6502 has no separate IO address space, every
/// chip is memory-mapped): those two `Bus` methods are defaulted in
/// `machine` specifically so a 6502-only `Bus` impl need not implement
/// them, proving the trait is genuinely CPU-agnostic, not Z80-shaped.
pub use machine::Bus;

pub use cpu::Cpu;
