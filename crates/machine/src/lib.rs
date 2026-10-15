mod bus;
mod clock;
mod memory;
mod port_watch;
mod region;
mod symbols;
mod video;

pub use bus::Bus;
pub use clock::{Clock, RunStats};
pub use memory::MemoryBus;
pub use port_watch::{PortHit, PortWatch, PortWatcher};
pub use region::Region;
pub use symbols::SymbolTable;
pub use video::with_border;

/// The one sample rate every machine's `drain_audio` output and every
/// audio-output consumer (e.g. `retroclawd`'s Bevy audio backend) agree on
/// -- a single shared constant here rather than each machine picking its
/// own, since the whole point is that a caller can treat any machine's
/// audio output identically. 44.1kHz is a completely arbitrary, common
/// choice (not derived from any emulated hardware's own rate -- real
/// AY-3-8910/beeper output is analog) comfortably above twice the highest
/// frequency either the AY-3-8910 or a 1-bit beeper can usefully produce.
pub const AUDIO_SAMPLE_RATE: u32 = 44_100;

/// A CPU + memory system driven by a fixed clock rate. Implemented once per
/// concrete computer (ZX81, ZX Spectrum, ...); `emu` and any future UI only
/// ever hold a `Box<dyn Machine>`, so they work the same way regardless of
/// which CPU chip is inside.
pub trait Machine {
    /// Fixed clock rate for this system, in Hz.
    fn clock_hz(&self) -> u64;

    /// Pacing/refresh rate for this system, in Hz (e.g. 50 for PAL, 60 for
    /// NTSC/Japan). Read by each machine's own `run()` to build a `Clock`
    /// at the right tick rate -- distinct from `clock_hz` (CPU speed).
    fn frame_hz(&self) -> u64;

    /// Runs this machine until `running` returns false, checked between
    /// instructions.
    ///
    /// This has no default body -- do not add one, even the obvious one-liner
    /// `Clock::new(self.frame_hz()).run(self, running)`. A default body would
    /// need `where Self: Sized` to coerce `&mut Self` into `&mut dyn Machine`
    /// for that inner call, and Rust excludes any `Self: Sized`-bounded
    /// method from the trait's vtable -- making it uncallable through
    /// `Box<dyn Machine>`, which is exactly how `emu` calls it. Requiring
    /// each impl to supply its own (identical, one-line) body keeps `run`
    /// fully virtual. As a secondary benefit, this also lets a machine with a
    /// fundamentally different timing model replace the loop entirely.
    fn run(&mut self, running: &mut dyn FnMut(&dyn Machine) -> bool) -> RunStats;

    /// Executes one CPU instruction. Returns its cost in clock cycles
    /// (whatever the underlying chip calls them -- "T-states" for Z80,
    /// this trait itself is CPU-agnostic).
    fn step(&mut self) -> u32;

    /// True once the CPU has halted.
    fn halted(&self) -> bool;

    /// The program counter, alone -- cheap enough to call after every single
    /// `step()` (e.g. a per-instruction breakpoint check), unlike
    /// `registers()` which allocates a whole `Vec` every call. No default
    /// body: every impl already stores its CPU's `pc` as a plain field (see
    /// `set_pc`, already mandatory for the same reason), so implementing
    /// this must never cost more than a direct field read -- a default that
    /// derived it from `registers()` would silently reintroduce the
    /// allocation this method exists to avoid.
    fn pc(&self) -> u16;

