use crate::Bus;

/// Deliberately implements only `read`/`write` -- no `read_io`/`write_io`
/// override at all. That's the actual M0 proof: a 6502-only `Bus` impl
/// compiles and drives a real `Cpu` with zero IO-port ceremony, because
/// `machine::Bus` defaults those two methods (see `machine::bus`'s doc
/// comment). A Z80 machine's `FlatBus` (`crates/core/z80/src/test_support.rs`)
/// still overrides both; this one doesn't need to.
pub(crate) struct FlatBus {
    mem: [u8; 0x10000],
}

impl FlatBus {
    pub(crate) fn new() -> Self {
        Self { mem: [0; 0x10000] }
    }

    pub(crate) fn load(&mut self, addr: u16, bytes: &[u8]) {
        for (i, b) in bytes.iter().enumerate() {
            self.mem[addr as usize + i] = *b;
        }
    }
}

impl Bus for FlatBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.mem[addr as usize] = val;
    }
}
