use crate::Bus;
use crate::opcodes::{IndexReg, base_table, cb_table, ed_table, indexed_cb_table};

pub(crate) const FLAG_C: u8 = 0x01;
pub(crate) const FLAG_N: u8 = 0x02;
pub(crate) const FLAG_PV: u8 = 0x04;
pub(crate) const FLAG_F3: u8 = 0x08;
pub(crate) const FLAG_H: u8 = 0x10;
pub(crate) const FLAG_F5: u8 = 0x20;
pub(crate) const FLAG_Z: u8 = 0x40;
pub(crate) const FLAG_S: u8 = 0x80;

/// `#[cold]`: `step()`'s `halted` branch is untaken for the overwhelming
/// majority of instructions -- hints the optimizer the same way
/// `accept_interrupt` being `#[cold]` does.
#[cold]
fn halted_nop() -> u8 {
    4
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Cpu {
    pub af: u16,
    pub bc: u16,
    pub de: u16,
    pub hl: u16,

    pub af_: u16,
    pub bc_: u16,
    pub de_: u16,
    pub hl_: u16,

    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,

    pub i: u8,
    pub r: u8,

    pub iff1: bool,
    pub iff2: bool,
    pub im: u8,

    pub halted: bool,

    /// Which index register (IX/IY), if any, a DD/FD prefix has substituted
    /// for HL on the instruction currently being dispatched. Reset to `None`
    /// at the start of every `step()`.
    pub(crate) active_index: Option<IndexReg>,

    /// Extra T-states accrued mid-dispatch (e.g. the displacement-byte fetch
    /// for `(IX+d)`/`(IY+d)` addressing) that a handler's own fixed return
    /// value doesn't account for. Reset to 0 at the start of every `step()`
    /// and added to the total once the handler returns.
    pub(crate) pending_cycles: u8,

    /// Set by `request_interrupt`, consumed (and cleared) at the top of the
    /// next `step()` call regardless of `halted` -- real hardware holds the
    /// interrupt line up rather than latching it only at specific moments,
    /// so checking once per instruction boundary (every `step()` call) is
    /// the natural match for this codebase's one-`step()`-per-instruction
    /// shape. Whether it's actually *taken* still depends on `iff1`.
    pub(crate) irq_pending: bool,

    /// Set by `request_nmi`, consumed at the top of the next `step()` call.
    /// Unlike `irq_pending`, always accepted regardless of `iff1` (real
    /// NMI is non-maskable) and checked first (real Z80: NMI takes
    /// priority over a simultaneously-pending maskable interrupt).
    pub(crate) nmi_pending: bool,

    /// Set by `EI`: the Z80 does not accept a maskable interrupt until the
    /// instruction *after* `EI` has run (so `EI; RET` can return before an
    /// already-waiting interrupt is taken). Taken (cleared) at the top of
    /// every `step()`; only a maskable interrupt is held off, never an NMI.
    pub(crate) ei_delay: bool,

    /// The hidden "Q" latch: the flags byte if the previous instruction wrote
    /// F, else 0. Only SCF/CCF read it (their F3/F5 are `(Q ^ F) | A`). Not
    /// part of a saved state; a freshly loaded CPU starts with 0.
    #[serde(skip)]
    pub(crate) q: u8,
    /// What the instruction now executing has written to F (0 if nothing);
    /// becomes `q` when the next one starts.
    #[serde(skip)]
    pub(crate) q_next: u8,

    /// The hidden MEMPTR (WZ) register: the 16-bit internal address buffer that BIT n,(HL) shows
    /// bits 11 and 13 of in F3/F5 and that most address-handling instructions leave a value in
    /// (Boo-boo's "MEMPTR, esoteric register of the Zilog Z80 CPU"). Not part of a saved state; a
    /// freshly loaded CPU starts with 0.
    #[serde(skip)]
    pub(crate) memptr: u16,
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    /// The refresh register's low seven bits count opcode-fetch (M1)
    /// cycles; bit 7 only ever changes through `LD R,A`. Real software reads
    /// it (`LD A,R`) as a cheap pseudo-random number -- the CPC firmware's
    /// tape-sampling loop seeds its phase from it, and SpeedLock-style
    /// loaders key off it -- so a constant R changes behaviour.
    #[inline]
    fn refresh(&mut self) {
        self.r = (self.r & 0x80) | (self.r.wrapping_add(1) & 0x7F);
    }

    /// Ordered register name/value pairs, for debugging/introspection.
    /// Widened to `u64` so any future (possibly wider) CPU core can report
    /// through the same shape.
    pub fn registers(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("AF", self.af as u64),
            ("BC", self.bc as u64),
            ("DE", self.de as u64),
            ("HL", self.hl as u64),
            ("AF'", self.af_ as u64),
            ("BC'", self.bc_ as u64),
            ("DE'", self.de_ as u64),
            ("HL'", self.hl_ as u64),
            ("IX", self.ix as u64),
            ("IY", self.iy as u64),
            ("SP", self.sp as u64),
            ("PC", self.pc as u64),
            ("I", self.i as u64),
            ("R", self.r as u64),
            ("IM", self.im as u64),
            ("IFF1", self.iff1 as u64),
            ("IFF2", self.iff2 as u64),
        ]
    }
    pub fn set_register(&mut self, name: &str, value: u64) -> Result<(), String> {
        let reg = name.trim();
        if reg.is_empty() {
            return Err("empty register name".to_string());
        }
        let parse_u16 = |label: &str| -> Result<u16, String> {
            u16::try_from(value).map_err(|_| format!("value out of range for register {label}"))
        };
        let parse_u8 = |label: &str| -> Result<u8, String> {
            u8::try_from(value).map_err(|_| format!("value out of range for register {label}"))
        };

        let upper = reg.to_ascii_uppercase();
        match upper.as_str() {
            "AF" => self.af = parse_u16("AF")?,
            "BC" => self.bc = parse_u16("BC")?,
            "DE" => self.de = parse_u16("DE")?,
            "HL" => self.hl = parse_u16("HL")?,
            "AF'" => self.af_ = parse_u16("AF'")?,
            "BC'" => self.bc_ = parse_u16("BC'")?,
            "DE'" => self.de_ = parse_u16("DE'")?,
            "HL'" => self.hl_ = parse_u16("HL'")?,
            "IX" => self.ix = parse_u16("IX")?,
            "IY" => self.iy = parse_u16("IY")?,
            "SP" => self.sp = parse_u16("SP")?,
            "PC" => self.pc = parse_u16("PC")?,
            "A" => self.set_a(parse_u8("A")?),
            "F" => self.set_f(parse_u8("F")?),
            "B" => self.set_b(parse_u8("B")?),
            "C" => self.set_c(parse_u8("C")?),
            "D" => self.set_d(parse_u8("D")?),
            "E" => self.set_e(parse_u8("E")?),
            "H" => self.set_h(parse_u8("H")?),
            "L" => self.set_l(parse_u8("L")?),
            "IXH" => self.set_ixh(parse_u8("IXH")?),
            "IXL" => self.set_ixl(parse_u8("IXL")?),
            "IYH" => self.set_iyh(parse_u8("IYH")?),
            "IYL" => self.set_iyl(parse_u8("IYL")?),
            "I" => self.i = parse_u8("I")?,
            "R" => self.r = parse_u8("R")?,
            "IM" => {
                let im = parse_u8("IM")?;
                if im > 2 {
                    return Err("IM must be 0, 1, or 2".to_string());
                }
                self.im = im;
            }
            "IFF1" => {
                self.iff1 = match value {
                    0 => false,
                    1 => true,
                    _ => return Err("IFF1 must be 0 or 1".to_string()),
                }
            }
            "IFF2" => {
                self.iff2 = match value {
                    0 => false,
                    1 => true,
                    _ => return Err("IFF2 must be 0 or 1".to_string()),
                }
            }
            _ => return Err(format!("unknown register {name:?}")),
        }
        Ok(())
    }

    /// Raises the maskable interrupt line. Real hardware (e.g. a ZX
    /// Spectrum's ULA) does this once per video frame; this codebase's
    /// callers do the same, once per `frame_hz()` tick (see
    /// `machine::Clock::run` and `retroclawd`'s `emu_thread` background
    /// thread).
    /// Doesn't check `iff1` here -- the interrupt line itself doesn't care
    /// whether the CPU currently has interrupts masked, only `step()`'s
    /// acceptance check does, matching real hardware.
    pub fn request_interrupt(&mut self) {
        self.irq_pending = true;
    }

    /// Raises the non-maskable interrupt line. Real hardware (e.g. a ZX81's
    /// NMI generator, running at a fixed rate while enabled) fires this far
    /// more often than the once-per-frame maskable interrupt above --
    /// callers that need that finer grain (see `zx81::Zx81::step`) track
    /// their own cycle-accumulator and call this directly rather than going
    /// through `Machine::request_interrupt`'s once-per-`frame_hz()`-tick
    /// shape, which is too coarse for it.
    pub fn request_nmi(&mut self) {
        self.nmi_pending = true;
    }

    /// Contended-memory-aware instruction fetch (M1 cycle, base cost 4
    /// T-states). Every opcode-stream byte fetch in `step()`'s own dispatch
    /// loop goes through this rather than a raw `bus.read` -- real hardware
    /// contends the *first* T-state of an instruction fetch exactly like any
    /// other memory access. `Bus::contend_memory`/`advance_tstates` default
    /// to no-ops, so this costs nothing extra for a bus that doesn't model
    /// contention.
    #[inline]
    pub(crate) fn fetch<B: Bus>(&mut self, bus: &mut B, addr: u16) -> u8 {
        let delay = bus.contend_memory(addr);
        self.pending_cycles += delay;
        bus.advance_tstates(delay + 4);
        bus.fetch_opcode(addr)
    }

    /// Contended-memory-aware operand/data read (base cost 3 T-states) --
    /// every handler's memory read goes through this instead of calling
    /// `bus.read` directly, so screen-RAM/contended-bank accesses pick up
    /// the real wait-state delay wherever they occur mid-instruction.
    #[inline]
    pub(crate) fn mem_read<B: Bus>(&mut self, bus: &mut B, addr: u16) -> u8 {
        let delay = bus.contend_memory(addr);
        self.pending_cycles += delay;
        bus.advance_tstates(delay + 3);
        bus.read(addr)
    }

    /// Contended-memory-aware operand/data write -- see `mem_read`.
    #[inline]
    pub(crate) fn mem_write<B: Bus>(&mut self, bus: &mut B, addr: u16, val: u8) {
        let delay = bus.contend_memory(addr);
        self.pending_cycles += delay;
        bus.advance_tstates(delay + 3);
        bus.write(addr, val);
    }

    /// Reports `n` internal T-states spent right here in the instruction (see
    /// `Bus::internal_cycles`). They are already part of the handler's returned
    /// base cost; this only tells the bus *where* they fell.
    #[inline]
    pub(crate) fn internal_cycles<B: Bus>(&mut self, bus: &mut B, n: u8) {
        bus.internal_cycles(n);
    }

    /// IO read wrapper (base cost 4 T-states). `Bus::contend_io` has its
    /// own, differently-shaped timing model from memory contention -- see
    /// `zx_spectrum::bus::ZxSpectrumBus::contend_io`'s doc comment.
    #[inline]
    pub(crate) fn io_read<B: Bus>(&mut self, bus: &mut B, port: u16) -> u8 {
        let delay = bus.contend_io(port);
        self.pending_cycles += delay;
        bus.advance_tstates(delay + 4);
        let value = bus.read_io(port);
        bus.io_accessed(port, value, false);
        value
    }

    /// IO write wrapper -- see `io_read`.
    #[inline]
    pub(crate) fn io_write<B: Bus>(&mut self, bus: &mut B, port: u16, val: u8) {
        let delay = bus.contend_io(port);
        self.pending_cycles += delay;
        bus.advance_tstates(delay + 4);
        bus.write_io(port, val);
        bus.io_accessed(port, val, true);
    }

    /// Generic over the bus type (`B: Bus`) rather than `&mut dyn Bus` --
    /// this is the single hot loop of the interpreter, called once per
    /// instruction, so every `bus.read`/`bus.write` here needs to be
    /// inlinable rather than an indirect vtable call. Each dispatch table
    /// (`base_table::<B>()` etc.) is built once per concrete `B` at compile
    /// time, not once per call -- see the doc comment on `base_table`.
    pub fn step<B: Bus + 'static>(&mut self, bus: &mut B) -> u8 {
        // Reset up front (moved ahead of the NMI/IRQ checks below) so
        // `accept_nmi`/`accept_interrupt`'s own stack-push writes -- real
        // memory accesses, contendable like any other -- have a fresh
        // accumulator to add their contention delay into.
        self.pending_cycles = 0;
        self.q = std::mem::take(&mut self.q_next);

        let after_ei = std::mem::take(&mut self.ei_delay);

        // NMI checked first (real Z80 priority: NMI beats a simultaneously-
        // pending maskable interrupt) and unconditionally -- unlike the
        // maskable interrupt below, non-maskable really means it, no `iff1`
        // check. Both wake the CPU from HALT and vector immediately, same
        // as real hardware.
        if self.nmi_pending {
            self.nmi_pending = false;
            let cost = self.accept_nmi(bus);
            return cost + self.pending_cycles;
        }

        // If masked (`iff1` false) this just clears the pending flag and
        // falls through to normal fetch/dispatch (or the `halted` no-op
        // below), exactly as if the line were never raised for this
        // instruction.
        //
        // Right after `EI` the request is left pending (not dropped): the
        // line is still up once the next instruction has run, and that is
        // the earliest the interrupt may be taken.
        if self.irq_pending && !after_ei {
            self.irq_pending = false;
            if self.iff1 {
                let cost = self.accept_interrupt(bus);
                return cost + self.pending_cycles;
            }
        }

        if self.halted {
            // Real HALT keeps re-executing an implicit NOP in place (PC
            // does not advance) until an enabled interrupt arrives above.
            // Each of those is a real M1 cycle, so R keeps counting.
            self.refresh();
            return halted_nop();
        }

        self.active_index = None;

        let base_table = base_table::<B>();
        let cb_table = cb_table::<B>();
        let ed_table = ed_table::<B>();
        let indexed_cb_table = indexed_cb_table::<B>();

        let mut opcode = self.fetch(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        self.refresh();
        let mut prefix_cycles = 0u8;

        loop {
            match opcode {
                0xCB => {
                    let sub = self.fetch(bus, self.pc);
                    self.pc = self.pc.wrapping_add(1);
                    self.refresh();
                    return prefix_cycles + cb_table[sub as usize](self, bus) + self.pending_cycles;
                }
                0xED => {
                    let sub = self.fetch(bus, self.pc);
                    self.pc = self.pc.wrapping_add(1);
                    self.refresh();
                    return prefix_cycles + ed_table[sub as usize](self, bus) + self.pending_cycles;
                }
                0xDD | 0xFD => {
                    self.active_index = Some(if opcode == 0xDD {
                        IndexReg::Ix
                    } else {
                        IndexReg::Iy
                    });
                    prefix_cycles += 4;
                    opcode = self.fetch(bus, self.pc);
                    self.pc = self.pc.wrapping_add(1);
                    self.refresh();

                    if opcode == 0xCB {
                        // Real hardware: the displacement byte is fetched as
                        // a plain (contended) memory read, not an M1 opcode
                        // fetch -- only the following sub-opcode byte is
                        // itself fetched as M1 (for timing; R counts just
                        // the `DD`/`FD` and `CB` fetches, two in all).
                        let d = self.mem_read(bus, self.pc) as i8;
                        self.pc = self.pc.wrapping_add(1);
                        let sub = self.fetch(bus, self.pc);
                        self.pc = self.pc.wrapping_add(1);
                        return prefix_cycles
                            + indexed_cb_table[sub as usize](self, bus, d)
                            + self.pending_cycles;
                    }
                    // Loop back around and dispatch `opcode` through base_table
                    // with `active_index` set.
                }
                _ => {
                    return prefix_cycles
                        + base_table[opcode as usize](self, bus)
                        + self.pending_cycles;
                }
            }
        }
    }

    /// Runs the maskable-interrupt acknowledge sequence: pushes `pc` (like a
    /// CALL), clears `iff1`/`iff2` (matches real hardware -- unlike NMI
    /// below, which clears only `iff1`, leaving `iff2` for `RETN` to
    /// restore from), then vectors per `im`. Only reachable from `step()`,
    /// and only when `iff1` was true at the check. `#[cold]`: at most once
    /// per `step()` call, vs. every call for the rest of dispatch -- hints
    /// the optimizer to keep this out of the hot path `step()` otherwise is.
    #[cold]
    fn accept_interrupt<B: Bus + 'static>(&mut self, bus: &mut B) -> u8 {
        self.halted = false;
        self.iff1 = false;
        self.iff2 = false;
        self.refresh(); // the acknowledge cycle is an M1 cycle
        bus.interrupt_acknowledged();

        self.sp = self.sp.wrapping_sub(1);
        self.mem_write(bus, self.sp, (self.pc >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        self.mem_write(bus, self.sp, self.pc as u8);

        match self.im {
            2 => {
                // IM2: vector is a 16-bit pointer read from `(I:byte)`,
                // where `byte` is whatever the interrupting device places
                // on the data bus during acknowledge -- `0xFF` (a
                // floating/pulled-up bus) unless the bus models a real
                // vectoring controller (see `Bus::interrupt_vector_byte`'s
                // own doc comment).
                let vector_byte = bus.interrupt_vector_byte();
                let ptr = ((self.i as u16) << 8) | vector_byte as u16;
                let lo = self.mem_read(bus, ptr);
                let hi = self.mem_read(bus, ptr.wrapping_add(1));
                self.pc = ((hi as u16) << 8) | lo as u16;
                self.memptr = self.pc;
                19
            }
            _ => {
                // IM0 and IM1 both simplified to RST 38h: real IM0 would
                // execute whatever instruction the interrupting device
                // places on the data bus, but neither ZX81 nor ZX Spectrum's
                // ROM uses IM0, and IM1 genuinely always jumps to 0x0038 on
                // real hardware -- this covers the only mode either machine
                // needs.
                self.pc = 0x0038;
                self.memptr = self.pc;
                13
            }
        }
    }

    /// Runs the NMI acknowledge sequence: pushes `pc`, clears `iff1` only
    /// (unlike maskable-interrupt acceptance, `iff2` is preserved -- real
    /// hardware's `RETN` instruction restores `iff1` from it, letting code
    /// interrupted with maskable interrupts enabled resume that way once
    /// the NMI handler returns), and vectors to the fixed `0x0066` -- NMI
    /// is not vectored through `im` on real hardware, always this address.
    /// 11 T-states: 5 for the acknowledge (replaces the normal opcode
    /// fetch's M1 cycle) + 3+3 for the two-byte push, the real Z80 NMI
    /// response time. `#[cold]` for the same reason `accept_interrupt` is.
    #[cold]
    fn accept_nmi<B: Bus + 'static>(&mut self, bus: &mut B) -> u8 {
        self.halted = false;
        self.iff1 = false;
        self.refresh();

        self.sp = self.sp.wrapping_sub(1);
        self.mem_write(bus, self.sp, (self.pc >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        self.mem_write(bus, self.sp, self.pc as u8);

        self.pc = 0x0066;
        self.memptr = self.pc;
        11
    }

    /// The 16-bit register an opcode's fixed "HL" role resolves to: HL
    /// normally, or IX/IY under an active DD/FD prefix. Used by opcodes that
    /// reference HL directly rather than through the 3-bit register-field
    /// encoding (`LD dd,nn`, `INC/DEC ss`, `ADD HL,ss`, `PUSH/POP HL`,
    /// `JP (HL)`, `LD SP,HL`, `EX (SP),HL`). `EX DE,HL` is a documented
    /// exception -- it always operates on real HL, prefix or not.
    #[allow(dead_code)] // wired up by the base_loads/base_arith16/base_control/base_stack_misc opcode groups
    pub(crate) fn hl_or_index(&self) -> u16 {
        match self.active_index {
            None => self.hl,
            Some(IndexReg::Ix) => self.ix,
            Some(IndexReg::Iy) => self.iy,
        }
    }

    #[allow(dead_code)] // wired up by the base_loads/base_arith16/base_control/base_stack_misc opcode groups
    pub(crate) fn set_hl_or_index(&mut self, val: u16) {
        match self.active_index {
            None => self.hl = val,
            Some(IndexReg::Ix) => self.ix = val,
            Some(IndexReg::Iy) => self.iy = val,
        }
    }

    /// `A` is the high byte of `af` -- standard Z80 register-pair layout.
    /// `pub` (unlike most single-register accessors, which stay
    /// `pub(crate)` for the opcode handlers' own use): a host driving this
    /// core sometimes needs clean A/F access without decomposing `af`
    /// itself -- e.g. `zx_spectrum`'s fast-tape-load ROM trap, which reads/
    /// writes the real `LD-BYTES` routine's register contract directly.
    pub fn a(&self) -> u8 {
        (self.af >> 8) as u8
    }

    pub fn set_a(&mut self, val: u8) {
        self.af = (self.af & 0x00FF) | ((val as u16) << 8);
    }

    /// `F` is the low byte of `af` -- see `a`'s doc comment for why this is
    /// `pub`. Bit 0 is the carry flag on every Z80 (not re-exporting this
    /// crate's own internal `FLAG_C` constant, which stays `pub(crate)` --
    /// bit 0 meaning "carry" is standard Z80 knowledge, not something a
    /// host needs this crate's own dispatch-table internals for).
    pub fn f(&self) -> u8 {
        self.af as u8
    }

    /// The hidden MEMPTR (WZ) register (see the field's doc comment).
    pub fn memptr(&self) -> u16 {
        self.memptr
    }

    pub fn set_memptr(&mut self, val: u16) {
        self.memptr = val;
    }

    pub fn set_f(&mut self, val: u8) {
        self.q_next = val;
        self.af = (self.af & 0xFF00) | val as u16;
    }

    pub(crate) fn b(&self) -> u8 {
        (self.bc >> 8) as u8
    }

    pub(crate) fn set_b(&mut self, val: u8) {
        self.bc = (self.bc & 0x00FF) | ((val as u16) << 8);
    }

    pub(crate) fn c(&self) -> u8 {
        self.bc as u8
    }

    pub(crate) fn set_c(&mut self, val: u8) {
        self.bc = (self.bc & 0xFF00) | val as u16;
    }

    pub(crate) fn d(&self) -> u8 {
        (self.de >> 8) as u8
    }

    pub(crate) fn set_d(&mut self, val: u8) {
        self.de = (self.de & 0x00FF) | ((val as u16) << 8);
    }

    pub(crate) fn e(&self) -> u8 {
        self.de as u8
    }

    pub(crate) fn set_e(&mut self, val: u8) {
        self.de = (self.de & 0xFF00) | val as u16;
    }

    pub(crate) fn h(&self) -> u8 {
        (self.hl >> 8) as u8
    }

    pub(crate) fn set_h(&mut self, val: u8) {
        self.hl = (self.hl & 0x00FF) | ((val as u16) << 8);
    }

    pub(crate) fn l(&self) -> u8 {
        self.hl as u8
    }

    pub(crate) fn set_l(&mut self, val: u8) {
        self.hl = (self.hl & 0xFF00) | val as u16;
    }

    pub(crate) fn ixh(&self) -> u8 {
        (self.ix >> 8) as u8
    }

    pub(crate) fn set_ixh(&mut self, val: u8) {
        self.ix = (self.ix & 0x00FF) | ((val as u16) << 8);
    }

    pub(crate) fn ixl(&self) -> u8 {
        self.ix as u8
    }

    pub(crate) fn set_ixl(&mut self, val: u8) {
        self.ix = (self.ix & 0xFF00) | val as u16;
    }

    pub(crate) fn iyh(&self) -> u8 {
        (self.iy >> 8) as u8
    }

    pub(crate) fn set_iyh(&mut self, val: u8) {
        self.iy = (self.iy & 0x00FF) | ((val as u16) << 8);
    }

    pub(crate) fn iyl(&self) -> u8 {
        self.iy as u8
    }

    pub(crate) fn set_iyl(&mut self, val: u8) {
        self.iy = (self.iy & 0xFF00) | val as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FlatBus;

    /// A bus whose opcode fetches at `0x8000` and above run as NOPs when bit 6 of the byte is clear, as the ZX81's
    /// video circuit does; data reads and operands are untouched.
    struct EchoBus(FlatBus);

    impl Bus for EchoBus {
        fn read(&mut self, addr: u16) -> u8 {
            self.0.read(addr)
        }
        fn write(&mut self, addr: u16, val: u8) {
            self.0.write(addr, val);
        }
        fn fetch_opcode(&mut self, addr: u16) -> u8 {
            let byte = self.read(addr);
            if addr >= 0x8000 && byte & 0x40 == 0 {
                0x00
            } else {
                byte
            }
        }
    }

    #[test]
    fn fetch_opcode_changes_what_runs_but_not_operands_or_data_reads() {
        let mut bus = EchoBus(FlatBus::new());
        // 0x8000: INC A, INC A, ADD A,0x11, LD A,(HL), HALT. Opcodes 0x3C (INC A) have bit 6 clear and become
        // NOPs; 0xC6 (ADD A,n), 0x7E (LD A,(HL)) and 0x76 (HALT) have it set and run.
        for (i, b) in [0x3C, 0x3C, 0xC6, 0x11, 0x7E, 0x76].iter().enumerate() {
            bus.write(0x8000 + i as u16, *b);
        }
        let mut cpu = Cpu::new();
        cpu.pc = 0x8000;
        cpu.hl = 0x8000;

        cpu.step(&mut bus);
        cpu.step(&mut bus);
        assert_eq!(cpu.af >> 8, 0, "INC A fetched at 0x8000 ran as a NOP");
        // The operand 0x11 has bit 6 clear: read as data it is 0x11, fetched as an opcode it would be a NOP.
        cpu.step(&mut bus);
        assert_eq!(cpu.af >> 8, 0x11, "an operand is not an opcode fetch");
        // A data read of the byte at 0x8000 sees the real 0x3C, not the NOP's 0x00.
        cpu.step(&mut bus);
        assert_eq!(cpu.af >> 8, 0x3C, "a data read is not an opcode fetch");
        cpu.step(&mut bus);
        assert!(cpu.halted, "0x76 has bit 6 set: a real HALT");
    }

    #[test]
    fn nop_advances_pc_and_takes_4_cycles() {
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0x00);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.pc, 1);
        assert_eq!(
            cpu,
            Cpu {
                pc: 1,
                r: 1, // one M1 fetch
                ..Cpu::new()
            }
        );
    }

    /// R counts one per M1 cycle (opcode and prefix fetches, interrupt
    /// acknowledge, each re-executed HALT NOP) in its low seven bits and
    /// leaves bit 7 alone -- the CPC firmware's tape loop and SpeedLock-style
    /// loaders read it with `LD A,R`.
    #[test]
    fn r_counts_one_per_m1_cycle_and_keeps_bit_7() {
        let r_after = |program: &[u8], r0: u8| {
            let mut cpu = Cpu::new();
            cpu.r = r0;
            cpu.sp = 0xFF00;
            let mut bus = FlatBus::new();
            for (i, b) in program.iter().enumerate() {
                bus.write(i as u16, *b);
            }
            cpu.step(&mut bus);
            cpu.r
        };
        assert_eq!(r_after(&[0x00], 0), 1, "NOP");
        assert_eq!(r_after(&[0xCB, 0x00], 0), 2, "CB prefix + opcode");
        assert_eq!(r_after(&[0xED, 0x44], 0), 2, "ED prefix + opcode");
        assert_eq!(r_after(&[0xDD, 0x21, 1, 2], 0), 2, "DD prefix + opcode");
        assert_eq!(
            r_after(&[0xFD, 0x00], 0),
            2,
            "FD prefix + non-indexed opcode"
        );
        assert_eq!(
            r_after(&[0xDD, 0xCB, 0x05, 0x06], 0),
            2,
            "DDCB: only DD and CB are M1"
        );
        assert_eq!(
            r_after(&[0xFD, 0xCB, 0x05, 0x06], 0),
            2,
            "FDCB: only FD and CB are M1"
        );
        assert_eq!(
            r_after(&[0xDD, 0xDD, 0x00], 0),
            3,
            "each chained prefix is its own M1"
        );
        // Only the low seven bits count; bit 7 is preserved across the wrap.
        assert_eq!(r_after(&[0x00], 0x80), 0x81);
        assert_eq!(r_after(&[0x00], 0x7F), 0x00);
        assert_eq!(r_after(&[0x00], 0xFF), 0x80);

        // HALT's repeated implicit NOPs and the interrupt acknowledge cycle.
        let mut cpu = Cpu::new();
        cpu.sp = 0xFF00;
        cpu.im = 1;
        let mut bus = FlatBus::new();
        bus.write(0, 0x76); // HALT
        cpu.step(&mut bus);
        assert_eq!(cpu.r, 1, "the HALT opcode fetch");
        cpu.step(&mut bus);
        cpu.step(&mut bus);
        assert_eq!(cpu.r, 3, "each re-executed HALT NOP is an M1 cycle");
        cpu.iff1 = true;
        cpu.request_interrupt();
        cpu.step(&mut bus);
        assert_eq!(cpu.r, 4, "interrupt acknowledge");
        cpu.request_nmi();
        cpu.step(&mut bus);
        assert_eq!(cpu.r, 5, "NMI acknowledge");
    }

    #[test]
    fn an_undefined_ed_opcode_is_an_eight_t_state_nop_not_a_panic() {
        // ED 0x00 is not a defined instruction; real hardware executes it as
        // a two-byte, 8-T-state NOP (the full per-opcode coverage lives next to
        // the ED table in `opcodes::ed`).
        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        bus.write(0, 0xED);
        bus.write(1, 0x00);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 8);
        assert_eq!(cpu.pc, 2);
    }

    #[test]
    fn ld_b_c_copies_register_and_takes_4_cycles() {
        let mut cpu = Cpu::new();
        cpu.set_c(0x42);
        let mut bus = FlatBus::new();
        bus.write(0, 0x41); // LD B,C

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.b(), 0x42);
    }

    #[test]
    fn ld_b_b_is_a_4_cycle_no_op() {
        let mut cpu = Cpu::new();
        cpu.set_b(0x42);
        let mut bus = FlatBus::new();
        bus.write(0, 0x40); // LD B,B

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.b(), 0x42);
    }

    #[test]
    fn ld_a_hl_reads_memory_and_takes_7_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x1234;
        let mut bus = FlatBus::new();
        bus.write(0x1234, 0x99);
        bus.write(0, 0x7E); // LD A,(HL)

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(cpu.a(), 0x99);
    }

    #[test]
    fn ld_hl_a_writes_memory_and_takes_7_cycles() {
        let mut cpu = Cpu::new();
        cpu.hl = 0x1234;
        cpu.set_a(0x77);
        let mut bus = FlatBus::new();
        bus.write(0, 0x77); // LD (HL),A

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 7);
        assert_eq!(bus.read(0x1234), 0x77);
    }

    #[test]
    fn add_a_b_sets_half_carry_and_overflow_on_signed_wrap() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x7F);
        cpu.set_b(0x01);
        let mut bus = FlatBus::new();
        bus.write(0, 0x80); // ADD A,B

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.f() & FLAG_S, FLAG_S);
        assert_eq!(cpu.f() & FLAG_Z, 0);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV);
        assert_eq!(cpu.f() & FLAG_N, 0);
        assert_eq!(cpu.f() & FLAG_C, 0);
    }

    #[test]
    fn sub_a_b_sets_borrow_without_overflow() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x00);
        cpu.set_b(0x01);
        let mut bus = FlatBus::new();
        bus.write(0, 0x90); // SUB A,B

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0xFF);
        assert_eq!(cpu.f() & FLAG_C, FLAG_C);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_PV, 0);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
    }

    #[test]
    fn and_a_b_sets_half_carry_and_parity() {
        let mut cpu = Cpu::new();
        cpu.set_a(0xFF);
        cpu.set_b(0x0F);
        let mut bus = FlatBus::new();
        bus.write(0, 0xA0); // AND A,B

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x0F);
        assert_eq!(cpu.f() & FLAG_H, FLAG_H);
        assert_eq!(cpu.f() & FLAG_C, 0);
        assert_eq!(cpu.f() & FLAG_PV, FLAG_PV); // 0x0F has even parity
    }

    #[test]
    fn xor_a_a_zeroes_and_sets_zero_flag() {
        let mut cpu = Cpu::new();
        cpu.set_a(0xFF);
        let mut bus = FlatBus::new();
        bus.write(0, 0xAF); // XOR A,A

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.f() & FLAG_Z, FLAG_Z);
        assert_eq!(cpu.f() & FLAG_H, 0);
        assert_eq!(cpu.f() & FLAG_C, 0);
    }

    #[test]
    fn cp_a_b_leaves_a_unchanged_and_takes_f3_f5_from_operand() {
        let mut cpu = Cpu::new();
        cpu.set_a(0x10);
        cpu.set_b(0x08); // bit 3 set, bit 5 clear
        let mut bus = FlatBus::new();
        bus.write(0, 0xB8); // CP A,B

        cpu.step(&mut bus);

        assert_eq!(cpu.a(), 0x10); // CP does not store the result
        assert_eq!(cpu.f() & FLAG_F3, FLAG_F3);
        assert_eq!(cpu.f() & FLAG_F5, 0);
        assert_eq!(cpu.f() & FLAG_N, FLAG_N);
    }

    #[test]
    fn dd_prefixed_reg_reg_load_targets_ixh_and_costs_8_cycles() {
        // DD 60 -- undocumented LD IXH,B. Exercises DD reuse of the plain
        // LD r,r' table via reg_get/reg_set's active_index substitution.
        let mut cpu = Cpu::new();
        cpu.set_b(0x42);
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x60);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 8);
        assert_eq!(cpu.ixh(), 0x42);
        assert_eq!(cpu.h(), 0); // real HL untouched
        assert_eq!(cpu.pc, 2);
    }

    #[test]
    fn dd_prefixed_hl_load_reads_ix_plus_displacement_and_costs_19_cycles() {
        // DD 7E d -- LD A,(IX+d). Exercises the displacement fetch and its
        // extra 8 T-states via indexed_addr/pending_cycles.
        let mut cpu = Cpu::new();
        cpu.ix = 0x1000;
        let mut bus = FlatBus::new();
        bus.write(0, 0xDD);
        bus.write(1, 0x7E);
        bus.write(2, 0x05); // d = +5
        bus.write(0x1005, 0x99);

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 19);
        assert_eq!(cpu.a(), 0x99);
        assert_eq!(cpu.pc, 3);
    }

    #[test]
    fn halted_with_no_pending_interrupt_re_executes_nop_in_place() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;
        cpu.halted = true;
        let mut bus = FlatBus::new();

        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.pc, 0x1000); // real HALT does not advance pc
        assert!(cpu.halted);
    }

    #[test]
    fn masked_interrupt_is_ignored_and_execution_continues_normally() {
        let mut cpu = Cpu::new();
        cpu.iff1 = false; // interrupts disabled
        let mut bus = FlatBus::new();
        bus.write(0, 0x00); // NOP

        cpu.request_interrupt();
        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 4); // ran the NOP, not an interrupt ack
        assert_eq!(cpu.pc, 1);
    }

    #[test]
    fn im1_interrupt_pushes_pc_disables_iff_and_jumps_to_0038() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x8000;
        cpu.sp = 0xFF00;
        cpu.iff1 = true;
        cpu.iff2 = true;
        cpu.im = 1;
        let mut bus = FlatBus::new();

        cpu.request_interrupt();
        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 13);
        assert_eq!(cpu.pc, 0x0038);
        assert!(!cpu.iff1);
        assert!(!cpu.iff2);
        assert_eq!(cpu.sp, 0xFEFE);
        assert_eq!(bus.read(0xFEFE), 0x00); // pushed pc low byte
        assert_eq!(bus.read(0xFEFF), 0x80); // pushed pc high byte
    }

    #[test]
    fn im2_interrupt_vectors_through_the_i_register_table() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x9000;
        cpu.sp = 0xFF00;
        cpu.iff1 = true;
        cpu.im = 2;
        cpu.i = 0x40;
        let mut bus = FlatBus::new();
        // IM2 approximates the interrupting device's data-bus byte as 0xFF,
        // so the vector pointer is (I:0xFF) = 0x40FF.
        bus.write(0x40FF, 0x34);
        bus.write(0x4100, 0x12);

        cpu.request_interrupt();
        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 19);
        assert_eq!(cpu.pc, 0x1234);
    }

    /// A test-only bus modeling a real vectoring controller: a
    /// caller-chosen IM2 vector byte (instead of the default floating-bus
    /// `0xFF`), mirroring the CPC Plus ASIC's own interrupt-vector
    /// register this hook exists for.
    struct VectoringBus {
        inner: FlatBus,
        vector_byte: u8,
    }

    impl Bus for VectoringBus {
        fn read(&mut self, addr: u16) -> u8 {
            self.inner.read(addr)
        }
        fn write(&mut self, addr: u16, val: u8) {
            self.inner.write(addr, val);
        }
        fn interrupt_vector_byte(&mut self) -> u8 {
            self.vector_byte
        }
    }

    #[test]
    fn im2_interrupt_uses_bus_supplied_vector_byte() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x9000;
        cpu.sp = 0xFF00;
        cpu.iff1 = true;
        cpu.im = 2;
        cpu.i = 0x40;
        let mut bus = VectoringBus {
            inner: FlatBus::new(),
            vector_byte: 0x06,
        };
        // A real vectoring controller supplies its own byte (here 0x06,
        // matching the CPC Plus ASIC's raster-interrupt vector low bits)
        // instead of the default floating-bus 0xFF -- the pointer becomes
        // (I:0x06) = 0x4006, not 0x40FF.
        bus.write(0x4006, 0x78);
        bus.write(0x4007, 0x56);

        cpu.request_interrupt();
        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 19);
        assert_eq!(cpu.pc, 0x5678);
    }

    #[test]
    fn interrupt_wakes_a_halted_cpu_and_vectors_immediately() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x2000;
        cpu.sp = 0xFF00;
        cpu.iff1 = true;
        cpu.im = 1;
        cpu.halted = true;
        let mut bus = FlatBus::new();

        cpu.request_interrupt();
        let cycles = cpu.step(&mut bus);

        assert!(!cpu.halted);
        assert_eq!(cpu.pc, 0x0038);
        assert_eq!(cycles, 13); // interrupt ack, not the 4-cycle halted no-op
    }

    #[test]
    fn request_interrupt_is_consumed_even_once_and_does_not_repeat() {
        let mut cpu = Cpu::new();
        cpu.iff1 = true;
        cpu.im = 1;
        let mut bus = FlatBus::new();
        bus.write(0x0038, 0x00); // NOP, so the second step() is unambiguous

        cpu.request_interrupt();
        cpu.step(&mut bus); // accepts the interrupt, jumps to 0x0038
        let cycles = cpu.step(&mut bus); // should just run the NOP at 0x0038

        assert_eq!(cycles, 4);
        assert_eq!(cpu.pc, 0x0039);
    }

    /// `EI` holds a maskable interrupt off until the instruction after it
    /// has run (real Z80 behaviour: lets `EI; RET` return first). The request
    /// is kept, not dropped, so it is taken right after that instruction.
    #[test]
    fn a_waiting_interrupt_is_taken_only_after_the_instruction_following_ei() {
        let mut cpu = Cpu::new();
        cpu.sp = 0xFF00;
        cpu.im = 1;
        let mut bus = FlatBus::new();
        bus.write(0x0000, 0xFB); // EI
        bus.write(0x0001, 0x00); // NOP
        bus.write(0x0002, 0x00); // NOP

        cpu.step(&mut bus); // EI
        cpu.request_interrupt();
        cpu.step(&mut bus); // the NOP must run before the interrupt
        assert_eq!(cpu.pc, 2, "interrupt must not be taken right after EI");
        cpu.step(&mut bus); // now the interrupt is accepted
        assert_eq!(cpu.pc, 0x0038);
        assert_eq!(
            bus.read(0xFEFE),
            0x02,
            "pushed return address is after the NOP"
        );
    }

    /// An NMI is never held off by `EI`.
    #[test]
    fn nmi_right_after_ei_is_not_delayed() {
        let mut cpu = Cpu::new();
        cpu.sp = 0xFF00;
        let mut bus = FlatBus::new();
        bus.write(0x0000, 0xFB); // EI

        cpu.step(&mut bus);
        cpu.request_nmi();
        cpu.step(&mut bus);

        assert_eq!(cpu.pc, 0x0066);
    }

    struct AckCountingBus {
        inner: FlatBus,
        acks: u32,
    }

    impl Bus for AckCountingBus {
        fn read(&mut self, addr: u16) -> u8 {
            self.inner.read(addr)
        }
        fn write(&mut self, addr: u16, val: u8) {
            self.inner.write(addr, val);
        }
        fn interrupt_acknowledged(&mut self) {
            self.acks += 1;
        }
    }

    /// The acknowledge hook fires once per accepted maskable interrupt, in
    /// every mode, and never for a masked or NMI request.
    #[test]
    fn accepting_a_maskable_interrupt_calls_the_acknowledge_hook_once() {
        for im in [0u8, 1, 2] {
            let mut cpu = Cpu::new();
            cpu.sp = 0xFF00;
            cpu.iff1 = true;
            cpu.im = im;
            let mut bus = AckCountingBus {
                inner: FlatBus::new(),
                acks: 0,
            };
            cpu.request_interrupt();
            cpu.step(&mut bus);
            assert_eq!(bus.acks, 1, "IM{im}");
        }

        let mut cpu = Cpu::new();
        cpu.sp = 0xFF00;
        let mut bus = AckCountingBus {
            inner: FlatBus::new(),
            acks: 0,
        };
        cpu.request_interrupt(); // masked (IFF1 clear)
        cpu.step(&mut bus);
        cpu.request_nmi();
        cpu.step(&mut bus);
        assert_eq!(bus.acks, 0);
    }

    #[test]
    fn nmi_is_accepted_even_with_interrupts_disabled() {
        let mut cpu = Cpu::new();
        cpu.pc = 0x8000;
        cpu.sp = 0xFF00;
        cpu.iff1 = false; // NMI is non-maskable -- must fire anyway.
        let mut bus = FlatBus::new();

        cpu.request_nmi();
        let cycles = cpu.step(&mut bus);

        assert_eq!(cycles, 11);
        assert_eq!(cpu.pc, 0x0066);
        assert_eq!(cpu.sp, 0xFEFE);
        assert_eq!(bus.read(0xFEFE), 0x00);
        assert_eq!(bus.read(0xFEFF), 0x80);
    }

    #[test]
    fn nmi_clears_iff1_only_and_preserves_iff2() {
        let mut cpu = Cpu::new();
        cpu.iff1 = true;
        cpu.iff2 = true;
        let mut bus = FlatBus::new();

        cpu.request_nmi();
        cpu.step(&mut bus);

        assert!(!cpu.iff1);
        assert!(cpu.iff2); // unlike accept_interrupt, which clears both
    }

    #[test]
    fn nmi_wakes_a_halted_cpu() {
        let mut cpu = Cpu::new();
        cpu.halted = true;
        let mut bus = FlatBus::new();

        cpu.request_nmi();
        cpu.step(&mut bus);

        assert!(!cpu.halted);
        assert_eq!(cpu.pc, 0x0066);
    }

    #[test]
    fn nmi_takes_priority_over_a_simultaneously_pending_maskable_interrupt() {
        let mut cpu = Cpu::new();
        cpu.iff1 = true;
        cpu.im = 1;
        let mut bus = FlatBus::new();

        cpu.request_interrupt();
        cpu.request_nmi();
        let cycles = cpu.step(&mut bus);

        // NMI serviced first, not the maskable interrupt's RST 38h.
        assert_eq!(cycles, 11);
        assert_eq!(cpu.pc, 0x0066);
        // The maskable interrupt request is still latched (real hardware:
        // it's re-evaluated at the next instruction boundary) -- not
        // silently dropped just because NMI went first.
        assert!(cpu.irq_pending);
    }

    #[test]
    fn set_register_updates_named_z80_registers() {
        let mut cpu = Cpu::new();

        cpu.set_register("af", 0x1234).unwrap();
        assert_eq!(cpu.af, 0x1234);

        cpu.set_register("a", 0x56).unwrap();
        assert_eq!(cpu.a(), 0x56);
        assert_eq!(cpu.f(), 0x34);

        cpu.set_register("iff1", 1).unwrap();
        cpu.set_register("im", 2).unwrap();
        assert!(cpu.iff1);
        assert_eq!(cpu.im, 2);
    }

    #[test]
    fn set_register_rejects_out_of_range_or_unknown_values() {
        let mut cpu = Cpu::new();

        let err = cpu.set_register("A", 0x1FF).unwrap_err();
        assert!(err.contains("out of range"));

        let err = cpu.set_register("IM", 3).unwrap_err();
        assert!(err.contains("IM must be 0, 1, or 2"));

        let err = cpu.set_register("does-not-exist", 0).unwrap_err();
        assert!(err.contains("unknown register"));
    }
}