    /// Ordered register name/value pairs, e.g. `[("AF", 0x2A00), ...]`.
    /// Order and register set are chip-specific; the shape is uniform so a
    /// future UI can render any machine's registers without knowing which
    /// CPU is inside.
    fn registers(&self) -> Vec<(&'static str, u64)>;

    /// Renders the current video frame as RGB pixels: `(width, height, rgb)`
    /// where `rgb.len() == width as usize * height as usize * 3`, row-major,
    /// top-to-bottom, left-to-right. `None` for machines without video
    /// output implemented yet.
    ///
    /// This is a default method (unlike `run()`) -- it's a plain `&self`
    /// method returning an owned value, with no need to coerce `self` into
    /// `&mut dyn Machine` internally, so it needs no `Self: Sized` bound and
    /// works through `Box<dyn Machine>` with no caveats. Every machine's
    /// video hardware is genuinely different at the implementation level
    /// (a VDP's tile+sprite compositing has nothing in common with a ZX81
    /// reading its own ROM font) -- this method deliberately unifies only
    /// the *output* (a pixel grid), not any generation mechanism, so future
    /// machines with wildly different video hardware can each implement it
    /// without this trait needing to change again.
    fn render_frame(&self) -> Option<(u32, u32, Vec<u8>)> {
        None
    }

    /// Pure read of one memory byte for display purposes -- does not go
    /// through the CPU's normal bus read path (which may have side effects
    /// for I/O-mapped reads).
    fn read_memory(&self, addr: u16) -> u8;

    /// Loads bytes into memory starting at `addr`, through normal write
    /// semantics (a write into ROM is dropped, matching real hardware).
    ///
    /// Returns the count of bytes actually written: a byte landing outside
    /// every mapped region, or inside a ROM region, is not written and not
    /// counted. A caller that needs its whole payload in memory (e.g. a test
    /// program ending in `HALT`) should compare the result against
    /// `bytes.len()` rather than assume it all landed.
    fn load(&mut self, addr: u16, bytes: &[u8]) -> usize;

    /// Sets the program counter. Real hardware always resets to a fixed
    /// vector; this lets a caller (e.g. `emu`) point execution at a loaded
    /// test program instead, since there's no real ROM boot code yet.
    fn set_pc(&mut self, addr: u16);

    /// Raises this machine's frame interrupt (e.g. a ZX Spectrum ULA's
    /// once-per-50Hz-frame `INT` line). Meant to be called once per
    /// `frame_hz()` tick by whatever drives `step()` (`Clock::run()`, or
    /// `retroclawd`'s `emu_thread` background thread, which reimplements
    /// `Clock`'s pacing without using `Clock` itself) -- the interrupt
    /// itself is only actually taken on the CPU's next instruction boundary,
    /// and only if that CPU currently has interrupts enabled, same as real
    /// hardware. A no-op default so a future CPU/machine with no interrupt
    /// concept (or one not modeled yet) doesn't need to implement this.
    ///
    /// This is a default method (unlike `run()`) for the same reason
    /// `render_frame` is: a plain `&mut self` method with no need to coerce
    /// into `&mut dyn Machine`, so it needs no `Self: Sized` bound and works
    /// through `Box<dyn Machine>`/`&mut dyn Machine` with no caveats.
    fn request_interrupt(&mut self) {}

    /// Drains and returns every audio sample this machine has generated
    /// since the last call, as mono `f32` PCM in `-1.0..=1.0`, at
    /// `AUDIO_SAMPLE_RATE`. A no-op default returning an empty `Vec` for a
    /// machine with no audio hardware modeled (or none at all, like ZX81)
    /// -- same "plain method, no `Self: Sized` needed" reasoning as
    /// `render_frame`/`request_interrupt` above.
    ///
    /// Deliberately a drain, not an on-demand render: unlike
    /// `render_frame` (a snapshot of current state), audio depends on the
    /// full history of what happened since the last drain (e.g. every
    /// beeper edge), so a machine with audio must accumulate samples
    /// internally as it steps, not synthesize them after the fact.
    fn drain_audio(&mut self) -> Vec<f32> {
        Vec::new()
    }

    /// Replaces the set of I/O ports the debugger wants to be told about.
    /// Returns whether this machine has a port space to watch at all; the
    /// default (no I/O ports, e.g. the 6502 machines) says no and ignores
    /// the list.
    fn set_port_watches(&mut self, _watches: Vec<PortWatch>) -> bool {
        false
    }

    /// Takes the first port access that matched a watch since the last call
    /// (see [`PortWatcher`]). Default: never.
    fn take_port_hit(&mut self) -> Option<PortHit> {
        None
    }

    /// Serializes this machine's full live state (registers, memory,
    /// peripheral timing state) into an opaque byte blob -- a save state.
    /// Deliberately excludes externally-supplied media (ROM contents, a
    /// loaded disk image's own bytes, a loaded tape's own pulse data),
    /// exactly like `--rom` itself is never embedded in a `.z80`/`.sna`
    /// snapshot -- restoring requires the same media already attached.
    ///
    /// Each impl does its own encoding internally (bincode, currently) so
    /// this trait -- and every other machine -- stays free of any
    /// serialization-library dependency. No default: a machine with no
    /// save-state support yet should make that a compile error for callers
    /// expecting one, not a silently-empty blob.
    fn save_state(&self) -> Vec<u8>;

    /// Restores state previously produced by `save_state`. Errors clearly
    /// (not a silent best-effort) on a corrupt blob or one saved from a
    /// different machine/model -- same "must match" stance as
    /// `ZxSpectrum::load_snapshot`.
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String>;

    /// Resets this machine's CPU to its power-on state (PC back to the
    /// reset vector, interrupt flags/mode cleared, halted cleared) --
    /// the same real-world effect pressing a hardware RESET button has.
    /// Deliberately leaves RAM contents untouched: a real Z80/6502
    /// `/RESET` line only resets the CPU itself, it never clears memory,
    /// so a machine coming back up after this re-runs its ROM's boot code
    /// against whatever was already in RAM a moment ago -- same as real
    /// hardware. No default: a machine with no reset implemented yet
    /// should be a compile error for callers expecting one, not a silent
    /// no-op indistinguishable from "it worked."
    fn reset(&mut self);
}
