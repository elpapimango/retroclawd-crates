# Credits

Third-party references this codebase's implementation is based on -- none
of this code is copied from these sources; each is a fresh Rust
implementation of documented behavior, cited here because that behavior
(file layouts, byte offsets, register constants) came from reading them,
not from independent derivation.


> Sources for the machine families (ZX81/Spectrum, CPC, C64/C128 and their chips and formats)
> are credited in `CREDITS.md` of `retroclawd-sinclair`, `retroclawd-amstrad` and
> `retroclawd-commodore`.

## MOS 6520 PIA (`crates/chips/pia-6520`)

Register decoding, direction selection, asymmetric port A/B readback,
edge-latched interrupt status, C1/C2 interrupt enable/polarity, C2
handshake/pulse/manual outputs and reset behavior: Avery Lee's
[Altirra Hardware Reference Manual](https://www.virtualdub.org/downloads/Altirra%20Hardware%20Reference%20Manual.pdf),
2026-01-02, sections 2.5 and 14.5, pages 34-36 and 453-456.
This is a fresh implementation of documented hardware behavior, not a
translation of an emulator. Machine-specific wiring, analogue floating-input
decay and output-to-input spurious C2 interrupts are outside this implementation.

## AY-3-8910 chip emulation (`crates/ay-3-8910`)

The tone/noise/envelope generator algorithms and the 16-step logarithmic
volume table are the AY-3-8910's own well-documented, widely-published
internal architecture (its datasheet's own described behavior, echoed
consistently across many independent emulator implementations) -- not
attributed to one specific source the way the file-format work above is,
since this is closer to "how the chip works" than "one project's specific
choice."

## ZX Spectrum tape formats (`crates/zx-tape`)

Standard ROM tape-loading pulse timings (pilot pulse 2168 T-states, 8063
pulses for a header block/3223 for a data block, sync pulses 667/735
T-states, bit-0/bit-1 as paired 855/1710 T-state pulses) -- the TZX format
specification, as mirrored at
<https://k1.spdns.de/Develop/Projects/zasm/Documentation/z103.htm>
(`.tap` has no pulse timing of its own; it's defined to always play back
through this same standard protocol, which the TZX spec documents
explicitly for its own "Standard Speed Data Block", ID `0x10`).

Port `0xFE` EAR input (bit 6) and MIC/speaker output (bits 3/4) bit
layout: the World of Spectrum FAQ, "48K reference"
<https://worldofspectrum.org/faq/reference/48kreference.htm>.

`.pzx` format (block structure, `PZXT`/`PULS`/`DATA`/`PAUS`/`STOP` block
layouts, the `PULS` repeat-count/extended-duration decoding pseudocode) --
"PZX (Perfect ZX Tape) file format version 1.0" by Patrik Rak, as mirrored
at <https://github.com/kounch/pzxtools/blob/main/docs/pzx_format.txt>.

`.csw` (Compressed Square Wave) format (v1/v2 header layouts, byte
offsets, the RLE pulse-byte encoding + `0x00`-escape extended-duration
form, the `0x02`=Z-RLE compression type meaning "RLE further compressed
with zlib"), per the format's own home page
<https://k1.spdns.de/Develop/Projects/zasm/Documentation/csw.html>.

## NEC765/uPD765A floppy disk controller (`crates/nec765`)

The real uPD765 datasheet's command-phase and result-phase byte tables
(NEC, December 1978) -- OCR text via Internet Archive,
<https://archive.org/stream/bitsavers_necdatasheec78_1042541/uPD765_Data_Sheet_Dec78_djvu.txt>
-- every command's parameter-byte order/count and result-byte order/
meaning (Read Data, Recalibrate, Seek, Sense Interrupt Status, Specify,
Read ID) comes from this document, fetched directly rather than
recalled. Port addresses `0x2ffd`/`0x3ffd` and their `0xF002`-family
decode mask are this project's own assumption by analogy with `0x1ffd`'s
already-manual-verified mask (see the +2A/+3 paging entry above) -- NOT
independently verified against the official +3 manual or a hardware test
suite, unlike that entry.

## CPCEMU `.dsk` disk image format (`crates/dsk`)

cpctech.cpcwiki.de's own DSK format reference,
<https://cpctech.cpcwiki.de/docs/dsk.html> -- the Disc Information
block's and Track Information block's exact byte offsets/sizes, the
`"MV - CPCEMU Disk-File\r\nDisk-Info\r\n"`/`"Track-Info\r\n"` signature
strings verbatim, and the 8-byte Sector Information List entry layout
all come from this page, fetched directly rather than recalled.

Extended DSK support (the `"EXTENDED CPC DSK File\r\nDisk-Info\r\n"`
signature, the per-track size table replacing the standard format's
single global track size, `0` meaning an unformatted/not-present track,
and the Sector Information List's "actual data length" override at bytes
6-7 for weak/copy-protected sectors) -- cross-checked against
`AmstradDSKExplorer`'s own header-parsing code
<https://github.com/muckypaws/AmstradDSKExplorer/blob/master/DSKInfoV3.py>
and general-web corroboration of the "actual data length" field's
purpose, since cpctech.cpcwiki.de's own Extended DSK page
(<https://cpctech.cpcwiki.de/docs/extdsk.html>) wasn't directly fetchable
in this environment.

## 6502 CPU core (`crates/core/6502`, package `mos6502`)

General instruction semantics, addressing modes, and cycle counts are the
6502's own well-documented, widely-published architecture (echoed
consistently across many independent references) -- same "how the chip
works" epistemic tier as the AY-3-8910 entry above, not attributed to one
specific source.

The core is validated against two independent GPL-3.0-or-later test
programs vendored with matching source, license text, revision, and
checksums under `crates/core/6502/tests/fixtures/`:

- Klaus Dormann's `6502_functional_test`, official 2017 prebuilt binary
  and matching source from
  <https://github.com/Klaus2m5/6502_65C02_functional_tests>, validates
  every documented NMOS opcode and addressing mode. That suite explicitly
  ignores decimal-mode N/V/Z and does not exercise illegal opcodes.
- Tom Seddon's NMOS/illegal-opcode suite,
  <https://github.com/tom-seddon/6502-tests>, derives its coverage from
  Wolfgang Lorenz's tests, exhaustively checks valid and invalid BCD inputs
  plus stable illegal opcodes, and was checked by its author on real BBC
  Micro NMOS hardware.

Both pass. The Seddon suite exposed two real errors in the initial core:
decimal `ADC` incorrectly derived N/V from the unadjusted binary result
instead of the partially BCD-adjusted value, and decimal-mode `ARR` omitted
the NMOS nibble corrections entirely. The corrected behavior follows that
suite's independently hardware-checked model; focused regressions preserve
the exact failures it found.

The chip-dependent unstable group (`SHY`/`SHX`/`SHA`/`TAS`/`LAS`/`XAA`)
is not covered by a single hardware-universal expected result. Those
opcodes retain the deterministic common-case formulas from the widely
cited "NMOS 6510 Unintended Opcodes" reference by Groepaz/Hitmen. This is
useful deterministic emulation, not a claim that temperature- and
chip-dependent silicon behavior has one exact answer.

Acid800 1.2 (Avery Lee, MIT; privately executed, not committed) exposed an
incorrect SHY page-crossing store. SHY/SHX/SHA/TAS now mask with the
**unindexed** base high byte plus one and replace the destination high byte
with the masked data when indexing carries. Existing indexed dummy reads
and cycle counts are preserved. Sources: Altirra Hardware Reference
2026-01-02, chapter 3, and Atari800 7.2.1's independently implemented
[`src/cpu.c`](https://github.com/atari800/atari800/blob/63a381884175e3cbfc745ff00dfb3d70d928eb50/src/cpu.c)
opcodes `$93/$9B/$9C/$9E/$9F`. The latter explicitly selects high-byte-plus-one
for SHA; the manual describes variant SHA masking and warns of instability.
This is a specified deterministic common case, not a universal silicon claim.
The regression covers the actual Acid800 SHY failure, all five opcodes,
non-crossing and crossing stores, zero-page pointer wrap, 16-bit address
wrap, unchanged flags/registers, TAS's stack update and exact final bus events.

Acid800's CLI/SEI timing test also exposed the live-I interrupt check.
The Core now retains I from before CLI, SEI and PLP for the next boundary;
RTI's restored I applies immediately. Source: the same Altirra Hardware
Reference, chapter 3, "Clearing/Setting the I flag with an interrupt pending".
Focused bus tests cover delayed IRQ entry, pushed I, RTI and latch cleanup
on reset, NMI and debugger status edits. No bus event or cycle is added.
This fixes instruction-boundary mask sampling, not the full cycle-exact
IRQ/NMI/BRK pipeline or taken-branch interrupt delay.

The published `Cpu` serde bytes remain unchanged. Exact snapshots must store
`Cpu::irq_poll_mask()` separately in their versioned machine envelope and
restore it with `restore_irq_poll_mask`. Legacy states restore `None`,
preserving their live-I boundary semantics because they contain no history.
Atari 800 v3, VIC-20 v5 and the C64/C128/1541/1581 IRQ envelopes persist this
latch, including nested drive checkpoints. Preserve old state files for
rollback; old plugins cannot read the new machine envelopes.

BRK/IRQ vector selection now permits a cycle-clocked Bus NMI edge to hijack
the sequence after cycle 4, before the status push. Sources:
[NESdev CPU interrupts, "Interrupt hijacking"](https://www.nesdev.org/wiki/CPU_interrupts#Interrupt_hijacking)
(including the Visual6502-derived tick sequence), and Altirra Hardware
Reference chapter 3, "Overlapping interrupts". Both stack frames and seven
bus cycles remain unchanged: BRK still pushes PC+2 and B=1; IRQ pushes the
interrupted PC and B=0. Tests place the edge after each of the seven cycles,
checking vector reads, stack contents and retention of late edges.
`Bus::take_nmi_edge` defaults to false, so existing instruction-boundary
callers retain their behavior. No CPU field, serde layout or Plugin ABI
changes. A connected Bus owns and persists unconsumed edges; Atari's existing
Hardware State already does so. This is not a complete interrupt pipeline:
consecutive-interrupt inhibition, detector latency and taken-branch timing
remain outside this increment.

## uPD765A behaviour used by real CPC/+3 firmware (`crates/chips/nec765`, 2026-09-29)

Where the datasheet leaves open what real CPC/+3 software depends on, the
controller follows Caprice32's `src/fdc.cpp` (Ulrich Doewich / Colin Pitrat,
GPL-2.0-or-later), shallow-cloned and read directly rather than summarised:

- Command table and parameter counts (`fdc_cmd_table`), including that the
  flag bits are part of the opcode (`0x46` Read Data + MFM, `0x66` with SK).
- Main status register values per phase (`fdc_read_status`): idle `0x80`,
  receiving parameters `0x90`, execution `0x80|0x30` plus `0x40` when the
  controller is sending, result `0xD0`, and the single not-ready poll after
  an execution phase begins (`read_status_delay`).
- Termination without a terminal-count line (`LOAD_RESULT_WITH_STATUS`):
  ST0 `0x40` + ST1 `0x80` ("end of cylinder") is the normal success result
  on a CPC/+3, cleared when a real error bit is set, and a bare deleted-data
  control mark is not an error.
- Sector search (`find_sector`): match all of CHRN, walk the track from the
  head's rotational position, stop after the index hole passes twice, and
  record Bad Cylinder (`0xFF` C) / Wrong Cylinder in ST2.
- Sense Drive Status (`fdc_drvstat`) ST3 composition, Sense Interrupt Status
  (`fdc_intstat`), Read ID rotation, Format Track (`fdc_writeID`) and its
  result, Write Data/Write Deleted Data (`cmd_write`), and the write-protect
  "not writable" (ST1 `0x02`) result.

Deliberately *not* copied: Caprice32's overrun timeouts and per-sector weak
read keying (this crate uses instant execution and one rolling counter), and
its `&=` clobber of ST2 in the read path (an apparent slip -- the running
ST2 flags are kept here). The `.dsk` write-back layout follows the CPCEMU
standard/Extended DSK references already cited under "CPCEMU `.dsk` disk
image format": Track Information block fields 0x10-0x17, sector-info bytes
4/5 as ST1/ST2, Extended per-track size table in 256-byte units.



## MC68000 core (`crates/core/68000`, package `mc68000`)

Written from the manuals; no code, table or comment of another core is copied.

- *M68000 8-/16-/32-Bit Microprocessors User's Manual*, ninth edition (Motorola, 1993; M68000UM/AD): exception
  processing (section 6), bus cycles and function codes (section 5), instruction clock counts (section 8). Copy used:
  https://cdn.hackaday.io/files/1805367724052224/M68000UM_AD_M68000_Microprocessor_Users_Manual_Rev8_1993.pdf
- *M68000 Family Programmer's Reference Manual* (M68000PM/AD): instruction encodings and condition codes.
- SingleStepTests/680x0 (https://github.com/SingleStepTests/680x0), commit
  `e0d5ece9670205cc84a0101081837deb446f86a3`, `68000/v1`: the conformance gate (`tests/singlestep.rs`). The
  repository states no licence, so it is downloaded at test time by `scripts/fetch_singlesteptests.sh` and never
  committed. 996 322 of 1 000 060 cases match exactly (state, prefetch queue, RAM, clock count); 3 736 are ASR by a
  register count larger than the operand size on a negative operand, where the data clears C and X and the PRM gives
  the sign bit (kept to the PRM, allowed by a narrow rule in the harness); 2 are bad data reported upstream
  (https://github.com/SingleStepTests/680x0/issues/4). Where the manuals leave flags undefined (BCD N/V, CHK, DIV
  overflow) or give no bus order around a fault, the core follows this data.
- `m68k` 0.14.6 (https://github.com/benletchford/m68k-rs, MIT): a dev-dependency of `tests/differential.rs` only,
  an independent oracle for random instructions. It is never linked into the core or a machine.
