# CLAUDE.md

Guidance for Claude Code (claude.ai/code) working in this repo.

## What this repository is

The shared crates of retroclawd (see README.md): `machine`, `plugin-api`, the `z80`, `mos6502` and `mc68000` cores, `ay-3-8910`, `mc6850`, `nec765`, `wd1770`, `dsk`, `zx-tape`. No machine and no application code. The application (`elpapimango/retroclawd`) uses this repository as a submodule; the machine repositories pin it by git revision. The plugin ABI rules are in `docs/plugin-abi.md`: read them before changing `plugin-api`.

## Commands

- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`
- `cargo run --release -p z80 --example zexall -- zexall` (and `zexdoc`): Z80 accuracy

## Crate notes

Written in the pre-split repository; mentions of `zx-spectrum`, `c64`, `retroclawd`'s GUI and similar refer to code that now lives in the application or machine repositories.

- **`machine`** — `Machine` = per-computer trait (`clock_hz`, `frame_hz`,
  `step`, `halted`, `registers`, `read_memory`, `load`, `set_pc`, `run`,
  `render_frame`); `run` required (not default method) -- encapsulates each
  machine's own timing loop, default impl can't work through
  `Box<dyn Machine>` (object-safety). Each machine implements `run` as
  `Clock::new(self.frame_hz()).run(self, running)`; `frame_hz()` = pacing
  rate (display refresh, typically 50/60 Hz), distinct from `clock_hz()`
  (CPU speed in MHz). `Clock` = reusable pacing helper, sleeps off real time
  each clock-cycle budget finishes early by (generic "cycles", not
  Z80-specific "T-states" -- `RunStats.cycles`/`step()`'s return value named
  this way deliberately, crate is CPU-agnostic; `crates/core/z80` itself still
  says "T-states" throughout, correctly, its own precise term).
  `render_frame` (default method, returns `Option<(u32, u32, Vec<u8>)>` RGB
  pixels, default `None`) is opposite case object-safety-wise: plain `&self`
  method, no need to coerce into `&mut dyn Machine`, no `Self: Sized` bound
  needed, works fine as default -- unifies only video *output* (pixel grid)
  across machines, not generation mechanism, since each machine's video
  hardware genuinely differs (see `zx81` below vs. future VDP/CRTC-based
  machine). `with_border` (in `video.rs`) pads a `render_frame`-shaped RGB
  buffer with a solid-color border on every side -- the one piece of pixel
  compositing genuinely shared between `zx81`/`zx-spectrum` (both wrap
  their picture in a border), so it lives next to the trait that defines
  the RGB-tuple contract it operates on, not in either machine crate.
  `MemoryBus` = region-mapped ROM/RAM map (unmapped reads return
  `0xFF`, writes to ROM/unmapped space dropped, `load` returns bytes
  actually landed so truncated load can't hang CPU silently). `Bus` =
  read/write/read_io/write_io trait a CPU core drives -- only `read`/
  `write` are required; `read_io`/`write_io` are defaulted (`0xFF` read /
  no-op write, mirroring `MemoryBus`'s own unmapped-access convention)
  since a real IO-port address space is a Z80-family concept, not a
  universal CPU-bus one -- the 6502 (`mos6502`) has none at all, every
  chip is memory-mapped, so a 6502 `Bus` impl need not touch either
  method and that core's `step()` never calls them. Every existing Z80
  `Bus` impl already overrides both, so this changed nothing for them;
  it only stopped being a forced no-op for a CPU family that never
  needed the concept.
- **`mc68000`** (`crates/core/68000`, 2026-10-20) — Motorola 68000: its own bus trait `M68kBus` (byte and word cycles
  with function codes, `iack`, a `sync` hook for internal clocks, `reset_devices`), `Cpu::step` returning clocks, the
  two-word prefetch queue, all exceptions, `take_exception` for diagnostics, `disassemble`, and `opcodes::OPCODES`,
  the one pattern table decoding and disassembly share. Conformance: `tests/singlestep.rs` (SingleStepTests 68000/v1,
  fetched by `scripts/fetch_singlesteptests.sh`, never committed), `tests/exceptions.rs`, `tests/disassemble.rs`,
  `tests/differential.rs` (`--ignored`, against `m68k` 0.14.6). See CREDITS.md for the numbers.
- **`ay-3-8910` YM2149 variant (2026-10-21)** — `Variant::Ym2149` (`with_variant`, `set_variant`): Table 4 envelope
  shapes with the 5-bit level and the Fig. 1 D/A curve of Yamaha's catalog (CREDITS). The variant is configuration and
  never serialized (`#[serde(skip)]`): the Spectrum and CPC save their AY in bincode, so a new field would break their
  `.state` files. The AY path is unchanged (including its known envelope hold bug for shapes 1101/1111).
