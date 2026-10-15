/// The memory/IO bus a CPU core drives. Deliberately CPU-agnostic: it is just
/// byte reads/writes over a 16-bit address space plus a 16-bit IO port space,
/// so any CPU crate can be generic over it and any machine can implement it.
pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, val: u8);

    /// The byte the CPU executes when it fetches an *opcode* (an M1 cycle) at
    /// `addr`: the instruction byte and the CB/ED/DD/FD prefix and sub-opcode
    /// bytes, never an operand or a data read. Defaults to `read`, which is
    /// what nearly every machine wants. A machine whose hardware treats an
    /// opcode fetch differently from a data read overrides it: the ZX81's
    /// video circuit, for one, turns a fetch at 0x8000 and above into a NOP
    /// (and shows the byte as a character) when bit 6 of the byte is clear.
    /// Only the Z80 core calls it; the 6502 core never does.
    #[inline]
    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        self.read(addr)
    }

    /// Defaulted (unlike `read`/`write`): a genuinely optional concept, not a
    /// universal CPU-bus feature. Z80-family buses have a real IO port space
    /// and always override both -- every existing impl in this codebase
    /// already does. A CPU with no separate IO space at all (e.g. 6502 --
    /// everything is memory-mapped) implements `Bus` with no IO methods and
    /// its core simply never calls them, so these defaults are dead code on
    /// that path rather than a per-machine ceremony no-op.
    fn read_io(&mut self, _port: u16) -> u8 {
        0xFF
    }
    fn write_io(&mut self, _port: u16, _val: u8) {}

    /// Called by a CPU core when an instruction spends `n` internal T-states
    /// (no bus access: the extended `M1` of `PUSH`/`RST`/`RET cc`/`DJNZ`, the
    /// address arithmetic of indexed addressing, ...) *at that point in the
    /// instruction*, i.e. before a bus access that follows within the same
    /// instruction. A bus whose later accesses are aligned to a slot grid (the
    /// CPC's Gate Array) needs to know the internal cycle happened before
    /// that access to align it correctly; reconciling internal cycles only at
    /// the end of the instruction (as the total returned by `step` does)
    /// misplaces the alignment. Only called where an access follows; internal
    /// cycles at the very end of an instruction need no hook. Default no-op,
    /// so every other machine is unaffected.
    #[inline]
    fn internal_cycles(&mut self, _n: u8) {}

    /// A bus cycle in which the CPU drives an address (and, for a write, a
    /// value) but uses nothing that comes back -- the cycles a 6502 spends
    /// on "dummy" reads and writes: the opcode-less second cycle of an
    /// implied instruction, the uncorrected-address read of an indexed
    /// access, the stack read of `PLA`/`RTS`, the write-back of the
    /// unmodified value in a read-modify-write, the interrupt sequence's
    /// padding reads. Every 6502 cycle is exactly one of `read`, `write` or
    /// this hook, in real order, so a bus that models per-cycle timing
    /// (the C64's VIC-II stealing read cycles with BA/AEC) can count them
    /// and stall on them. `write` is `true` for a dummy write (`value` is
    /// what is on the data bus), `false` for a dummy read (`value` is 0 --
    /// the CPU never sees it). Called *instead of* `read`/`write` so that a
    /// dummy read has no device side effects unless the bus chooses to
    /// give it some. Default no-op, inlined away: only the 6502 core calls
    /// it, and only a cycle-accurate bus overrides it.
    #[inline]
    fn dummy_cycle(&mut self, _addr: u16, _write: bool, _value: u8) {}

    /// Called by a CPU core's I/O funnel right after every `IN`
    /// (`write == false`, `value` is the byte the CPU received) and `OUT`
    /// (`write == true`, `value` is the byte it sent). The hook the
    /// debugger's port breakpoints hang off; a bus that doesn't offer them
    /// keeps this empty default, which inlines away. It is separate from
    /// `read_io`/`write_io` so a core's single funnel can serve every
    /// machine without each bus having to wrap its (early-returning) port
    /// decode.
    #[inline]
    fn io_accessed(&mut self, _port: u16, _value: u8, _write: bool) {}

    /// Extra wait-state T-states incurred by accessing `addr` right now
    /// (contended/"slow" memory -- e.g. the ZX Spectrum ULA stealing bus
    /// cycles while it reads screen RAM). Called by the CPU core before
    /// every memory access (instruction fetch, read, write). Default `0`:
    /// a bus with no contention (ZX81, bare test harnesses) needs no change.
    fn contend_memory(&mut self, _addr: u16) -> u8 {
        0
    }

    /// Same idea for I/O port access -- a genuinely different, more
    /// elaborate timing model than memory contention on real hardware (see
    /// `zx_spectrum::bus::ZxSpectrumBus::contend_io`'s doc comment for the
    /// ZX Spectrum's own 4-branch model). Default `0`: a bus with no IO
    /// contention (ZX81, bare test harnesses) needs no change.
    fn contend_io(&mut self, _port: u16) -> u8 {
        0
    }

    /// Advances a contention-tracking bus's own notion of "how far into the
    /// current frame are we" by `n` T-states. Called by the CPU core after
    /// every access (contended or not), so a bus that models contention
    /// keeps its position in sync with real elapsed time between accesses
    /// within a single instruction. Default no-op.
    fn advance_tstates(&mut self, _n: u8) {}

    /// The byte a real interrupting device places on the data bus during a
    /// Z80 IM2 acknowledge cycle (used to form the low byte of the vector
    /// pointer, `(I:byte)`). Defaulted to `0xFF` (a floating/pulled-up
    /// bus, the same assumption every existing IM2-vectoring ROM here
    /// already builds its table on) since no interrupting device is
    /// modeled for most machines. A machine with a real vectoring
    /// controller (e.g. the Amstrad CPC Plus ASIC's own interrupt-vector
    /// register) overrides this to supply the real, source-dependent byte.
    fn interrupt_vector_byte(&mut self) -> u8 {
        0xFF
    }

    /// Called once when the CPU accepts a maskable interrupt (any
    /// interrupt mode), at the acknowledge cycle. A device that holds its
    /// interrupt request until the CPU answers it (the Amstrad CPC's Gate
    /// Array) drops the request here -- and may adjust its own counters, as
    /// the Gate Array does (it clears bit 5 of its scanline counter).
    /// Defaulted to a no-op: the ZX81/Spectrum/C64 model interrupts as a
    /// pulse the CPU either takes or misses, so nothing needs to know.
    fn interrupt_acknowledged(&mut self) {}
}