- **`mc6850`** (2026-10-21) — Motorola ACIA from the data sheet; no clock inside: the host times characters from
  `frame_bits() × divide()` and calls `start_tx`/`take_tx`/`rx_byte`. Used by the Atari ST.
- **`wd1770`** (moved from retroclawd-commodore 2026-10-21) — WD1770/1772 against a `Floppy` trait; the C64's 1581 and
  the Atari ST (WD1772) use it. Two data-sheet corrections since the move: Type I sets S5 once the spin-up phase is
  over, and Force Interrupt keeps MO (and a running command's status). Its behaviour tests are the 1581's in
  retroclawd-commodore plus `tests/type1.rs`.
- **`ay-3-8910`** — emulates the General Instrument AY-3-8910 PSG (3 tone
  channels sharing one noise generator + one envelope generator), used by
  the Spectrum 128K/+2/+3, Amstrad CPC, MSX, and many arcade boards --
  own crate for the same reuse reasoning as `crates/core/z80` (see "Shared
  peripheral chips" in `TODO.md`). No dependency on `machine`, any CPU
  crate, or an audio backend -- `select_register`/`write`/`read` is the
  host-facing register interface real hardware exposes via its BC1/BDIR
  pins (the actual port-address decode for those pins is each host
  machine's own job, e.g. `zx_spectrum::bus`'s `0xfffd`/`0xbffd`, not this
  crate's); `tick(cycles)`/`output() -> f32` drives sample generation, at
  whatever clock-divided rate the caller passes in (real AY clock is a
  per-board wiring choice -- Spectrum 128K runs it at CPU clock / 2, this
  crate doesn't assume that or any other ratio). Tone/noise/envelope
  algorithms (divide-by-8/divide-by-16 prescalers, 17-bit noise LFSR, the
  envelope shape state machine, the logarithmic 16-step volume table) are
  the chip's own well-documented internal architecture, treated as a
  trusted external reference the same way this project treats Z80
  undocumented-flag-behavior references -- not something verified against
  this codebase's own ROM disassembly the way ZX81-specific software
  behavior elsewhere was.
- **`zx-tape`** — parses `.tap`/`.tzx` tape files into a `Tape` (a flat
  sequence of EAR-line `Pulse`/`Stop` events, private `Event` enum) plus a
  `TapePlayer` that walks it in real time. No dependency on `machine`, any
  CPU crate, or `zx-spectrum` -- same reasoning as `z80-snapshot`/
  `zx81-snapshot`: this is a fact about the file format, not about how a
  machine consumes it. Unlike a snapshot, `.tap`/`.tzx` aren't a
  point-in-time dump loaded once -- real tape loading is the ROM's own
  routine bit-banging through the EAR input in real time, so this crate
  models pulse *timing*, not just pulse *content*. `.tap` stores no pulse
  timing at all (`parse_tap`) -- each raw data block is defined to always
  play back through the standard ROM protocol, so `push_standard_block`
  synthesizes it: pilot tone (2168 T-states/pulse, 8063 pulses for a
  header block/3223 for a data block, chosen from the block's own flag
  byte), sync (667/735 T-states), then each byte's bits MSB-first as
  855/1710 T-state pulse pairs -- all verified against the TZX format
  specification, not guessed (see `CREDITS.md`). `.tzx` (`parse_tzx`)
  implements the two block types that carry real pulse data for the vast
  majority of commercial releases -- Standard Speed Data Block (`0x10`,
  same synthesis as `.tap` but with an explicit per-block pause) and Turbo
  Speed Data Block (`0x11`, same algorithm with every timing parameter --
  including how many bits of the last byte are actually used -- read from
  the block itself instead of the fixed standard constants, via the
  shared `push_pulse_block`) -- plus the pause/stop-tape control block
  (`0x20`), the custom-loader blocks real `.cdt` protection schemes
  (SpeedLock) use -- Pure Tone `0x12`, Pulse Sequence `0x13`, Pure Data
  `0x14` -- and two trivial, commonly-present metadata blocks whose skip
  length this was verified against (Group Start `0x21`, Group End `0x22`),
  plus the purely informational blocks whose length framing is documented
  and which never carry pulse data -- Text Description `0x30` (1-byte
  length), Message `0x31` (1-byte duration + 1-byte length), Archive Info
  `0x32` (2-byte length), Hardware Type `0x33` (count * 3 bytes) and Glue
  `0x5A` (fixed 9 bytes) -- skipped via `skip_length_prefixed` (a real CPC
  game, `Oh Mummy.cdt`, failed to parse at all before this; framing taken
  from ep128emu's and Caprice32's own `tape.cpp`, see `CREDITS.md`). Every
  other block ID (Emulation Info `0x34`/Custom Info `0x35`, where the two
  reference parsers disagree on the layout, Direct Recording `0x15`, CSW/
  generalized-data `0x18`/`0x19`, loops `0x24`/`0x25`, `0x2A`/`0x2B`) is still a
  clear `Error::UnsupportedBlockType` rather than a guessed skip length --
  silently skipping a block that might carry real pulse data would corrupt
  loading, not just omit metadata.
  `TapePlayer::tick(cycles)`/`ear_bit() -> bool` mirror `ay-3-8910`'s
  `tick`/`output` shape; `play()` starts/resumes playback (a no-op once
  the tape's fully finished) and `set_speed_multiplier(n)` is turbo
  mode -- the *same* real pulse protocol, sped up, never a different
  (ROM-routine-trapping) mechanism, so loading-screen behavior (border
  stripes, etc.) stays correct regardless of speed. A `.tzx` pause block
  with a `0`ms duration is the format's own "stop the tape" signal (used
  by multi-load games) -- modeled as `Event::Stop`: `tick` halts playback
  there (advancing past the marker first, so a later `play()` doesn't
  immediately re-trigger the same stop) and `ear_bit()` reads idle-high
  (matching a real stopped cassette) whenever not actively playing,
  regardless of which pulse happens to be queued up next.
  `parse_pzx` implements `.pzx` v1.0's 4 mandatory blocks (`PZXT`/`PULS`/
  `DATA`/`PAUS`) plus `STOP` (see CREDITS.md) -- unlike `.tzx`, an
  unrecognized block is safely skipped via its own size prefix rather than
  an error, matching the format's own design (every block is self-
  describing this way). Deliberately doesn't populate `Tape::blocks`/
  `block_starts`/`block_ends` at all: a PZX `DATA` block's raw bytes could
  equally be a standard ROM-loadable block or an arbitrary custom loader's
  own data, with no per-block marker distinguishing the two (unlike
  `.tzx`'s separate Standard/Turbo IDs) -- `try_fast_tape_load_trap` (see
  `zx-spectrum` below) simply never engages for a `.pzx`-sourced tape.
  `.tap`/`.tzx` still populate those fields for the trap to use, tracked
  by matching `events` index ranges (`block_starts`/`block_ends`) since a
  `.tzx` Turbo Speed Data Block can sit between two standard blocks
  without appearing in `blocks` itself.
  `parse_csw` implements `.csw` (Compressed Square Wave) v1 (16-bit
  sample rate, always-RLE) and v2 (32-bit sample rate, RLE or Z-RLE --
  RLE further zlib-compressed) header layouts, per the format's own home
  page (see CREDITS.md) -- a literal sampled waveform, not a
  block-structured protocol like `.tap`/`.tzx`/`.pzx`: one byte per pulse
  (its duration in samples at the header's own sample rate), `0x00`
  escaping to a 4-byte little-endian extended duration for anything
  needing more than 255 samples. New `flate2` dependency (pure-Rust
  `miniz_oxide` backend, no new C toolchain need) does the Z-RLE
  zlib-inflate step; RLE-only data (`compression_type == 1`) skips that
  step entirely. Each decoded sample-count converts to real Z80 T-states
  via the same PAL clock literal (`CSW_CLOCK_HZ` = `3_546_900`)
  `ms_to_cycles`/`DEFAULT_PAUSE_CYCLES` already use elsewhere in this
  file. Same "doesn't populate `Tape::blocks`/`block_starts`/
  `block_ends`" choice as `.pzx`, same reason: a raw pulse-duration
  stream has no marker distinguishing standard-loadable data from a
  custom loader's own waveform, so `try_fast_tape_load_trap` never
  engages for a `.csw`-sourced tape either -- always real-time pulse
  playback, correct if less accelerated.
- **`mos6502`** (`crates/core/6502`) — full NMOS 6502 core (`Cpu`,
  `crate::opcodes`' single flat 256-entry dispatch `match`, no prefix
  tables needed unlike `z80` since every opcode is one byte), depends on
  `machine` for `Bus` alone, same shape as `z80`. `Bus::read_io`/
  `write_io` are defaulted specifically so this core never has to touch
  them -- the 6502 has no separate IO-port space at all, everything is
  memory-mapped, so `mos6502::Cpu::step` only ever calls `read`/`write`.
  Full documented instruction set + all addressing modes + real NMOS
  quirks (page-cross extra cycle on reads only, never on writes/RMW;
  `JMP ($xxFF)`'s indirect page-wrap bug; `JSR`/`RTS`'s real off-by-one
  push/pop; `BRK` vs. IRQ/NMI's differing stacked-`B`-flag convention) +
  the full illegal/undocumented opcode set (`LAX`/`SAX`/`SLO`/`RLA`/
  `SRE`/`RRA`/`DCP`/`ISC`/`ANC`/`ALR`/`ARR`/`SBX`/`XAA`, the "highly
  unstable" `SHY`/`SHX`/`SHA`/`TAS`/`LAS` group, illegal `NOP` widths,
  all 12 `JAM` opcodes) -- in scope from day one, not deferred the way
  `z80`'s undocumented flag-bit work originally was (see `TODO.md`'s
  "Commodore 64 support" section for why). `Cpu::reset`/`request_irq`/
  `request_nmi` mirror `z80::Cpu`'s own interrupt API shape. `set_overflow`
  (added for the 1541 GCR work, see `c1541`'s own bullet below) models the
  real, dedicated SO input pin every 6502 datasheet documents -- sets the
  V flag immediately, with no interrupt-enable gating or vectoring at all,
  unlike `request_irq`/`request_nmi`. Validated by
  focused unit tests plus Klaus Dormann's documented-opcode functional
  test and Tom Seddon's exhaustive NMOS decimal/illegal-opcode suite
  (vendored with matching source/license/provenance under
  `crates/core/6502/tests/fixtures/`). The Seddon suite found and fixed
  decimal `ADC`'s partial-adjustment N/V behavior and decimal `ARR`'s
  nibble corrections. The genuinely chip-dependent unstable opcode group
  remains a deterministic common-case approximation, not a claim of one
  hardware-universal result.
  **Disassembler/assembler (`disassemble.rs`, 2026-10-12)**: `disassemble(bytes, addr) -> (text, len)` and
  `assemble_one(text, addr) -> Result<bytes>` for the debugger, the counterpart of `z80::disassemble`: all 256
  opcodes decode (undocumented ones print with a `*`, e.g. `*LAX $10`, `*JAM`), branches print their absolute
  target, the assembler takes the same syntax (zero-page vs absolute by operand width, documented opcode
  preferred over an undocumented twin). The table is checked against the core: for every non-control-flow
  opcode the decoded length equals how far `Cpu::step` moves the program counter.
  **Cycle-complete bus events (2026-10-12)**: every 6502 cycle is exactly one
  `Bus::read`, `Bus::write` or `Bus::dummy_cycle(addr, write, value)` (a new
  defaulted `machine::Bus` hook, no-op and inlined away unless a bus overrides
  it), in real order: the implied/accumulator padding read of the byte after
  the opcode, the indexed fix-up read at the *uncorrected* address (always for
  stores/RMW, only on a page cross for reads), the unindexed read of `zp,X`/
  `zp,Y`/`(zp,X)`, the RMW write-back of the old value before the new one,
  `PHA`/`PLA`/`JSR`/`RTS`/`RTI`/`BRK` stack and padding reads, a taken branch's
  wasted read (a second one across a page), the two throwaway reads of an
  IRQ/NMI, and JAM. `JSR` now fetches its high operand byte after pushing, as
  the chip does. Dummy reads reach the bus only through `dummy_cycle`, so they
  have no device side effects unless the bus chooses to give them some. Purpose:
  a cycle-accurate bus (the planned C64 `vic_accuracy = cycle`, see `TODO.md`)
  can tick the VIC once per event and stall on read cycles. Guard:
  `tests/cycle_hook.rs` -- all 256 opcodes in 16 operand/flag scenarios emit as
  many events as the cycles `step` reports, plus exact event orders for the
  tricky families; Klaus Dormann and Seddon are unchanged.
