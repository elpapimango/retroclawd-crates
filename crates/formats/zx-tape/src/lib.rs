/// One EAR-line pulse: how long it lasts (in real Z80 T-states) and what
/// level it holds for that duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pulse {
    pub cycles: u32,
    pub level: bool,
}

/// One event in a parsed tape's timeline -- either a timed pulse, or a
/// "stop the tape" control point (from a `.tzx` pause/stop block with a
/// zero duration): playback halts there until `TapePlayer::play` is
/// called again, matching a real cassette needing Play pressed again for
/// a multi-load game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    Pulse(Pulse),
    Stop,
}

/// A fully parsed tape: every EAR-line event, in order, from the moment
/// playback starts to end-of-tape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tape {
    events: Vec<Event>,
    /// Raw bytes (flag byte + payload + checksum) of each *standard-
    /// protocol* data block, in file order, alongside `events`' pulse-level
    /// encoding of the same data -- lets `TapePlayer::take_next_raw_block`
    /// hand a ROM fast-load trap the finished bytes directly instead of
    /// making it wait for real-time pulses. Only standard-protocol blocks
    /// (`.tap`'s own blocks, `.tzx`'s Standard Speed Data Block `0x10`)
    /// appear here -- a `.tzx` Turbo Speed Data Block never does, since by
    /// definition it's read by a custom (non-ROM) loader the standard
    /// `LD-BYTES` trap has no calling-convention knowledge of.
    blocks: Vec<Vec<u8>>,
    /// `events` index each `blocks[i]`'s own pulse sequence starts at --
    /// parallel to `blocks`, used to fast-forward `TapePlayer`'s real-time
    /// playback position past a block consumed via `take_next_raw_block`.
    block_starts: Vec<usize>,
    /// `events` index each `blocks[i]`'s own pulse sequence ends at
    /// (exclusive) -- parallel to `blocks`/`block_starts`. Needed
    /// separately from "the next block's own start": a `.tzx` Turbo Speed
    /// Data Block can sit between two standard blocks without appearing in
    /// `blocks` at all, so "before the next standard block starts" would
    /// wrongly include that turbo block's own pulse range too.
    block_ends: Vec<usize>,
}

impl Tape {
    /// Builds a tape from recorded pulses -- the capture side of a machine's
    /// cassette output line. `cycles` are real Z80-domain T-states (the
    /// 3.5469 MHz clock every other duration in this crate uses); `level`
    /// is the line level the pulse holds. Zero-length pulses are dropped.
    pub fn from_pulses(pulses: impl IntoIterator<Item = Pulse>) -> Self {
        Self {
            events: pulses
                .into_iter()
                .filter(|pulse| pulse.cycles > 0)
                .map(Event::Pulse)
                .collect(),
            ..Self::default()
        }
    }

    /// Returns this tape preceded by `ms` of low-level silence, unless it
    /// already opens with a low pause at least that long (or `ms` is 0).
    ///
    /// A cassette image made by a tape-capture tool often starts straight on
    /// the first block's pilot tone, because the blank lead-in of the real tape
    /// was trimmed. A real CPC's firmware (and CPCEC, whose
    /// `tape_safetydelay` opens every TZX/CDT with a 2000 ms hold for exactly
    /// this reason) spends about 2.4 seconds after switching the motor on
    /// before it starts sampling, so a pilot shorter than that is mostly gone
    /// and the block is missed. `block_starts`/`block_ends` shift by the one
    /// inserted event so a fast-load trap still finds the same blocks.
    #[must_use]
    pub fn with_leading_silence(mut self, ms: u32) -> Self {
        if ms == 0 {
            return self;
        }
        let wanted = (u64::from(ms) * 3_546_900 / 1000).min(u64::from(u32::MAX)) as u32;
        if let Some(Event::Pulse(first)) = self.events.first()
            && !first.level
            && first.cycles >= wanted
        {
            return self;
        }
        self.events.insert(
            0,
            Event::Pulse(Pulse {
                cycles: wanted,
                level: false,
            }),
        );
        for start in &mut self.block_starts {
            *start += 1;
        }
        for end in &mut self.block_ends {
            *end += 1;
        }
        self
    }

    /// This tape with every pulse length converted from a `from_hz` T-state clock to a `to_hz` one (rounded, never below
    /// one cycle). The parsers give lengths in the Spectrum's domain -- `.tzx` and `.pzx` count T-states of the 3.5 MHz
    /// Spectrum, `.csw` and `.voc` turn seconds into cycles at 3.5469 MHz -- so a machine with another clock, like the
    /// ZX81's 3.25 MHz, rescales the tape to hear the signal at its own pace: `tape.rescaled(3_500_000, 3_250_000)`.
    /// Stop points and the raw blocks kept for fast-load traps are unchanged.
    #[must_use]
    pub fn rescaled(&self, from_hz: u64, to_hz: u64) -> Tape {
        assert!(from_hz > 0, "the source clock must be non-zero");
        let events = self
            .events
            .iter()
            .map(|event| match event {
                Event::Pulse(pulse) => {
                    let cycles = (u64::from(pulse.cycles) * to_hz + from_hz / 2) / from_hz;
                    Event::Pulse(Pulse {
                        cycles: cycles.clamp(1, u64::from(u32::MAX)) as u32,
                        level: pulse.level,
                    })
                }
                Event::Stop => Event::Stop,
            })
            .collect();
        Tape {
            events,
            ..self.clone()
        }
    }

    /// Total playing time of every pulse, in Z80 T-states.
    pub fn total_cycles(&self) -> u64 {
        self.events
            .iter()
            .map(|event| match event {
                Event::Pulse(pulse) => u64::from(pulse.cycles),
                Event::Stop => 0,
            })
            .sum()
    }

    /// Total pulse/control events in this parsed tape timeline.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Number of standard-protocol data blocks (`.tap` blocks and `.tzx`
    /// Standard Speed Data Blocks) available to `take_next_raw_block`.
    pub fn standard_block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Access all events in this tape -- used internally by `write_voc`;
    /// `Event` itself stays crate-private (external consumers use
    /// `TapePlayer` instead), so this accessor does too.
    pub(crate) fn events(&self) -> &[Event] {
        &self.events
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TruncatedBlock,
    NotATzxFile,
    UnsupportedBlockType(u8),
    /// `.pzx` only: the file didn't start with the mandatory `PZXT` header
    /// block.
    NotAPzxFile,
    /// `.csw` only: the file didn't start with the mandatory 22-byte
    /// "Compressed Square Wave" signature + `0x1A` terminator.
    NotACswFile,
    /// `.csw` only: major version byte wasn't `1` or `2` -- the only two
    /// versions this project found a real spec for (see `CREDITS.md`).
    UnsupportedCswVersion(u8),
    /// `.csw` only: compression type byte wasn't `1` (RLE) or `2` (Z-RLE,
    /// v2 only).
    UnsupportedCswCompression(u8),
    /// `.csw` v2 only, compression type `2` (Z-RLE): the zlib stream
    /// itself failed to decompress (corrupt file, or a genuinely
    /// unsupported zlib feature) -- never silently produces garbage
    /// pulses from a partially-decompressed stream.
    CswInflateFailed,
    /// `.voc` only: the file didn't start with the mandatory 26-byte
    /// header including "Creative Voice File\x1A" signature.
    NotAVocFile,
    /// `.voc` only: encountered a codec (audio format) this project doesn't
    /// support (only 8-bit unsigned PCM, codec 0x00, is supported).
    UnsupportedVocCodec(u8),
    /// `.voc` only: encountered an unknown/unsupported VOC block type ID.
    UnsupportedVocBlock(u8),
    /// `.voc` only: a Repeat Start block specified count 0xFFFF, which
    /// would require infinite unrolling -- explicitly rejected.
    VocEndlessRepeat,
    /// `.wav` only: not a RIFF `WAVE` file, or it has no `fmt ` or `data` chunk.
    NotAWavFile,
    /// `.wav` only: a sample format other than integer PCM (8, 16, 24 or 32 bit) or IEEE float (32 or 64 bit): the
    /// format tag and the bits per sample.
    UnsupportedWavFormat {
        tag: u16,
        bits: u16,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TruncatedBlock => write!(f, "tape file truncated mid-block"),
            Error::NotATzxFile => write!(f, "not a .tzx file (missing 'ZXTape!' signature)"),
            Error::UnsupportedBlockType(id) => {
                write!(f, "unsupported .tzx block type 0x{id:02X}")
            }
            Error::NotAPzxFile => write!(f, "not a .pzx file (missing 'PZXT' header block)"),
            Error::NotACswFile => write!(
                f,
                "not a .csw file (missing 'Compressed Square Wave' signature)"
            ),
            Error::UnsupportedCswVersion(major) => {
                write!(
                    f,
                    "unsupported .csw major version {major} (expected 1 or 2)"
                )
            }
            Error::UnsupportedCswCompression(kind) => {
                write!(
                    f,
                    "unsupported .csw compression type {kind} (expected 1=RLE or 2=Z-RLE)"
                )
            }
            Error::CswInflateFailed => write!(f, ".csw Z-RLE (zlib) data failed to decompress"),
            Error::NotAWavFile => write!(
                f,
                "not a .wav file (no RIFF/WAVE header, or no fmt/data chunk)"
            ),
            Error::UnsupportedWavFormat { tag, bits } => write!(
                f,
                "unsupported .wav sample format (tag {tag}, {bits} bits): only PCM 8/16/24/32-bit and float 32/64-bit"
            ),
            Error::NotAVocFile => write!(
                f,
                "not a .voc file (missing or invalid Creative Voice File header)"
            ),
            Error::UnsupportedVocCodec(codec) => {
                write!(
                    f,
                    "unsupported .voc codec 0x{codec:02X} (only 0x00 = 8-bit unsigned PCM supported)"
                )
            }
            Error::UnsupportedVocBlock(id) => write!(f, "unsupported .voc block type 0x{id:02X}"),
            Error::VocEndlessRepeat => {
                write!(f, ".voc endless repeat (count=0xFFFF) not supported")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Standard ROM tape-loading protocol constants, in real Z80 T-states --
/// verified against the TZX format specification, see CREDITS.md.
const PILOT_PULSE: u32 = 2168;
const PILOT_PULSES_HEADER: u32 = 8063;
const PILOT_PULSES_DATA: u32 = 3223;
const SYNC1: u32 = 667;
const SYNC2: u32 = 735;
const BIT0_PULSE: u32 = 855;
const BIT1_PULSE: u32 = 1710;

/// Default pause (idle level) between blocks when the source doesn't
/// specify one, in real Z80 T-states at the Spectrum's ~3.5MHz PAL clock
/// (matching `zx_spectrum::ZxSpectrum::clock_hz`'s own PAL constant) --
/// the conventional 1-second gap most tools/emulators use for `.tap`
/// (which stores no pause value at all). Not format-critical: any
/// reasonably long idle gap works, since its only job is giving the ROM's
/// loader a clear silence between one block's data and the next pilot
/// tone.
const DEFAULT_PAUSE_CYCLES: u32 = 3_546_900; // 1000ms

fn read_u16_checked(bytes: &[u8], pos: usize) -> Result<u16, Error> {
    let slice = bytes.get(pos..pos + 2).ok_or(Error::TruncatedBlock)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_slice_checked(bytes: &[u8], pos: usize, len: usize) -> Result<&[u8], Error> {
    bytes.get(pos..pos + len).ok_or(Error::TruncatedBlock)
}

fn read_u24_checked(bytes: &[u8], pos: usize) -> Result<u32, Error> {
    let slice = bytes.get(pos..pos + 3).ok_or(Error::TruncatedBlock)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], 0]))
}

fn read_u32_checked(bytes: &[u8], pos: usize) -> Result<u32, Error> {
    let slice = bytes.get(pos..pos + 4).ok_or(Error::TruncatedBlock)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Converts a `.tzx` pause value (milliseconds) to real Z80 T-states, at
/// the same PAL clock rate `DEFAULT_PAUSE_CYCLES` uses.
fn ms_to_cycles(ms: u16) -> u32 {
    (ms as u64 * 3_546_900 / 1000) as u32
}

/// Synthesizes one block's pulse sequence: `pilot_count` pulses of
/// `pilot_cycles`, then a `sync1_cycles`/`sync2_cycles` pulse pair, then
/// each byte of `data` bit-encoded MSB-first as `bit0_cycles`/
/// `bit1_cycles` pulse pairs -- shared by both the standard protocol
/// (`.tap`, and `.tzx`'s Standard Speed Data Block) and `.tzx`'s Turbo
/// Speed Data Block, which is the exact same algorithm with every timing
/// value taken from the block itself instead of the fixed standard
/// constants. `used_bits_in_last_byte` trims how many of the final byte's
/// bits are actually encoded (`0` means all 8 bits, per the TZX spec;
/// explicit values 1-8 trim accordingly) -- always 8 for the standard
/// protocol, which has no such concept; `.tzx` turbo blocks can specify
/// fewer. `level` is the current line level entering this block and is
/// updated to the level after this block (including any trailing pause).
#[allow(clippy::too_many_arguments)]
fn push_pulse_block(
    events: &mut Vec<Event>,
    data: &[u8],
    pilot_cycles: u32,
    pilot_count: u32,
    sync1_cycles: u32,
    sync2_cycles: u32,
    bit0_cycles: u32,
    bit1_cycles: u32,
    used_bits_in_last_byte: u8,
    pause_cycles: u32,
    level: &mut bool,
) {
    for _ in 0..pilot_count {
        events.push(Event::Pulse(Pulse {
            cycles: pilot_cycles,
            level: *level,
        }));
        *level = !*level;
    }

    events.push(Event::Pulse(Pulse {
        cycles: sync1_cycles,
        level: *level,
    }));
    *level = !*level;
    events.push(Event::Pulse(Pulse {
        cycles: sync2_cycles,
        level: *level,
    }));
    *level = !*level;

    push_data_bit_pairs(
        events,
        data,
        bit0_cycles,
        bit1_cycles,
        used_bits_in_last_byte,
        level,
    );

    if pause_cycles != 0 {
        push_pause(events, pause_cycles, level);
    }
}

/// Appends a pause (silence) of `pause_cycles` after whatever pulse was
/// played last. TZX silence is the low level, but the last pulse before it
/// still has to be *terminated* by an edge or a loader that measures it
/// (every cassette loader measures each half-bit between two edges) never
/// sees it end: when the last pulse was low there is no edge into low
/// silence. So, like Caprice32 (`Tape_UpdateLevel`'s "start with a 1ms level
/// opposite to the one last played") and CPCEC (`tape_tails`, the first
/// millisecond of a tail toggles the line), the pause begins with up to 1 ms
/// at the opposite (high) level before dropping to silence. Arkanoid's
/// SpeedLock blocks need it: three of the five end on a low pulse and the
/// loader failed its last byte without the edge. `level` is the level the
/// *next* pulse would have, i.e. the opposite of the last pulse's.
fn push_pause(events: &mut Vec<Event>, pause_cycles: u32, level: &mut bool) {
    if *level {
        let edge = pause_cycles.min(ms_to_cycles(1));
        events.push(Event::Pulse(Pulse {
            cycles: edge,
            level: true,
        }));
        if pause_cycles > edge {
            events.push(Event::Pulse(Pulse {
                cycles: pause_cycles - edge,
                level: false,
            }));
        }
    } else {
        events.push(Event::Pulse(Pulse {
            cycles: pause_cycles,
            level: false,
        }));
    }
    *level = false;
}

/// Emits one pulse pair per encoded bit from `data`, toggling `level`
/// between each pulse. Shared by `.tzx` Turbo Speed Data (`0x11`) and
/// Pure Data (`0x14`) blocks.
fn push_data_bit_pairs(
    events: &mut Vec<Event>,
    data: &[u8],
    bit0_cycles: u32,
    bit1_cycles: u32,
    used_bits_in_last_byte: u8,
    level: &mut bool,
) {
    let used_bits_in_last_byte = match used_bits_in_last_byte {
        0 => 8,
        1..=8 => used_bits_in_last_byte,
        _ => 8,
    };

    let last_byte_index = data.len().saturating_sub(1);
    for (byte_index, &byte) in data.iter().enumerate() {
        let bits = if byte_index == last_byte_index {
            used_bits_in_last_byte
        } else {
            8
        };
        for bit_index in 0..bits {
            let bit = (byte >> (7 - bit_index)) & 1;
            let pulse_cycles = if bit == 0 { bit0_cycles } else { bit1_cycles };
            events.push(Event::Pulse(Pulse {
                cycles: pulse_cycles,
                level: *level,
            }));
            *level = !*level;
            events.push(Event::Pulse(Pulse {
                cycles: pulse_cycles,
                level: *level,
            }));
            *level = !*level;
        }
    }
}

/// Synthesizes the standard ROM loading pulse sequence for one block's
/// raw bytes (`data[0]` is the flag byte; the header-vs-data pilot-tone
/// pulse count is chosen from it, per the verified standard protocol --
/// see CREDITS.md). `level` carries line-state continuity across blocks.
fn push_standard_block(
    events: &mut Vec<Event>,
    blocks: &mut Vec<Vec<u8>>,
    block_starts: &mut Vec<usize>,
    block_ends: &mut Vec<usize>,
    data: &[u8],
    pause_cycles: u32,
    level: &mut bool,
) {
    block_starts.push(events.len());
    blocks.push(data.to_vec());

    let pilot_count = if data.first().copied().unwrap_or(0) < 128 {
        PILOT_PULSES_HEADER
    } else {
        PILOT_PULSES_DATA
    };
    push_pulse_block(
        events,
        data,
        PILOT_PULSE,
        pilot_count,
        SYNC1,
        SYNC2,
        BIT0_PULSE,
        BIT1_PULSE,
        8,
        pause_cycles,
        level,
    );

    block_ends.push(events.len());
}

/// Parses a `.tap` file: a flat sequence of `[len_lo, len_hi, data...]`
/// blocks, each played back through the standard ROM loading protocol
/// (`.tap` itself stores no pulse timing -- that's the whole point of the
/// format, it's *always* the standard protocol). `len` counts `data`'s
/// own bytes (flag byte + payload + checksum together), not including
/// the 2-byte length prefix itself.
pub fn parse_tap(bytes: &[u8]) -> Result<Tape, Error> {
    let mut events = Vec::new();
    let mut blocks = Vec::new();
    let mut block_starts = Vec::new();
    let mut block_ends = Vec::new();
    let mut level = false;
    let mut pos = 0;
    while pos < bytes.len() {
        let len = read_u16_checked(bytes, pos)? as usize;
        pos += 2;
        let data = read_slice_checked(bytes, pos, len)?;
        pos += len;
        push_standard_block(
            &mut events,
            &mut blocks,
            &mut block_starts,
            &mut block_ends,
            data,
            DEFAULT_PAUSE_CYCLES,
            &mut level,
        );
    }
    Ok(Tape {
        events,
        blocks,
        block_starts,
        block_ends,
    })
}

const TZX_SIGNATURE: &[u8] = b"ZXTape!\x1a";

/// Skips a block whose body is `fixed` leading bytes, followed by a `len_bytes`
/// wide little-endian length (counted from `len_at`) and that many data bytes.
fn skip_length_prefixed(
    bytes: &[u8],
    pos: &mut usize,
    fixed: usize,
    len_at: usize,
    len_bytes: usize,
) -> Result<(), Error> {
    let start = *pos;
    let len_field = bytes
        .get(start + len_at..start + len_at + len_bytes)
        .ok_or(Error::TruncatedBlock)?;
    let len = len_field
        .iter()
        .rev()
        .fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
    let end = start + fixed + len;
    if end > bytes.len() {
        return Err(Error::TruncatedBlock);
    }
    *pos = end;
    Ok(())
}

/// Parses a `.tzx` file. Implements standard ROM blocks (`0x10`), turbo
/// data blocks (`0x11`), the custom-loader pulse/data blocks seen in real
/// `.cdt` media (`0x12` Pure Tone, `0x13` Pulse Sequence, `0x14` Pure
/// Data), the pause/stop-tape control block (`0x20`), and the informational
/// blocks whose skip length is corroborated (Group Start/End `0x21`/`0x22`,
/// Text description `0x30`, Message `0x31`, Archive info `0x32`, Hardware
/// type `0x33`, Glue `0x5A`). Every other block ID -- including `0x34` and
/// `0x35`, whose layouts the reference parsers disagree on -- is a clear
/// `Error::UnsupportedBlockType` rather than a guessed skip.
pub fn parse_tzx(bytes: &[u8]) -> Result<Tape, Error> {
    if bytes.len() < TZX_SIGNATURE.len() + 2 || &bytes[..TZX_SIGNATURE.len()] != TZX_SIGNATURE {
        return Err(Error::NotATzxFile);
    }

    let mut events = Vec::new();
    let mut blocks = Vec::new();
    let mut block_starts = Vec::new();
    let mut block_ends = Vec::new();
    let mut level = false;
    let mut pos = TZX_SIGNATURE.len() + 2; // skip signature + major/minor version bytes
    while pos < bytes.len() {
        let id = bytes[pos];
        pos += 1;

        match id {
            0x10 => {
                let pause_ms = read_u16_checked(bytes, pos)?;
                let len = read_u16_checked(bytes, pos + 2)? as usize;
                pos += 4;
                let data = read_slice_checked(bytes, pos, len)?;
                pos += len;
                push_standard_block(
                    &mut events,
                    &mut blocks,
                    &mut block_starts,
                    &mut block_ends,
                    data,
                    ms_to_cycles(pause_ms),
                    &mut level,
                );
            }
            0x11 => {
                let pilot_pulse = read_u16_checked(bytes, pos)? as u32;
                let sync1 = read_u16_checked(bytes, pos + 2)? as u32;
                let sync2 = read_u16_checked(bytes, pos + 4)? as u32;
                let bit0 = read_u16_checked(bytes, pos + 6)? as u32;
                let bit1 = read_u16_checked(bytes, pos + 8)? as u32;
                let pilot_count = read_u16_checked(bytes, pos + 10)? as u32;
                let used_bits_in_last_byte = *bytes.get(pos + 12).ok_or(Error::TruncatedBlock)?;
                let pause_ms = read_u16_checked(bytes, pos + 13)?;
                let len = read_u24_checked(bytes, pos + 15)? as usize;
                pos += 18;
                let data = read_slice_checked(bytes, pos, len)?;
                pos += len;
                push_pulse_block(
                    &mut events,
                    data,
                    pilot_pulse,
                    pilot_count,
                    sync1,
                    sync2,
                    bit0,
                    bit1,
                    used_bits_in_last_byte,
                    ms_to_cycles(pause_ms),
                    &mut level,
                );
            }
            0x12 => {
                let pulse_cycles = read_u16_checked(bytes, pos)? as u32;
                let pulse_count = read_u16_checked(bytes, pos + 2)? as u32;
                pos += 4;
                for _ in 0..pulse_count {
                    events.push(Event::Pulse(Pulse {
                        cycles: pulse_cycles,
                        level,
                    }));
                    level = !level;
                }
            }
            0x13 => {
                let pulse_count = *bytes.get(pos).ok_or(Error::TruncatedBlock)? as usize;
                pos += 1;
                for _ in 0..pulse_count {
                    let pulse_cycles = read_u16_checked(bytes, pos)? as u32;
                    pos += 2;
                    events.push(Event::Pulse(Pulse {
                        cycles: pulse_cycles,
                        level,
                    }));
                    level = !level;
                }
            }
            0x14 => {
                let bit0 = read_u16_checked(bytes, pos)? as u32;
                let bit1 = read_u16_checked(bytes, pos + 2)? as u32;
                let used_bits_in_last_byte = *bytes.get(pos + 4).ok_or(Error::TruncatedBlock)?;
                let pause_ms = read_u16_checked(bytes, pos + 5)?;
                let len = read_u24_checked(bytes, pos + 7)? as usize;
                pos += 10;
                let data = read_slice_checked(bytes, pos, len)?;
                pos += len;

                push_data_bit_pairs(
                    &mut events,
                    data,
                    bit0,
                    bit1,
                    used_bits_in_last_byte,
                    &mut level,
                );
                if pause_ms != 0 {
                    push_pause(&mut events, ms_to_cycles(pause_ms), &mut level);
                }
            }
            0x20 => {
                let pause_ms = read_u16_checked(bytes, pos)?;
                pos += 2;
                if pause_ms == 0 {
                    events.push(Event::Stop);
                } else {
                    push_pause(&mut events, ms_to_cycles(pause_ms), &mut level);
                }
            }
            0x21 => {
                let len = *bytes.get(pos).ok_or(Error::TruncatedBlock)? as usize;
                pos += 1;
                if pos + len > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                pos += len;
            }
            0x22 => {}
            // Informational blocks that carry no pulses. Their skip lengths were
            // cross-checked in two independent real parsers (ep128emu's
            // `Tape_TZX` and Caprice32's `tape.cpp`), which agree exactly.
            // `0x30` Text description: 1-byte length + text.
            0x30 => skip_length_prefixed(bytes, &mut pos, 1, 0, 1)?,
            // `0x31` Message block: display time (1 byte), 1-byte length + text.
            0x31 => skip_length_prefixed(bytes, &mut pos, 2, 1, 1)?,
            // `0x32` Archive info: 2-byte (little-endian) length + data.
            0x32 => skip_length_prefixed(bytes, &mut pos, 2, 0, 2)?,
            // `0x33` Hardware type: 1-byte machine count, then 3 bytes each.
            0x33 => {
                let count = usize::from(*bytes.get(pos).ok_or(Error::TruncatedBlock)?);
                let end = pos + 1 + count * 3;
                if end > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                pos = end;
            }
            // `0x5A` Glue block: 9 bytes (the "XTape!" signature and version).
            0x5A => {
                if pos + 9 > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                pos += 9;
            }
            // `0x34` (Emulation info) and `0x35` (Custom info) are deliberately
            // still rejected: the two parsers above disagree on `0x35`'s
            // identification-string length, and only one handles `0x34`.
            other => return Err(Error::UnsupportedBlockType(other)),
        }
    }

    Ok(Tape {
        events,
        blocks,
        block_starts,
        block_ends,
    })
}

/// Parses a `.pzx` file. Implements the 4 mandatory block types the PZX
/// spec (v1.0, see `CREDITS.md`) requires any compliant implementation to
/// support -- `PZXT` (header, skipped beyond the version check), `PULS`
/// (arbitrary pulse sequence), `DATA` (bit-encoded data via caller-defined
/// pulse sequences per bit value), and `PAUS` (a single pause pulse) --
/// plus `STOP` (stop-tape command), mapped to `Event::Stop` unconditionally
/// regardless of its `flags` field's "only if 48K" nuance (not modeled --
/// this crate has no machine-model concept at all, and stopping slightly
/// more often than strictly necessary is harmless: playback simply resumes
/// on the next `TapePlayer::play()` call). Every other block type is
/// safely skipped using its own size prefix -- unlike `.tzx`, PZX's own
/// block structure (tag + size + data) makes this always well-defined by
/// design, so an unrecognized block is never an error here (matches the
/// spec's own "must skip it and continue" requirement, not a looser
/// version of `parse_tzx`'s stricter stance).
///
/// Deliberately doesn't populate `Tape::blocks`/`block_starts`/
/// `block_ends` at all -- a PZX `DATA` block's raw bytes could equally be a
/// standard ROM-loadable block or an arbitrary custom loader's own data
/// (PZX has no per-block marker distinguishing the two, unlike `.tzx`'s
/// separate Standard/Turbo block IDs), so there's no safe way to tell
/// `try_fast_tape_load_trap` candidates apart from everything else. The
/// fast-tape-load trap simply never engages for a `.pzx`-sourced tape,
/// falling back to real-time pulse playback -- correct, if less
/// accelerated, for every case.
pub fn parse_pzx(bytes: &[u8]) -> Result<Tape, Error> {
    const PZXT: u32 = u32::from_le_bytes(*b"PZXT");
    const PULS: u32 = u32::from_le_bytes(*b"PULS");
    const DATA: u32 = u32::from_le_bytes(*b"DATA");
    const PAUS: u32 = u32::from_le_bytes(*b"PAUS");
    const STOP: u32 = u32::from_le_bytes(*b"STOP");

    if bytes.len() < 4 || &bytes[0..4] != b"PZXT" {
        return Err(Error::NotAPzxFile);
    }

    let mut events = Vec::new();
    let mut pos = 0;

    while pos < bytes.len() {
        let tag = read_u32_checked(bytes, pos)?;
        let size = read_u32_checked(bytes, pos + 4)? as usize;
        pos += 8;
        let data = read_slice_checked(bytes, pos, size)?;
        pos += size;

        match tag {
            PZXT => {} // header/version/info -- nothing this crate needs.
            PULS => push_pzx_puls(&mut events, data)?,
            DATA => push_pzx_data(&mut events, data)?,
            PAUS => {
                let raw = read_u32_checked(data, 0)?;
                events.push(Event::Pulse(Pulse {
                    cycles: raw & 0x7FFF_FFFF,
                    level: raw & 0x8000_0000 != 0,
                }));
            }
            STOP => events.push(Event::Stop),
            _ => {} // unrecognized block: safely skipped, already advanced past above.
        }
    }

    Ok(Tape {
        events,
        blocks: Vec::new(),
        block_starts: Vec::new(),
        block_ends: Vec::new(),
    })
}

/// Real Z80 T-state rate `parse_csw` converts sample-rate-relative pulse
/// durations against -- the same PAL Spectrum clock `ms_to_cycles`/
/// `DEFAULT_PAUSE_CYCLES` already use above, kept as its own named
/// constant here since the conversion math needs it explicitly, not just
/// as an inline literal.
const CSW_CLOCK_HZ: u64 = 3_546_900;

/// Converts one contiguous run of `run_length_samples` (at `sample_rate`
/// Hz) into a real T-state pulse duration in the same `CSW_CLOCK_HZ`
/// domain `parse_csw` uses. `remainder` carries the sub-T-state leftover
/// (in `sample_rate`-normalized units) across successive calls sharing
/// the same `&mut u64`, so long VOC-sourced tapes don't drift the way an
/// independent per-run rounding (no carry at all) would over many
/// pulses -- a real accumulator technique, not `parse_csw`'s own
/// simpler independent-per-pulse rounding (that format's own pulses are
/// already whole samples, so it has no equivalent drift to guard
/// against).
fn voc_run_cycles(run_length_samples: u64, sample_rate: u32, remainder: &mut u64) -> u32 {
    let numerator = run_length_samples * CSW_CLOCK_HZ + *remainder;
    let cycles = numerator / u64::from(sample_rate);
    *remainder = numerator % u64::from(sample_rate);
    cycles as u32
}

/// Decodes an 8-bit unsigned PCM sample buffer (threshold 128, matching
/// Caprice32's own `VOC_THRESHOLD`) into EAR-line pulses, sharing the
/// exact conversion `voc_run_cycles` uses. Used by every VOC block that
/// carries raw PCM audio (0x01/0x02/0x09) -- their decode loop is
/// otherwise byte-for-byte identical, only the source block's own
/// header fields differ.
fn decode_voc_pcm_samples(
    audio_data: &[u8],
    sample_rate: u32,
    level: &mut bool,
    remainder: &mut u64,
    events: &mut Vec<Event>,
) {
    let mut run_start = 0;
    for (i, &sample) in audio_data.iter().enumerate() {
        let sample_level = sample >= 128;
        if sample_level != *level {
            let run_length = (i - run_start) as u64;
            let pulse_cycles = voc_run_cycles(run_length, sample_rate, remainder);
            if pulse_cycles > 0 {
                events.push(Event::Pulse(Pulse {
                    cycles: pulse_cycles,
                    level: *level,
                }));
            }
            *level = sample_level;
            run_start = i;
        }
    }
    if run_start < audio_data.len() {
        let run_length = (audio_data.len() - run_start) as u64;
        let pulse_cycles = voc_run_cycles(run_length, sample_rate, remainder);
        if pulse_cycles > 0 {
            events.push(Event::Pulse(Pulse {
                cycles: pulse_cycles,
                level: *level,
            }));
        }
    }
}

/// How far a sample must swing from zero, as a fraction of the recording's loudest sample, to flip the EAR level in
/// [`parse_wav`]: 10 %, so hiss and hum around the zero line do not turn into pulses.
const WAV_TRIGGER: f32 = 0.10;

/// Parses a `.wav` recording of a cassette into EAR-line pulses, in the same Z80 T-state domain as [`parse_csw`] and
/// [`parse_voc`] (3,546,900 Hz).
///
/// Supported: a RIFF `WAVE` file with integer PCM (format tag 1, or the extensible tag `0xFFFE` carrying it) of 8
/// (unsigned), 16, 24 or 32 bits, or IEEE float (tag 3) of 32 or 64 bits, at any sample rate; of several channels
/// the first one is used. The `data` chunk may claim more bytes than the file holds (streamed recordings): what is
/// there is read.
///
/// The waveform becomes a square wave with a Schmitt trigger: the line goes high when a sample rises above
/// [`WAV_TRIGGER`] of the file's peak and low when it falls below minus that, and keeps its level in between, so a
/// noisy or sine-shaped recording still gives one clean edge per cycle. A silent file is one low pulse. Like `.csw`
/// and `.voc` it has no ROM-block structure, so a fast-load trap never engages for it.
pub fn parse_wav(bytes: &[u8]) -> Result<Tape, Error> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::NotAWavFile);
    }
    // Walk the chunks for `fmt ` and `data`.
    let (mut format, mut data): (Option<&[u8]>, Option<&[u8]>) = (None, None);
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let declared = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(declared).min(bytes.len());
        match id {
            b"fmt " => format = Some(&bytes[body_start..body_end]),
            b"data" => {
                data = Some(&bytes[body_start..body_end]);
                break;
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        pos = body_start
            .saturating_add(declared)
            .saturating_add(declared & 1);
    }
    let (Some(fmt), Some(data)) = (format, data) else {
        return Err(Error::NotAWavFile);
    };
    if fmt.len() < 16 {
        return Err(Error::NotAWavFile);
    }
    let mut tag = u16::from_le_bytes([fmt[0], fmt[1]]);
    let channels = usize::from(u16::from_le_bytes([fmt[2], fmt[3]])).max(1);
    let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
    let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
    if tag == 0xFFFE && fmt.len() >= 26 {
        // WAVE_FORMAT_EXTENSIBLE: the real tag is the first two bytes of the sub-format GUID.
        tag = u16::from_le_bytes([fmt[24], fmt[25]]);
    }
    if rate == 0 {
        return Err(Error::NotAWavFile);
    }
    let sample_bytes = usize::from(bits / 8);
    let decode: fn(&[u8]) -> f32 = match (tag, bits) {
        (1, 8) => |b| (f32::from(b[0]) - 128.0) / 128.0,
        (1, 16) => |b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0,
        (1, 24) => |b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
        (1, 32) => |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        (3, 32) => |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        (3, 64) => |b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
        _ => return Err(Error::UnsupportedWavFormat { tag, bits }),
    };
    let frame = sample_bytes * channels;
    let samples: Vec<f32> = data
        .chunks_exact(frame)
        .map(|f| decode(&f[..sample_bytes]))
        .collect();
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let threshold = peak * WAV_TRIGGER;

    let mut events = Vec::new();
    let (mut level, mut run_start, mut remainder) = (false, 0usize, 0u64);
    let push = |events: &mut Vec<Event>, level: bool, samples: usize, remainder: &mut u64| {
        let cycles = voc_run_cycles(samples as u64, rate, remainder);
        if cycles > 0 {
            events.push(Event::Pulse(Pulse { cycles, level }));
        }
    };
    for (i, &sample) in samples.iter().enumerate() {
        let next = if peak > 0.0 && sample >= threshold {
            true
        } else if peak > 0.0 && sample <= -threshold {
            false
        } else {
            level
        };
        if next != level {
            push(&mut events, level, i - run_start, &mut remainder);
            level = next;
            run_start = i;
        }
    }
    push(
        &mut events,
        level,
        samples.len() - run_start,
        &mut remainder,
    );

    Ok(Tape {
        events,
        blocks: Vec::new(),
        block_starts: Vec::new(),
        block_ends: Vec::new(),
    })
}

/// Parses a `.csw` (Compressed Square Wave) file -- a literal sampled
/// waveform: a real recording of the tape's own EAR-line pulses, encoded
/// as one byte per pulse (its duration, in samples at the file's own
/// declared sample rate), not a hardware-protocol-level block structure
/// the way `.tap`/`.tzx`/`.pzx` all are. Format verified against
/// `k1.spdns.de`'s own CSW file format page (see `CREDITS.md`), not
/// guessed: both v1 (header ends `0x20`, sample rate a 16-bit `WORD`,
/// RLE-only) and v2 (header ends `0x34 + <extension length>`, sample rate
/// a 32-bit `DWORD`, RLE *or* Z-RLE (RLE further zlib-compressed)) are
/// supported, dispatched on the major-version byte at offset `0x17`.
///
/// Pulse encoding (both versions, once any Z-RLE zlib layer is peeled
/// off): each byte is a pulse's duration in samples, except `0x00`, which
/// means "the real duration doesn't fit in one byte" and is followed by
/// its real value as a 4-byte little-endian `DWORD`. Level starts at the
/// header's own `flags` bit 0 (initial polarity) and alternates after
/// every pulse -- the same "square wave, toggle each pulse" shape
/// `push_pzx_puls` already implements for `.pzx`, just driven by a flat
/// byte stream instead of a block's own repeat-count encoding.
///
/// Deliberately doesn't populate `Tape::blocks`/`block_starts`/
/// `block_ends` at all, same reasoning as `parse_pzx`: a `.csw` file has
/// no block/marker structure whatsoever to distinguish a standard
/// ROM-loadable data stream from an arbitrary custom loader's own
/// waveform, so `try_fast_tape_load_trap` never has anything to safely
/// recognize here -- falls back to real-time pulse playback always,
/// correct if less accelerated, exactly like a `.pzx`-sourced tape.
pub fn parse_csw(bytes: &[u8]) -> Result<Tape, Error> {
    const SIGNATURE: &[u8; 22] = b"Compressed Square Wave";
    if bytes.len() < 23 || &bytes[0..22] != SIGNATURE || bytes[22] != 0x1A {
        return Err(Error::NotACswFile);
    }
    let major = *bytes.get(23).ok_or(Error::TruncatedBlock)?;

    let (sample_rate, compression_type, flags, pulse_data_start): (u64, u8, u8, usize) = match major
    {
        1 => {
            let sample_rate = read_u16_checked(bytes, 0x19)? as u64;
            let compression_type = *bytes.get(0x1B).ok_or(Error::TruncatedBlock)?;
            let flags = *bytes.get(0x1C).ok_or(Error::TruncatedBlock)?;
            (sample_rate, compression_type, flags, 0x20)
        }
        2 => {
            let sample_rate = read_u32_checked(bytes, 0x19)? as u64;
            // Total-pulse count lives at 0x1D (4 bytes) -- not needed
            // for decoding (the pulse stream is self-terminating),
            // deliberately not read.
            let compression_type = *bytes.get(0x21).ok_or(Error::TruncatedBlock)?;
            let flags = *bytes.get(0x22).ok_or(Error::TruncatedBlock)?;
            let ext_len = *bytes.get(0x23).ok_or(Error::TruncatedBlock)? as usize;
            (sample_rate, compression_type, flags, 0x24 + 16 + ext_len)
        }
        other => return Err(Error::UnsupportedCswVersion(other)),
    };
    if sample_rate == 0 {
        // A real file always declares a real sample rate -- guards the
        // division below rather than letting a corrupt/malformed `0`
        // panic on divide-by-zero.
        return Err(Error::TruncatedBlock);
    }

    let compressed = read_slice_checked(bytes, pulse_data_start, bytes.len() - pulse_data_start)?;
    let pulse_bytes: std::borrow::Cow<[u8]> = match compression_type {
        1 => std::borrow::Cow::Borrowed(compressed),
        2 => {
            use std::io::Read;
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(compressed)
                .read_to_end(&mut out)
                .map_err(|_| Error::CswInflateFailed)?;
            std::borrow::Cow::Owned(out)
        }
        other => return Err(Error::UnsupportedCswCompression(other)),
    };

    let mut events = Vec::new();
    let mut level = flags & 0x01 != 0;
    let mut pos = 0;
    while pos < pulse_bytes.len() {
        let duration_samples = if pulse_bytes[pos] == 0 {
            let d = read_u32_checked(&pulse_bytes, pos + 1)?;
            pos += 5;
            d
        } else {
            let d = pulse_bytes[pos] as u32;
            pos += 1;
            d
        };
        let cycles = (duration_samples as u64 * CSW_CLOCK_HZ / sample_rate) as u32;
        events.push(Event::Pulse(Pulse { cycles, level }));
        level = !level;
    }

    Ok(Tape {
        events,
        blocks: Vec::new(),
        block_starts: Vec::new(),
        block_ends: Vec::new(),
    })
}

/// Parses a `.voc` (Creative Voice File) format tape. Format verified against
/// https://wiki.multimedia.cx/index.php/Creative_Voice (see `CREDITS.md`),
/// not guessed. Implements block types: 0x00 Terminator, 0x01 Sound Data,
/// 0x02 Sound Data Continuation, 0x03 Silence, 0x04 Marker (skip),
/// 0x05 Text (skip), 0x06 Repeat Start, 0x07 Repeat End, 0x08 Extra Info,
/// 0x09 New-Format Sound. Only 8-bit unsigned PCM audio is supported
/// (codec 0x00); any other codec triggers UnsupportedVocCodec. Unknown
/// block types trigger UnsupportedVocBlock.
///
/// Converts PCM samples to EAR-line pulses using threshold 128 (>=128 high,
/// <128 low), in the same Z80 T-state domain as parse_csw (3,546,900 Hz).
/// Deliberately doesn't populate `Tape::blocks`/`block_starts`/`block_ends`
/// (same reasoning as parse_pzx/parse_csw: raw audio has no ROM-block
/// structure), so `try_fast_tape_load_trap` never engages for a
/// `.voc`-sourced tape -- falls back to real-time pulse playback.
pub fn parse_voc(bytes: &[u8]) -> Result<Tape, Error> {
    // Validate 26-byte VOC header
    const HEADER_SIZE: usize = 26;
    const SIGNATURE: &[u8] = b"Creative Voice File\x1A";
    const SIGNATURE_SIZE: usize = 20; // includes \x1A terminator

    if bytes.len() < HEADER_SIZE {
        return Err(Error::NotAVocFile);
    }

    if &bytes[0..SIGNATURE_SIZE] != SIGNATURE {
        return Err(Error::NotAVocFile);
    }

    // Read data block offset (u16 LE at offset 0x14 = 20)
    let data_offset = read_u16_checked(bytes, 0x14)? as usize;

    // Read version (u16 LE at offset 0x16 = 22)
    let version = read_u16_checked(bytes, 0x16)?;

    // Read and validate checksum (u16 LE at offset 0x18 = 24)
    let checksum = read_u16_checked(bytes, 0x18)?;
    let expected_checksum = (0x1234u32).wrapping_sub(version as u32) as u16;
    if checksum != expected_checksum {
        return Err(Error::NotAVocFile);
    }

    // Parse blocks starting at data_offset
    let mut events = Vec::new();
    let mut pos = data_offset;
    let mut level = false; // Start at low level, will toggle on first pulse
    let mut current_sample_rate: u32 = 11025; // Default sample rate (256_000_000 / (256 - 231))
    let mut remainder_fractional: u64 = 0; // Fractional T-state remainder for accurate conversion

    // For repeat blocks
    #[derive(Debug, Clone)]
    struct RepeatRegion {
        start_event_pos: usize,
        count: u16,
    }
    let mut repeat_stack: Vec<RepeatRegion> = Vec::new();

    loop {
        // Check bounds before reading block type
        if pos >= bytes.len() {
            break;
        }

        let block_type = bytes[pos];
        pos += 1;

        match block_type {
            0x00 => {
                // Terminator: end of file
                break;
            }
            0x01 => {
                // Sound Data block
                if pos + 2 > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                let freq_divisor = bytes[pos];
                let codec = bytes[pos + 1];
                pos += 2;

                // Only support codec 0x00 (8-bit unsigned PCM)
                if codec != 0x00 {
                    return Err(Error::UnsupportedVocCodec(codec));
                }

                // Calculate sample rate from divisor
                current_sample_rate = 1_000_000u32 / (256u32 - freq_divisor as u32).max(1);

                // Read 3-byte data length (u24 LE)
                let data_len = read_u24_checked(bytes, pos)? as usize;
                pos += 3;

                let audio_data = read_slice_checked(bytes, pos, data_len)?;
                pos += data_len;

                decode_voc_pcm_samples(
                    audio_data,
                    current_sample_rate,
                    &mut level,
                    &mut remainder_fractional,
                    &mut events,
                );
            }
            0x02 => {
                // Sound Data Continuation block (same codec/rate and
                // continuing waveform level as prior 0x01/0x02 --
                // `decode_voc_pcm_samples` always continues from
                // whatever `level` the file's own waveform is already at).
                let data_len = read_u24_checked(bytes, pos)? as usize;
                pos += 3;

                let audio_data = read_slice_checked(bytes, pos, data_len)?;
                pos += data_len;

                decode_voc_pcm_samples(
                    audio_data,
                    current_sample_rate,
                    &mut level,
                    &mut remainder_fractional,
                    &mut events,
                );
            }
            0x03 => {
                // Silence block: emit a non-toggling pulse
                if pos + 3 > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                let silence_length = read_u16_checked(bytes, pos)? as u32;
                let freq_divisor = bytes[pos + 2];
                pos += 3;

                let sample_rate_silence = 1_000_000u32 / (256u32 - freq_divisor as u32).max(1);
                let silence_samples = silence_length.wrapping_add(1);
                let pulse_cycles = voc_run_cycles(
                    silence_samples as u64,
                    sample_rate_silence,
                    &mut remainder_fractional,
                );

                if pulse_cycles > 0 {
                    events.push(Event::Pulse(Pulse {
                        cycles: pulse_cycles,
                        level,
                    }));
                    // Note: silence does NOT toggle the level
                }
            }
            0x04 => {
                // Marker block: skip 2 bytes
                if pos + 2 > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                pos += 2;
            }
            0x05 => {
                // Text block: read null-terminated string
                let _text_start = pos;
                while pos < bytes.len() && bytes[pos] != 0 {
                    pos += 1;
                }
                if pos >= bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                pos += 1; // Skip null terminator
            }
            0x06 => {
                // Repeat Start block
                if pos + 2 > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                let count = read_u16_checked(bytes, pos)?;
                pos += 2;

                if count == 0xFFFF {
                    return Err(Error::VocEndlessRepeat);
                }

                repeat_stack.push(RepeatRegion {
                    start_event_pos: events.len(),
                    count,
                });
            }
            0x07 => {
                // Repeat End block
                if let Some(repeat_region) = repeat_stack.pop() {
                    // Unroll: repeat enclosed blocks (count + 1) times total
                    // We've already processed once, so repeat (count) more times
                    let _end_pos = pos;
                    let end_event_pos = events.len();
                    let repeated_block =
                        events[repeat_region.start_event_pos..end_event_pos].to_vec();

                    for _ in 0..repeat_region.count {
                        events.extend_from_slice(&repeated_block);
                    }
                } else {
                    // Unmatched 0x07, just skip it
                }
            }
            0x08 => {
                // Extra Info block
                if pos + 4 > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                let freq_divisor_u16 = read_u16_checked(bytes, pos)?;
                let codec = bytes[pos + 2];
                let channels_minus_1 = bytes[pos + 3];
                pos += 4;

                // Only single-channel (channels_minus_1 == 0)
                if channels_minus_1 != 0 {
                    return Err(Error::UnsupportedVocCodec(codec));
                }

                // Only codec 0x00 (8-bit unsigned PCM)
                if codec != 0x00 {
                    return Err(Error::UnsupportedVocCodec(codec));
                }

                // Update sample rate
                let divisor_u32 = freq_divisor_u16 as u32;
                current_sample_rate = 256_000_000u32 / (65536u32 - divisor_u32).max(1);
            }
            0x09 => {
                // New-Format Sound block
                if pos + 12 > bytes.len() {
                    return Err(Error::TruncatedBlock);
                }
                let sample_rate_hz = read_u32_checked(bytes, pos)?;
                let bits_per_sample = bytes[pos + 4];
                let channels = bytes[pos + 5];
                let codec = read_u16_checked(bytes, pos + 6)?;
                // 4 reserved bytes at pos + 8, skip them
                pos += 12;

                if codec != 0 || bits_per_sample != 8 || channels != 1 {
                    return Err(Error::UnsupportedVocCodec(codec as u8));
                }

                if sample_rate_hz == 0 {
                    return Err(Error::TruncatedBlock);
                }

                current_sample_rate = sample_rate_hz;

                // Read audio data: 3-byte length
                let data_len = read_u24_checked(bytes, pos)? as usize;
                pos += 3;

                let audio_data = read_slice_checked(bytes, pos, data_len)?;
                pos += data_len;

                decode_voc_pcm_samples(
                    audio_data,
                    current_sample_rate,
                    &mut level,
                    &mut remainder_fractional,
                    &mut events,
                );
            }
            other => {
                return Err(Error::UnsupportedVocBlock(other));
            }
        }
    }

    Ok(Tape {
        events,
        blocks: Vec::new(),
        block_starts: Vec::new(),
        block_ends: Vec::new(),
    })
}

/// Writes a `Tape` as a `.tzx`/`.cdt` file made of Pulse Sequence blocks
/// (`0x13`, up to 255 pulses each) and Pause blocks (`0x20`): a literal
/// pulse capture rather than a decoded data stream, which is exactly what
/// a recording of a cassette output line is. [`parse_tzx`] reads the result
/// back to the same pulse timeline.
///
/// TZX pulses carry no level of their own -- each block starts at the line
/// level the previous one left and every pulse toggles it -- so the writer
/// keeps the parser's view of the level and, where a recorded pulse's level
/// doesn't line up (after a pause, which leaves the line low), inserts a
/// 1 T-state pulse to flip it. A pulse too long for a `u16` T-state field
/// (over about 18 ms: silence between blocks) becomes a Pause block, rounded
/// up to the next millisecond.
pub fn write_tzx(tape: &Tape) -> Vec<u8> {
    fn flush(out: &mut Vec<u8>, pulses: &mut Vec<u16>) {
        if pulses.is_empty() {
            return;
        }
        out.push(0x13);
        out.push(pulses.len() as u8);
        for pulse in pulses.drain(..) {
            out.extend_from_slice(&pulse.to_le_bytes());
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(TZX_SIGNATURE);
    out.extend_from_slice(&[1, 20]);
    let mut pending: Vec<u16> = Vec::new();
    // The level the *parser* is at: false after a pause and at the start.
    let mut line_level = false;
    for event in &tape.events {
        match *event {
            Event::Pulse(pulse) if pulse.cycles > u32::from(u16::MAX) => {
                flush(&mut out, &mut pending);
                let ms = (u64::from(pulse.cycles) * 1000).div_ceil(3_546_900);
                let ms = ms.min(u64::from(u16::MAX)) as u16;
                out.push(0x20);
                out.extend_from_slice(&ms.to_le_bytes());
                line_level = false;
            }
            Event::Pulse(pulse) => {
                if pulse.level != line_level {
                    pending.push(1);
                    line_level = !line_level;
                    if pending.len() == 255 {
                        flush(&mut out, &mut pending);
                    }
                }
                pending.push(pulse.cycles as u16);
                line_level = !line_level;
                if pending.len() == 255 {
                    flush(&mut out, &mut pending);
                }
            }
            Event::Stop => {
                flush(&mut out, &mut pending);
                out.push(0x20);
                out.extend_from_slice(&0u16.to_le_bytes());
                line_level = false;
            }
        }
    }
    flush(&mut out, &mut pending);
    out
}

/// Error type for `write_voc` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteVocError {
    /// Tape contains unsupported event types (e.g., Stop events, which
    /// write_voc doesn't model in VOC format).
    UnsupportedEvent,
}

impl std::fmt::Display for WriteVocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteVocError::UnsupportedEvent => {
                write!(f, "tape contains Stop events, unsupported by .voc format")
            }
        }
    }
}

impl std::error::Error for WriteVocError {}

/// Writes a `Tape` to VOC (Creative Voice File) format. Emits a single
/// 0x01 Sound Data block containing 8-bit unsigned PCM samples (128 = low,
/// 255 = high, by level) at the specified `sample_rate`, followed by a 0x00
/// Terminator. Returns a proper 26-byte VOC header with correct checksum.
///
/// Round-trip (write_voc then parse_voc) reproduces equivalent pulse widths
/// within one sample's rounding tolerance.
pub fn write_voc(tape: &Tape, sample_rate: u32) -> Result<Vec<u8>, WriteVocError> {
    if sample_rate == 0 {
        // Avoid division by zero; use a sensible default
        return Err(WriteVocError::UnsupportedEvent);
    }

    // Build VOC header (26 bytes)
    let mut output = Vec::with_capacity(26 + 6 + 8192); // header + 0x01 block header + estimated audio data

    // Signature: "Creative Voice File\x1A" (20 bytes)
    output.extend_from_slice(b"Creative Voice File\x1A");

    // Data block offset (u16 LE): always 26 (start of first block)
    output.extend_from_slice(&26u16.to_le_bytes());

    // Version (u16 LE): use 0x0100 (1.00)
    let version = 0x0100u16;
    output.extend_from_slice(&version.to_le_bytes());

    // Checksum (u16 LE): 0x1234 - version
    let checksum = (0x1234u32).wrapping_sub(version as u32) as u16;
    output.extend_from_slice(&checksum.to_le_bytes());

    // Now emit 0x01 Sound Data block
    output.push(0x01); // Block type: Sound Data

    // A VOC Sound Data block stores its rate as an 8-bit divisor: rate =
    // 1_000_000 / (256 - divisor), so only whole-microsecond sample periods
    // exist (44100 Hz -> a 22 us period -> 45454 Hz). The samples below must
    // be generated at the rate the header really encodes, not the requested
    // one: using the requested rate scaled every pulse by requested/actual
    // (about 3% short at 44.1 kHz) when the file was read back -- enough to
    // shorten a tape's pilot tone past a loader's start-up window.
    let period_us = (1_000_000u32 / sample_rate).clamp(1, 256);
    let freq_divisor = (256 - period_us) as u8;
    let effective_rate = 1_000_000u32 / period_us;
    output.push(freq_divisor);

    output.push(0x00); // Codec: 8-bit unsigned PCM

    // Build PCM audio data by converting pulses to samples
    let mut audio_data = Vec::new();
    let mut current_level = false;
    let mut remainder: u64 = 0;

    for event in tape.events() {
        match event {
            Event::Pulse(pulse) => {
                // Convert cycle duration to sample count
                let sample_count_u64 =
                    (pulse.cycles as u64 * effective_rate as u64).wrapping_add(remainder);
                remainder = sample_count_u64 % CSW_CLOCK_HZ;
                let sample_count = (sample_count_u64 / CSW_CLOCK_HZ) as u32;

                // Emit samples: low=0, high=255
                let sample_value = if current_level { 255u8 } else { 0u8 };
                for _ in 0..sample_count {
                    audio_data.push(sample_value);
                }
                current_level = !current_level;
            }
            Event::Stop => {
                // Stop events can't be represented in VOC format
                return Err(WriteVocError::UnsupportedEvent);
            }
        }
    }

    // Write audio data length (u24 LE)
    let data_len = audio_data.len() as u32;
    let len_bytes = data_len.to_le_bytes();
    output.extend_from_slice(&len_bytes[0..3]);

    // Append audio data
    output.extend_from_slice(&audio_data);

    // Terminator block
    output.push(0x00);

    Ok(output)
}

/// A `PULS` block's own pulse-decoding loop -- see the PZX spec's own
/// pseudocode for this exact algorithm (`CREDITS.md`). Level starts low at
/// the start of every `PULS` block (the spec's own default, independent of
/// whatever any earlier block left off at -- a zero-duration leading pulse
/// is how a real encoder flips this to high when needed, and `Event::
/// Pulse`'s own explicit `level` field already supports that directly).
fn push_pzx_puls(events: &mut Vec<Event>, data: &[u8]) -> Result<(), Error> {
    let mut level = false;
    let mut pos = 0;
    while pos < data.len() {
        let mut count = 1u32;
        let mut duration = read_u16_checked(data, pos)? as u32;
        pos += 2;
        if duration > 0x8000 {
            count = duration & 0x7FFF;
            duration = read_u16_checked(data, pos)? as u32;
            pos += 2;
        }
        if duration >= 0x8000 {
            duration &= 0x7FFF;
            duration <<= 16;
            duration |= read_u16_checked(data, pos)? as u32;
            pos += 2;
        }
        for _ in 0..count {
            events.push(Event::Pulse(Pulse {
                cycles: duration,
                level,
            }));
            level = !level;
        }
    }
    Ok(())
}

/// A `DATA` block's own bit-decoding loop -- see the PZX spec's own field
/// layout (`CREDITS.md`). Unlike `PULS`, the initial level here is
/// explicit (bit 31 of `count`), and stays continuous (never resets)
/// across every bit of the block plus its own trailing tail pulse.
fn push_pzx_data(events: &mut Vec<Event>, data: &[u8]) -> Result<(), Error> {
    let raw_count = read_u32_checked(data, 0)?;
    let bit_count = raw_count & 0x7FFF_FFFF;
    let mut level = raw_count & 0x8000_0000 != 0;
    let tail = read_u16_checked(data, 4)? as u32;
    let p0 = *data.get(6).ok_or(Error::TruncatedBlock)? as usize;
    let p1 = *data.get(7).ok_or(Error::TruncatedBlock)? as usize;

    let s0_start = 8;
    let s1_start = s0_start + 2 * p0;
    let data_start = s1_start + 2 * p1;

    let read_sequence = |start: usize, count: usize| -> Result<Vec<u32>, Error> {
        (0..count)
            .map(|i| Ok(read_u16_checked(data, start + i * 2)? as u32))
            .collect()
    };
    let s0 = read_sequence(s0_start, p0)?;
    let s1 = read_sequence(s1_start, p1)?;

    let data_bytes = read_slice_checked(data, data_start, bit_count.div_ceil(8) as usize)?;
    for bit_index in 0..bit_count {
        let byte = data_bytes[(bit_index / 8) as usize];
        let bit = (byte >> (7 - (bit_index % 8))) & 1;
        let sequence = if bit == 0 { &s0 } else { &s1 };
        for &cycles in sequence {
            events.push(Event::Pulse(Pulse { cycles, level }));
            level = !level;
        }
    }

    events.push(Event::Pulse(Pulse {
        cycles: tail,
        level,
    }));
    Ok(())
}

/// Walks a `Tape`'s events in real time, exposing the EAR line's current
/// level. `tick` advances it by however many real CPU cycles just
/// elapsed; the caller (`zx_spectrum::ZxSpectrumBus`) is responsible for
/// calling it once per CPU step, same pattern as `ay_3_8910::Ay3_8910::
/// tick`.
#[derive(Debug, Clone)]
pub struct TapePlayer {
    tape: Tape,
    position: usize,
    cycles_into_current: u32,
    playing: bool,
    speed_multiplier: u32,
}

/// A `TapePlayer`'s playback position, for save states -- deliberately
/// excludes the tape's own data (`TapePlayer::tape`), which stays
/// externally supplied (see `zx_spectrum`'s own save-state module) exactly
/// like `--rom`/`--tape` already are for snapshots.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct TapePosition {
    position: usize,
    cycles_into_current: u32,
    playing: bool,
    speed_multiplier: u32,
}

/// Live playback state snapshot for diagnostics/UI. Unlike `TapePosition`,
/// this includes the tape's total event count and is not serde-bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapePlaybackState {
    pub position: usize,
    pub events: usize,
    pub playing: bool,
    pub finished: bool,
    pub speed_multiplier: u32,
}

impl TapePlayer {
    pub fn new(tape: Tape) -> Self {
        Self {
            tape,
            position: 0,
            cycles_into_current: 0,
            playing: false,
            speed_multiplier: 1,
        }
    }

    /// This player's own playback position, for save states.
    pub fn dump_state(&self) -> TapePosition {
        TapePosition {
            position: self.position,
            cycles_into_current: self.cycles_into_current,
            playing: self.playing,
            speed_multiplier: self.speed_multiplier,
        }
    }

    /// Restores a previously-dumped position -- the caller is responsible
    /// for having already loaded the same tape a save state expects, same
    /// as `--rom`/`--tape` for snapshots. `position` is clamped to this
    /// tape's own event count rather than trusted verbatim, in case a
    /// different (e.g. shorter) tape ended up loaded instead -- an
    /// out-of-range position would otherwise panic the next `tick`/
    /// `ear_bit` call.
    pub fn restore_state(&mut self, state: TapePosition) {
        self.position = state.position.min(self.tape.events.len());
        self.cycles_into_current = state.cycles_into_current;
        self.playing = state.playing;
        self.speed_multiplier = state.speed_multiplier.max(1);
    }

    /// Turbo mode: the *same* real pulse-level protocol, sped up -- not a
    /// different mechanism. `n` of `0` is treated as `1` (real time).
    pub fn set_speed_multiplier(&mut self, n: u32) {
        self.speed_multiplier = n.max(1);
    }

    /// Starts (or resumes) playback. A no-op if the tape has already run
    /// out -- there's nothing left to play.
    pub fn play(&mut self) {
        if !self.finished() {
            self.playing = true;
        }
    }

    /// Pauses playback without changing tape position or pulse phase.
    /// `play()` resumes from the same point. Host machines use this when
    /// their physical cassette motor has stopped after any hardware-style
    /// spin-down/coast window; this is deliberately different from a
    /// format-level `Stop` event, which advances past its marker.
    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn finished(&self) -> bool {
        self.position >= self.tape.events.len()
    }

    /// Current playback status for diagnostics.
    pub fn playback_state(&self) -> TapePlaybackState {
        TapePlaybackState {
            position: self.position,
            events: self.tape.events.len(),
            playing: self.playing,
            finished: self.finished(),
            speed_multiplier: self.speed_multiplier,
        }
    }
    /// Remaining tape-domain cycles until the next EAR-level transition
    /// from the current output level, if currently playing.
    ///
    /// Useful for host motor-control heuristics that need to avoid
    /// truncating a pulse mid-level and synthesizing a fake edge. Returns
    /// `None` when playback is idle/finished.
    pub fn cycles_until_next_edge(&self) -> Option<u32> {
        if !self.playing || self.finished() {
            return None;
        }

        let (level, mut remaining) = match self.tape.events.get(self.position)? {
            Event::Pulse(pulse) => (
                pulse.level,
                pulse.cycles.saturating_sub(self.cycles_into_current),
            ),
            Event::Stop => return None,
        };

        let mut idx = self.position + 1;
        while let Some(event) = self.tape.events.get(idx) {
            match event {
                Event::Stop => return Some(remaining),
                Event::Pulse(pulse) => {
                    if pulse.level != level {
                        return Some(remaining);
                    }
                    remaining = remaining.saturating_add(pulse.cycles);
                    idx += 1;
                }
            }
        }

        Some(remaining)
    }

    /// Advances playback by `cycles` real CPU cycles (scaled by the speed
    /// multiplier). A no-op while not playing or already finished.
    pub fn tick(&mut self, cycles: u32) {
        if !self.playing || self.finished() {
            return;
        }

        let mut remaining = cycles.saturating_mul(self.speed_multiplier);
        while remaining > 0 && !self.finished() {
            match self.tape.events[self.position] {
                Event::Stop => {
                    // Playback halts here until `play` is called again --
                    // advance past the marker now, so resuming doesn't
                    // immediately re-trigger the same stop.
                    self.playing = false;
                    self.position += 1;
                    self.cycles_into_current = 0;
                    return;
                }
                Event::Pulse(pulse) => {
                    let left_in_pulse = pulse.cycles.saturating_sub(self.cycles_into_current);
                    if remaining < left_in_pulse {
                        self.cycles_into_current += remaining;
                        remaining = 0;
                    } else {
                        remaining -= left_in_pulse;
                        self.cycles_into_current = 0;
                        self.position += 1;
                    }
                }
            }
        }
    }

    /// The EAR line's current level. Idle-high (`true`) whenever playback
    /// isn't actively running (not yet started, stopped at a `Stop`
    /// event, or the tape has fully finished) -- matches a real stopped
    /// cassette presenting no signal, regardless of which pulse happens
    /// to be queued up next.
    pub fn ear_bit(&self) -> bool {
        if !self.playing || self.finished() {
            return true;
        }
        match self.tape.events[self.position] {
            Event::Pulse(pulse) => pulse.level,
            Event::Stop => true,
        }
    }

    /// Returns the standard-protocol block "current" at this player's real
    /// playback position (flag byte + payload + checksum, exactly as
    /// stored in the file), and fast-forwards that position past the
    /// block's own pulses straight to whatever comes next -- for a ROM
    /// `LD-BYTES` fast-load trap to consume directly instead of making it
    /// wait for real-time pulses. `None` if playback isn't currently
    /// sitting at/within a standard block at all (e.g. it's positioned
    /// inside a `.tzx` Turbo Speed Data Block, which never appears in
    /// `Tape::blocks` -- see that field's own doc comment -- or the tape
    /// has no more standard blocks left).
    ///
    /// Looks up "which block" from the *current pulse position*, not a
    /// separate independent counter, so this stays correct even when some
    /// blocks play in real time (untrapped) and others get trapped, in any
    /// mix or order.
    pub fn take_next_raw_block(&mut self) -> Option<&[u8]> {
        let idx = self
            .tape
            .block_starts
            .partition_point(|&start| start <= self.position)
            .checked_sub(1)?;
        // `position` only counts as "at/within this block" up to its own
        // recorded end -- deliberately *not* "before the next standard
        // block's start", since a `.tzx` Turbo Speed Data Block (never
        // recorded in `blocks`/`block_starts`) can sit in between, and
        // `position` sitting inside *that* must not be mistaken for still
        // being inside this earlier standard block.
        if self.position >= self.tape.block_ends[idx] {
            return None;
        }

        self.position = self.tape.block_ends[idx];
        self.cycles_into_current = 0;
        Some(&self.tape.blocks[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tap_synthesizes_the_standard_pilot_sync_and_bit_pulses() {
        // Minimal 1-byte block: flag byte 0x00 (header -> 8063 pilot
        // pulses per the verified standard protocol), all bits zero.
        let bytes = [0x01, 0x00, 0x00]; // len=1 (LE), data=[0x00]
        let tape = parse_tap(&bytes).unwrap();

        // 8063 pilot + 2 sync + 8 bits * 2 pulses/bit + the pause. That is an
        // odd number of pulses, so the last data pulse is low and the pause
        // opens with its terminating 1 ms high edge before the silence.
        assert_eq!(tape.events.len(), 8063 + 2 + 16 + 2);

        assert_eq!(
            tape.events[0],
            Event::Pulse(Pulse {
                cycles: PILOT_PULSE,
                level: false
            })
        );
        assert_eq!(
            tape.events[1],
            Event::Pulse(Pulse {
                cycles: PILOT_PULSE,
                level: true
            })
        );

        // Sync pair right after the pilot tone (indices 0..8062 are
        // pilot, so index 8063 is the first index after it). 8063 pilot
        // pulses is an odd count, so the alternation (starting at low)
        // has flipped an odd number of times by the time sync1 pushes --
        // sync1 lands on level=true.
        assert_eq!(
            tape.events[8063],
            Event::Pulse(Pulse {
                cycles: SYNC1,
                level: true
            })
        );
        assert_eq!(
            tape.events[8064],
            Event::Pulse(Pulse {
                cycles: SYNC2,
                level: false
            })
        );

        // First data bit (0, since the byte is 0x00) -> two BIT0_PULSE
        // pulses right after sync, continuing the same alternation.
        assert_eq!(
            tape.events[8065],
            Event::Pulse(Pulse {
                cycles: BIT0_PULSE,
                level: true
            })
        );
        assert_eq!(
            tape.events[8066],
            Event::Pulse(Pulse {
                cycles: BIT0_PULSE,
                level: false
            })
        );

        // Final event is the pause, which drives/holds the low level.
        assert_eq!(
            *tape.events.last().unwrap(),
            Event::Pulse(Pulse {
                cycles: DEFAULT_PAUSE_CYCLES - ms_to_cycles(1),
                level: false
            })
        );
        assert_eq!(
            tape.events[tape.events.len() - 2],
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(1),
                level: true
            })
        );
    }

    #[test]
    fn parse_tap_uses_data_pilot_count_when_flag_byte_is_128_or_above() {
        let bytes = [0x01, 0x00, 0xFF]; // flag byte 0xFF (>= 128) -> data block
        let tape = parse_tap(&bytes).unwrap();

        // 3223 pilot + 2 sync + 16 bit pulses + a pause that opens with its
        // 1 ms terminating edge (the last data pulse is low).
        assert_eq!(tape.events.len(), 3223 + 2 + 16 + 2);
    }

    #[test]
    fn parse_tap_rejects_a_truncated_block() {
        let bytes = [0x05, 0x00, 0x01, 0x02]; // claims len=5, only 2 bytes follow
        assert_eq!(parse_tap(&bytes), Err(Error::TruncatedBlock));
    }

    #[test]
    fn parse_tap_handles_multiple_blocks() {
        // Two 1-byte blocks back to back.
        let bytes = [0x01, 0x00, 0x00, 0x01, 0x00, 0xFF];
        let tape = parse_tap(&bytes).unwrap();

        let first_block_len = 8063 + 2 + 16 + 2;
        let second_block_len = 3223 + 2 + 16 + 2;
        assert_eq!(tape.events.len(), first_block_len + second_block_len);
    }

    #[test]
    fn tape_player_reports_idle_level_until_played() {
        let tape = Tape {
            events: vec![Event::Pulse(Pulse {
                cycles: 10,
                level: true,
            })],
            ..Default::default()
        };
        let player = TapePlayer::new(tape);

        assert!(!player.finished());
        assert!(player.ear_bit()); // idle -- not playing yet
    }

    #[test]
    fn tape_player_advances_through_pulses_as_it_ticks() {
        let tape = Tape {
            events: vec![
                Event::Pulse(Pulse {
                    cycles: 10,
                    level: true,
                }),
                Event::Pulse(Pulse {
                    cycles: 5,
                    level: false,
                }),
            ],
            ..Default::default()
        };
        let mut player = TapePlayer::new(tape);
        player.play();

        assert!(player.ear_bit()); // first pulse: level=true

        player.tick(10); // exactly consumes the first pulse
        assert!(!player.ear_bit()); // now on the second pulse: level=false

        player.tick(5); // exactly consumes the second pulse -- tape ends
        assert!(player.finished());
        assert!(player.ear_bit()); // idle level once finished
    }

    #[test]
    fn tape_player_speed_multiplier_compresses_real_elapsed_time() {
        let tape = Tape {
            events: vec![Event::Pulse(Pulse {
                cycles: 100,
                level: true,
            })],
            ..Default::default()
        };
        let mut player = TapePlayer::new(tape);
        player.set_speed_multiplier(4);
        player.play();

        player.tick(25); // 25 real cycles * 4x = 100 effective cycles
        assert!(player.finished());
    }

    #[test]
    fn tape_player_stops_at_a_stop_event_and_resumes_on_play() {
        let tape = Tape {
            events: vec![
                Event::Pulse(Pulse {
                    cycles: 10,
                    level: true,
                }),
                Event::Stop,
                Event::Pulse(Pulse {
                    cycles: 10,
                    level: false,
                }),
            ],
            ..Default::default()
        };
        let mut player = TapePlayer::new(tape);
        player.play();
        player.tick(10); // consumes the first pulse, lands on Stop
        player.tick(1); // processes the Stop event -- playback halts

        assert!(!player.finished());
        assert!(player.ear_bit()); // stopped -- idle level regardless of position

        player.play(); // resume
        assert!(!player.ear_bit()); // now on the third event immediately
    }

    #[test]
    fn tape_player_play_is_a_no_op_once_finished() {
        let tape = Tape {
            events: vec![Event::Pulse(Pulse {
                cycles: 1,
                level: true,
            })],
            ..Default::default()
        };
        let mut player = TapePlayer::new(tape);
        player.play();
        player.tick(1);
        assert!(player.finished());

        player.play(); // no-op: nothing left to play
        assert!(player.ear_bit()); // still idle
    }

    #[test]
    fn take_next_raw_block_returns_blocks_in_order_then_none_when_exhausted() {
        let bytes = [0x01, 0x00, 0x00, 0x01, 0x00, 0xFF];
        let tape = parse_tap(&bytes).unwrap();
        let mut player = TapePlayer::new(tape);

        assert_eq!(player.take_next_raw_block(), Some(&[0x00][..]));
        assert_eq!(player.take_next_raw_block(), Some(&[0xFF][..]));
        assert_eq!(player.take_next_raw_block(), None);
    }

    #[test]
    fn take_next_raw_block_fast_forwards_real_time_playback_past_the_consumed_block() {
        let bytes = [0x01, 0x00, 0x00, 0x01, 0x00, 0xFF];
        let tape = parse_tap(&bytes).unwrap();
        let mut player = TapePlayer::new(tape);

        player.take_next_raw_block(); // consume block 0 -- fast-forwards past it
        player.play();
        // `take_next_raw_block` should leave playback positioned at block 1's
        // first pilot pulse (not still in block 0's trailing pause).
        // With TZX/TAP's low-level pause semantics and continuity, that first
        // pilot pulse is low.
        assert!(!player.ear_bit());
    }

    #[test]
    fn take_next_raw_block_returns_none_when_position_is_inside_a_turbo_block() {
        // A Turbo Speed Data Block between two standard blocks must not be
        // mistaken for the standard block preceding it.
        let mut bytes = tzx_header();
        bytes.push(0x10); // Standard Speed Data Block
        bytes.extend_from_slice(&0u16.to_le_bytes()); // pause
        bytes.extend_from_slice(&1u16.to_le_bytes()); // length
        bytes.push(0x00); // data

        bytes.push(0x11); // Turbo Speed Data Block
        bytes.extend_from_slice(&2168u16.to_le_bytes()); // pilot pulse
        bytes.extend_from_slice(&667u16.to_le_bytes()); // sync1
        bytes.extend_from_slice(&735u16.to_le_bytes()); // sync2
        bytes.extend_from_slice(&855u16.to_le_bytes()); // bit0
        bytes.extend_from_slice(&1710u16.to_le_bytes()); // bit1
        bytes.extend_from_slice(&100u16.to_le_bytes()); // pilot count
        bytes.push(8); // used bits in last byte
        bytes.extend_from_slice(&0u16.to_le_bytes()); // pause
        bytes.extend_from_slice(&1u32.to_le_bytes()[..3]); // length (24-bit)
        bytes.push(0xAA); // data

        let tape = parse_tzx(&bytes).unwrap();
        let mut player = TapePlayer::new(tape);

        assert_eq!(player.take_next_raw_block(), Some(&[0x00][..]));
        // Playback position now sits right at the turbo block's own first
        // pulse -- not a standard block, so nothing more to trap.
        assert_eq!(player.take_next_raw_block(), None);
    }

    fn tzx_header() -> Vec<u8> {
        let mut bytes = b"ZXTape!\x1a".to_vec();
        bytes.push(1); // major version
        bytes.push(20); // minor version
        bytes
    }

    #[test]
    fn parse_tzx_rejects_a_file_without_the_signature() {
        assert_eq!(parse_tzx(b"not a tzx file"), Err(Error::NotATzxFile));
    }

    #[test]
    fn parse_tzx_standard_speed_block_matches_tap_synthesis() {
        let mut bytes = tzx_header();
        bytes.push(0x10); // Standard Speed Data Block
        bytes.extend_from_slice(&500u16.to_le_bytes()); // pause (ms)
        bytes.extend_from_slice(&1u16.to_le_bytes()); // data length
        bytes.push(0x00); // data: flag byte 0x00 (header)

        let tape = parse_tzx(&bytes).unwrap();

        // Same shape as the equivalent .tap block, but with the block's
        // own pause value instead of the default.
        assert_eq!(tape.events.len(), 8063 + 2 + 16 + 2);
        let pause_cycles = 500u32 * 3_546_900 / 1000;
        assert_eq!(
            *tape.events.last().unwrap(),
            Event::Pulse(Pulse {
                cycles: pause_cycles - ms_to_cycles(1),
                level: false
            })
        );
    }

    /// The body of a `.tzx` Pure Data block (`0x14`): zero bit = 500 T, one
    /// bit = 1000 T, `used` bits in the last byte, then a pause of `pause_ms`.
    fn pure_data_block(data: &[u8], used: u8, pause_ms: u16) -> Vec<u8> {
        let mut bytes = vec![0x14];
        bytes.extend_from_slice(&500u16.to_le_bytes());
        bytes.extend_from_slice(&1000u16.to_le_bytes());
        bytes.push(used);
        bytes.extend_from_slice(&pause_ms.to_le_bytes());
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes()[..3]);
        bytes.extend_from_slice(data);
        bytes
    }

    /// A tape of a single `0x13` pulse (so the data that follows starts on the
    /// *other* level) followed by a Pure Data block.
    fn flipped_pure_data_tape(data: &[u8], used: u8, pause_ms: u16) -> Tape {
        let mut bytes = tzx_header();
        bytes.extend_from_slice(&[0x13, 1]);
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.extend_from_slice(&pure_data_block(data, used, pause_ms));
        parse_tzx(&bytes).unwrap()
    }

    #[test]
    fn a_pause_after_a_high_last_pulse_needs_no_extra_edge() {
        // Data starting on the initial (low) level: 16 pulses, the last one
        // high -- the drop into low silence is already an edge.
        let mut bytes = tzx_header();
        bytes.extend_from_slice(&pure_data_block(&[0x00], 8, 3));
        let tape = parse_tzx(&bytes).unwrap();
        let n = tape.events.len();
        assert_eq!(n, 16 + 1);
        assert_eq!(
            tape.events[n - 2],
            Event::Pulse(Pulse {
                cycles: 500,
                level: true
            })
        );
        assert_eq!(
            tape.events[n - 1],
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(3),
                level: false
            })
        );
    }

    #[test]
    fn a_pause_after_a_low_last_pulse_opens_with_a_one_ms_terminating_edge() {
        // One flipping pulse, then 16 data pulses starting high: the last is
        // low, and low silence after it has no edge to end that pulse.
        let tape = flipped_pure_data_tape(&[0x00], 8, 3);
        let n = tape.events.len();
        assert_eq!(n, 1 + 16 + 2);
        assert_eq!(
            tape.events[n - 3],
            Event::Pulse(Pulse {
                cycles: 500,
                level: false
            }),
            "the last data pulse is low"
        );
        assert_eq!(
            tape.events[n - 2],
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(1),
                level: true
            }),
            "terminating edge"
        );
        assert_eq!(
            tape.events[n - 1],
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(3) - ms_to_cycles(1),
                level: false
            }),
            "then the rest of the silence"
        );
    }

    #[test]
    fn a_pause_of_one_ms_or_less_after_a_low_pulse_is_just_the_edge() {
        let tape = flipped_pure_data_tape(&[0x00], 8, 1);
        assert_eq!(
            *tape.events.last().unwrap(),
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(1),
                level: true
            }),
            "a 1 ms pause is entirely the terminating edge"
        );
        assert_eq!(tape.events.len(), 1 + 16 + 1);
    }

    #[test]
    fn a_standalone_pause_block_after_a_low_pulse_gets_the_terminating_edge_too() {
        let mut bytes = tzx_header();
        bytes.extend_from_slice(&[0x13, 2]); // two pulses: the last is high (level false->true)
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.extend_from_slice(&[0x13, 1]); // third pulse: low, then a pause
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.push(0x20);
        bytes.extend_from_slice(&5u16.to_le_bytes());
        let tape = parse_tzx(&bytes).unwrap();
        let n = tape.events.len();
        assert_eq!(n, 3 + 2);
        assert_eq!(
            tape.events[n - 2],
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(1),
                level: true
            })
        );
        assert_eq!(
            tape.events[n - 1],
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(5) - ms_to_cycles(1),
                level: false
            })
        );
    }

    #[test]
    fn a_leading_silence_is_prepended_unless_the_tape_already_opens_with_one() {
        let bare = flipped_pure_data_tape(&[0x00], 8, 3);
        let first_pulse = bare.events[0];
        let with = bare.clone().with_leading_silence(2000);
        assert_eq!(with.events.len(), bare.events.len() + 1);
        assert_eq!(
            with.events[0],
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(2000),
                level: false
            })
        );
        assert_eq!(with.events[1], first_pulse);
        // 0 ms leaves the tape untouched.
        assert_eq!(bare.clone().with_leading_silence(0), bare);
        // A tape that already opens with a low pause of at least that length
        // is left alone (CPCEC's hold is 2000 ms, so is ours).
        let mut already = tzx_header();
        already.push(0x20);
        already.extend_from_slice(&2500u16.to_le_bytes());
        already.extend_from_slice(&pure_data_block(&[0x00], 8, 3));
        let already = parse_tzx(&already).unwrap();
        assert_eq!(already.clone().with_leading_silence(2000), already);
        // ... while a shorter opening pause still gets the full leader.
        let mut short = tzx_header();
        short.push(0x20);
        short.extend_from_slice(&500u16.to_le_bytes());
        short.extend_from_slice(&pure_data_block(&[0x00], 8, 3));
        let short = parse_tzx(&short).unwrap();
        assert_eq!(
            short.clone().with_leading_silence(2000).events.len(),
            short.events.len() + 1
        );
    }

    #[test]
    fn a_leading_silence_shifts_the_standard_block_ranges_the_fast_load_trap_uses() {
        // Two standard blocks (.tap): the fast-load trap indexes them by
        // `events` position, so the inserted event must move every range.
        let bytes = [0x01, 0x00, 0x00, 0x01, 0x00, 0xFF];
        let tape = parse_tap(&bytes).unwrap();
        let shifted = tape.clone().with_leading_silence(1500);
        assert_eq!(shifted.block_starts.len(), tape.block_starts.len());
        for (a, b) in shifted.block_starts.iter().zip(&tape.block_starts) {
            assert_eq!(*a, *b + 1);
        }
        for (a, b) in shifted.block_ends.iter().zip(&tape.block_ends) {
            assert_eq!(*a, *b + 1);
        }
        assert_eq!(shifted.blocks, tape.blocks);
    }

    #[test]
    fn parse_arkanoid_cdt_keeps_long_turbo_pause_low_level() {
        // The tape lives in the Amstrad repository's `software/cpc`; set
        // ARKANOID_CDT to its path to run this check (skipped otherwise).
        let Some(path) = std::env::var_os("ARKANOID_CDT") else {
            eprintln!("skipped: ARKANOID_CDT is not set");
            return;
        };
        let bytes = std::fs::read(path).expect("ARKANOID_CDT is readable");
        let tape = parse_tzx(&bytes).unwrap();

        // Arkanoid's second turbo block has a 10.110s inter-block pause. A TZX
        // pause is a low-level silence interval; this block's last data pulse
        // is low, so it opens with the 1 ms edge that terminates that pulse.
        let silence = ms_to_cycles(10_110) - ms_to_cycles(1);
        let at = tape
            .events
            .iter()
            .position(|e| {
                *e == Event::Pulse(Pulse {
                    cycles: silence,
                    level: false,
                })
            })
            .expect("the 10.110 s silence");
        assert_eq!(
            tape.events[at - 1],
            Event::Pulse(Pulse {
                cycles: ms_to_cycles(1),
                level: true
            }),
            "terminating edge before the silence"
        );
        assert!(matches!(
            tape.events[at - 2],
            Event::Pulse(Pulse { level: false, .. })
        ));
        assert!(matches!(tape.events[at + 1], Event::Pulse(_)));
    }

    #[test]
    fn parse_tzx_turbo_speed_block_uses_the_blocks_own_timing_and_used_bits() {
        let mut bytes = tzx_header();
        bytes.push(0x11); // Turbo Speed Data Block
        bytes.extend_from_slice(&1000u16.to_le_bytes()); // pilot pulse
        bytes.extend_from_slice(&600u16.to_le_bytes()); // sync1
        bytes.extend_from_slice(&700u16.to_le_bytes()); // sync2
        bytes.extend_from_slice(&500u16.to_le_bytes()); // bit0
        bytes.extend_from_slice(&1000u16.to_le_bytes()); // bit1
        bytes.extend_from_slice(&100u16.to_le_bytes()); // pilot pulse count
        bytes.push(3); // used bits in last byte
        bytes.extend_from_slice(&0u16.to_le_bytes()); // pause (ms)
        bytes.extend_from_slice(&[1, 0, 0]); // data length (3 bytes, LE)
        bytes.push(0xFF); // 1 data byte, only 3 bits used

        let tape = parse_tzx(&bytes).unwrap();

        // 100 pilot + 2 sync + 3 bits (used_bits_in_last_byte) * 2 pulses.
        assert_eq!(tape.events.len(), 100 + 2 + 6);
        assert_eq!(
            tape.events[0],
            Event::Pulse(Pulse {
                cycles: 1000,
                level: false
            })
        );
    }

    #[test]
    fn parse_tzx_pure_tone_block_emits_fixed_width_alternating_pulses() {
        let mut bytes = tzx_header();
        bytes.push(0x12); // Pure Tone
        bytes.extend_from_slice(&900u16.to_le_bytes()); // pulse length
        bytes.extend_from_slice(&3u16.to_le_bytes()); // pulse count

        let tape = parse_tzx(&bytes).unwrap();

        assert_eq!(tape.event_count(), 3);
        assert_eq!(
            tape.events,
            vec![
                Event::Pulse(Pulse {
                    cycles: 900,
                    level: false
                }),
                Event::Pulse(Pulse {
                    cycles: 900,
                    level: true
                }),
                Event::Pulse(Pulse {
                    cycles: 900,
                    level: false
                }),
            ]
        );
    }

    #[test]
    fn parse_tzx_pulse_sequence_block_uses_each_declared_pulse_duration() {
        let mut bytes = tzx_header();
        bytes.push(0x13); // Pulse Sequence
        bytes.push(3); // number of pulses
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.extend_from_slice(&200u16.to_le_bytes());
        bytes.extend_from_slice(&300u16.to_le_bytes());

        let tape = parse_tzx(&bytes).unwrap();

        assert_eq!(
            tape.events,
            vec![
                Event::Pulse(Pulse {
                    cycles: 100,
                    level: false
                }),
                Event::Pulse(Pulse {
                    cycles: 200,
                    level: true
                }),
                Event::Pulse(Pulse {
                    cycles: 300,
                    level: false
                }),
            ]
        );
    }

    #[test]
    fn parse_tzx_pure_data_block_encodes_bits_without_pilot_or_sync() {
        let mut bytes = tzx_header();
        bytes.push(0x14); // Pure Data Block
        bytes.extend_from_slice(&500u16.to_le_bytes()); // bit-0 pulse
        bytes.extend_from_slice(&1000u16.to_le_bytes()); // bit-1 pulse
        bytes.push(3); // use only top 3 bits from last byte
        bytes.extend_from_slice(&250u16.to_le_bytes()); // pause (ms)
        bytes.extend_from_slice(&[1, 0, 0]); // data length (24-bit LE)
        bytes.push(0xA0); // 101xxxxx (3 used bits => 1,0,1)

        let tape = parse_tzx(&bytes).unwrap();

        let pause_cycles = 250u32 * 3_546_900 / 1000;
        assert_eq!(tape.event_count(), 3 * 2 + 1);
        assert_eq!(
            tape.events,
            vec![
                Event::Pulse(Pulse {
                    cycles: 1000,
                    level: false
                }),
                Event::Pulse(Pulse {
                    cycles: 1000,
                    level: true
                }),
                Event::Pulse(Pulse {
                    cycles: 500,
                    level: false
                }),
                Event::Pulse(Pulse {
                    cycles: 500,
                    level: true
                }),
                Event::Pulse(Pulse {
                    cycles: 1000,
                    level: false
                }),
                Event::Pulse(Pulse {
                    cycles: 1000,
                    level: true
                }),
                Event::Pulse(Pulse {
                    cycles: pause_cycles,
                    level: false
                }),
            ]
        );
    }

    #[test]
    fn parse_tzx_pause_block_with_nonzero_duration_is_a_low_pulse() {
        let mut bytes = tzx_header();
        bytes.push(0x20); // Pause/Stop the tape
        bytes.extend_from_slice(&2000u16.to_le_bytes()); // 2000ms, not a stop

        let tape = parse_tzx(&bytes).unwrap();

        assert_eq!(tape.events.len(), 1);
        let expected_cycles = (2000u64 * 3_546_900 / 1000) as u32;
        assert_eq!(
            tape.events[0],
            Event::Pulse(Pulse {
                cycles: expected_cycles,
                level: false
            })
        );
    }

    #[test]
    fn parse_tzx_turbo_block_with_zero_pause_adds_no_zero_duration_event() {
        let mut bytes = tzx_header();
        bytes.push(0x11); // Turbo Speed Data Block
        bytes.extend_from_slice(&1000u16.to_le_bytes()); // pilot pulse
        bytes.extend_from_slice(&600u16.to_le_bytes()); // sync1
        bytes.extend_from_slice(&700u16.to_le_bytes()); // sync2
        bytes.extend_from_slice(&500u16.to_le_bytes()); // bit0
        bytes.extend_from_slice(&1000u16.to_le_bytes()); // bit1
        bytes.extend_from_slice(&100u16.to_le_bytes()); // pilot pulse count
        bytes.push(8); // all bits used in the final byte
        bytes.extend_from_slice(&0u16.to_le_bytes()); // pause (ms)
        bytes.extend_from_slice(&[1, 0, 0]); // data length (24-bit LE)
        bytes.push(0x00);

        let tape = parse_tzx(&bytes).unwrap();

        // 100 pilot + 2 sync + 8 bits * 2 pulses; no trailing pause event.
        assert_eq!(tape.event_count(), 100 + 2 + 16);
    }
    #[test]
    fn parse_tzx_pause_block_with_zero_duration_is_a_stop_event() {
        let mut bytes = tzx_header();
        bytes.push(0x20);
        bytes.extend_from_slice(&0u16.to_le_bytes());

        let tape = parse_tzx(&bytes).unwrap();

        assert_eq!(tape.events, vec![Event::Stop]);
    }

    #[test]
    fn parse_tzx_skips_group_start_and_end_blocks() {
        let mut bytes = tzx_header();
        bytes.push(0x21); // Group start
        bytes.push(3); // name length
        bytes.extend_from_slice(b"abc");
        bytes.push(0x22); // Group end (no fields)

        let tape = parse_tzx(&bytes).unwrap();

        assert_eq!(tape.events.len(), 0);
    }

    #[test]
    fn parse_tzx_errors_clearly_on_an_unimplemented_block_type() {
        let mut bytes = tzx_header();
        // Custom info: its identification-string length is disputed between
        // reference parsers, so it must error rather than be skipped by a guess.
        bytes.push(0x35);

        assert_eq!(parse_tzx(&bytes), Err(Error::UnsupportedBlockType(0x35)));
    }

    #[test]
    fn parse_tzx_skips_informational_blocks_without_producing_pulses() {
        let mut bytes = tzx_header();
        bytes.push(0x33); // Hardware type: 2 entries of (type, id, info)
        bytes.push(2);
        bytes.extend_from_slice(&[0, 1, 1, 1, 7, 0]);
        bytes.push(0x30); // Text description
        bytes.push(5);
        bytes.extend_from_slice(b"hello");
        bytes.push(0x31); // Message block: time, length, text
        bytes.push(3);
        bytes.push(2);
        bytes.extend_from_slice(b"hi");
        bytes.push(0x32); // Archive info: u16 length + data
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 1, 0, 0]);
        bytes.push(0x5A); // Glue block
        bytes.extend_from_slice(b"XTape!\x1a\x01\x14");
        // A real block after them proves the skips landed exactly on its ID.
        bytes.push(0x20);
        bytes.extend_from_slice(&100u16.to_le_bytes());

        let tape = parse_tzx(&bytes).unwrap();

        assert_eq!(
            tape.events.len(),
            1,
            "only the pause block produced an event"
        );
    }

    #[test]
    fn parse_tzx_rejects_a_truncated_informational_block() {
        for body in [
            vec![0x30, 9, b'a'],    // text shorter than its length
            vec![0x31, 1, 9, b'a'], // message shorter than its length
            vec![0x32, 9, 0, 1],    // archive info shorter than its length
            vec![0x33, 3, 0, 0, 0], // fewer entries than the count
            vec![0x5A, b'X'],       // glue block cut short
        ] {
            let mut bytes = tzx_header();
            bytes.extend_from_slice(&body);
            assert_eq!(parse_tzx(&bytes), Err(Error::TruncatedBlock), "{body:02X?}");
        }
    }

    fn pzxt_header() -> Vec<u8> {
        let mut bytes = b"PZXT".to_vec();
        bytes.extend_from_slice(&4u32.to_le_bytes()); // size: major+minor+2 reserved-ish info bytes
        bytes.push(1); // major
        bytes.push(0); // minor
        bytes.push(0); // (no info strings)
        bytes.push(0);
        bytes
    }

    fn push_pzx_block(bytes: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        bytes.extend_from_slice(tag);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
    }

    #[test]
    fn parse_pzx_rejects_a_file_without_the_pzxt_header() {
        assert_eq!(parse_pzx(&[0u8; 8]), Err(Error::NotAPzxFile));
    }

    #[test]
    fn parse_pzx_puls_block_decodes_plain_pulses_without_repeat_count() {
        let mut bytes = pzxt_header();
        let mut data = Vec::new();
        data.extend_from_slice(&2168u16.to_le_bytes());
        data.extend_from_slice(&667u16.to_le_bytes());
        push_pzx_block(&mut bytes, b"PULS", &data);

        let tape = parse_pzx(&bytes).unwrap();

        assert_eq!(
            tape.events,
            vec![
                Event::Pulse(Pulse {
                    cycles: 2168,
                    level: false
                }),
                Event::Pulse(Pulse {
                    cycles: 667,
                    level: true
                }),
            ]
        );
    }

    #[test]
    fn parse_pzx_puls_block_honors_an_explicit_repeat_count() {
        let mut bytes = pzxt_header();
        let mut data = Vec::new();
        data.extend_from_slice(&(0x8000u16 | 3).to_le_bytes()); // repeat count = 3
        data.extend_from_slice(&100u16.to_le_bytes());
        push_pzx_block(&mut bytes, b"PULS", &data);

        let tape = parse_pzx(&bytes).unwrap();

        assert_eq!(
            tape.events,
            vec![
                Event::Pulse(Pulse {
                    cycles: 100,
                    level: false
                }),
                Event::Pulse(Pulse {
                    cycles: 100,
                    level: true
                }),
                Event::Pulse(Pulse {
                    cycles: 100,
                    level: false
                }),
            ]
        );
    }

    #[test]
    fn parse_pzx_data_block_encodes_one_bit_via_the_standard_sequences() {
        let mut bytes = pzxt_header();
        let mut data = Vec::new();
        // count: 1 bit, initial level high (bit 31 set).
        data.extend_from_slice(&(0x8000_0001u32).to_le_bytes());
        data.extend_from_slice(&945u16.to_le_bytes()); // tail
        data.push(2); // p0
        data.push(2); // p1
        data.extend_from_slice(&855u16.to_le_bytes()); // s0[0]
        data.extend_from_slice(&855u16.to_le_bytes()); // s0[1]
        data.extend_from_slice(&1710u16.to_le_bytes()); // s1[0]
        data.extend_from_slice(&1710u16.to_le_bytes()); // s1[1]
        data.push(0x80); // one bit, value 1 (MSB set)
        push_pzx_block(&mut bytes, b"DATA", &data);

        let tape = parse_pzx(&bytes).unwrap();

        assert_eq!(
            tape.events,
            vec![
                Event::Pulse(Pulse {
                    cycles: 1710,
                    level: true
                }),
                Event::Pulse(Pulse {
                    cycles: 1710,
                    level: false
                }),
                Event::Pulse(Pulse {
                    cycles: 945,
                    level: true
                }),
            ]
        );
    }

    #[test]
    fn parse_pzx_paus_block_decodes_duration_and_level() {
        let mut bytes = pzxt_header();
        let raw = 0x8000_0064u32; // duration=100, level high
        push_pzx_block(&mut bytes, b"PAUS", &raw.to_le_bytes());

        let tape = parse_pzx(&bytes).unwrap();

        assert_eq!(
            tape.events,
            vec![Event::Pulse(Pulse {
                cycles: 100,
                level: true
            })]
        );
    }

    #[test]
    fn parse_pzx_stop_block_becomes_a_stop_event() {
        let mut bytes = pzxt_header();
        push_pzx_block(&mut bytes, b"STOP", &0u16.to_le_bytes());

        let tape = parse_pzx(&bytes).unwrap();

        assert_eq!(tape.events, vec![Event::Stop]);
    }

    #[test]
    fn parse_pzx_skips_an_unrecognized_block_without_erroring() {
        let mut bytes = pzxt_header();
        push_pzx_block(&mut bytes, b"XYZZ", &[0xDE, 0xAD, 0xBE, 0xEF]);
        push_pzx_block(&mut bytes, b"PAUS", &0x0000_0064u32.to_le_bytes());

        let tape = parse_pzx(&bytes).unwrap();

        assert_eq!(
            tape.events,
            vec![Event::Pulse(Pulse {
                cycles: 100,
                level: false
            })]
        );
    }

    #[test]
    fn parse_pzx_does_not_populate_raw_blocks_for_the_fast_load_trap() {
        let mut bytes = pzxt_header();
        push_pzx_block(&mut bytes, b"PAUS", &0x0000_0064u32.to_le_bytes());

        let tape = parse_pzx(&bytes).unwrap();
        let mut player = TapePlayer::new(tape);

        assert_eq!(player.take_next_raw_block(), None);
    }

    fn csw_v1_header(sample_rate: u16, compression_type: u8, flags: u8) -> Vec<u8> {
        let mut bytes = b"Compressed Square Wave".to_vec();
        assert_eq!(bytes.len(), 22);
        bytes.push(0x1A);
        bytes.push(1); // major
        bytes.push(1); // minor
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.push(compression_type);
        bytes.push(flags);
        bytes.extend_from_slice(&[0, 0, 0]); // reserved
        assert_eq!(bytes.len(), 32);
        bytes
    }

    fn csw_v2_header(sample_rate: u32, compression_type: u8, flags: u8) -> Vec<u8> {
        let mut bytes = b"Compressed Square Wave".to_vec();
        bytes.push(0x1A);
        bytes.push(2); // major
        bytes.push(0); // minor
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // total pulses, unused by parse_csw
        bytes.push(compression_type);
        bytes.push(flags);
        bytes.push(0); // no header extension
        bytes.extend_from_slice(&[0u8; 16]); // encoding application, unused
        assert_eq!(bytes.len(), 52);
        bytes
    }

    #[test]
    fn parse_csw_rejects_a_file_without_the_signature() {
        let bytes = b"not a csw file at all".to_vec();
        assert_eq!(parse_csw(&bytes), Err(Error::NotACswFile));
    }

    #[test]
    fn parse_csw_v1_decodes_rle_pulses_starting_high() {
        let mut bytes = csw_v1_header(44100, 1, 0x01); // flags bit0 = initial polarity high
        bytes.extend_from_slice(&[10, 20]);

        let tape = parse_csw(&bytes).unwrap();

        assert_eq!(
            tape.events,
            vec![
                Event::Pulse(Pulse {
                    cycles: (10u64 * CSW_CLOCK_HZ / 44100) as u32,
                    level: true,
                }),
                Event::Pulse(Pulse {
                    cycles: (20u64 * CSW_CLOCK_HZ / 44100) as u32,
                    level: false,
                }),
            ]
        );
    }

    #[test]
    fn parse_csw_v1_decodes_the_extended_duration_escape_byte() {
        let mut bytes = csw_v1_header(44100, 1, 0x00);
        bytes.push(0x00);
        bytes.extend_from_slice(&300u32.to_le_bytes());

        let tape = parse_csw(&bytes).unwrap();

        assert_eq!(
            tape.events,
            vec![Event::Pulse(Pulse {
                cycles: (300u64 * CSW_CLOCK_HZ / 44100) as u32,
                level: false,
            })]
        );
    }

    #[test]
    fn parse_csw_v2_rle_decodes_pulses() {
        let mut bytes = csw_v2_header(44100, 1, 0x00);
        bytes.extend_from_slice(&[5, 15, 25]);

        let tape = parse_csw(&bytes).unwrap();

        assert_eq!(tape.events.len(), 3);
        assert_eq!(
            tape.events[0],
            Event::Pulse(Pulse {
                cycles: 402,
                level: false
            })
        );
    }

    #[test]
    fn parse_csw_v2_z_rle_inflates_zlib_compressed_pulses() {
        use std::io::Write;
        let raw_pulses = [7u8, 14, 21, 28];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&raw_pulses).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut bytes = csw_v2_header(44100, 2, 0x00);
        bytes.extend_from_slice(&compressed);

        let tape = parse_csw(&bytes).unwrap();

        assert_eq!(tape.events.len(), raw_pulses.len());
        let mut level = false;
        for (event, &duration) in tape.events.iter().zip(raw_pulses.iter()) {
            assert_eq!(
                *event,
                Event::Pulse(Pulse {
                    cycles: (duration as u64 * CSW_CLOCK_HZ / 44100) as u32,
                    level,
                })
            );
            level = !level;
        }
    }

    #[test]
    fn parse_csw_rejects_an_unsupported_major_version() {
        let mut bytes = b"Compressed Square Wave".to_vec();
        bytes.push(0x1A);
        bytes.push(9);
        bytes.push(0);
        assert_eq!(parse_csw(&bytes), Err(Error::UnsupportedCswVersion(9)));
    }

    #[test]
    fn parse_csw_rejects_an_unsupported_compression_type() {
        let bytes = csw_v1_header(44100, 3, 0x00);
        assert_eq!(parse_csw(&bytes), Err(Error::UnsupportedCswCompression(3)));
    }

    #[test]
    fn parse_csw_does_not_populate_raw_blocks_for_the_fast_load_trap() {
        let mut bytes = csw_v1_header(44100, 1, 0x00);
        bytes.extend_from_slice(&[10]);

        let tape = parse_csw(&bytes).unwrap();
        let mut player = TapePlayer::new(tape);

        assert_eq!(player.take_next_raw_block(), None);
    }

    #[test]
    fn dump_state_then_restore_state_reproduces_playback_position() {
        let tape = Tape {
            events: vec![
                Event::Pulse(Pulse {
                    cycles: 10,
                    level: true,
                }),
                Event::Pulse(Pulse {
                    cycles: 20,
                    level: false,
                }),
            ],
            ..Default::default()
        };
        let mut player = TapePlayer::new(tape.clone());
        player.play();
        player.tick(10); // consumes the first pulse exactly, lands on the second

        let dumped = player.dump_state();
        assert_eq!(dumped.speed_multiplier, 1); // the field itself round-trips too

        let mut restored = TapePlayer::new(tape);
        restored.set_speed_multiplier(9); // must be overwritten by restore_state below
        restored.restore_state(dumped);

        assert!(!restored.ear_bit()); // on the second (level=false) pulse
        restored.tick(20); // exactly consumes it -- should finish
        assert!(restored.finished());
    }

    #[test]
    fn restore_state_clamps_a_position_past_the_loaded_tapes_own_length() {
        let long_tape = Tape {
            events: vec![
                Event::Pulse(Pulse {
                    cycles: 1,
                    level: true,
                });
                10
            ],
            ..Default::default()
        };
        let mut player = TapePlayer::new(long_tape);
        player.play();
        player.tick(5); // position = 5, well within this tape

        let short_tape = Tape {
            events: vec![Event::Pulse(Pulse {
                cycles: 1,
                level: true,
            })],
            ..Default::default()
        };
        let mut restored = TapePlayer::new(short_tape);
        restored.restore_state(player.dump_state()); // must not panic

        assert!(restored.finished()); // clamped to this (shorter) tape's own length
    }

    #[test]
    fn cycles_until_next_edge_tracks_same_level_run_and_partial_progress() {
        let tape = Tape {
            events: vec![
                Event::Pulse(Pulse {
                    cycles: 10,
                    level: false,
                }),
                Event::Pulse(Pulse {
                    cycles: 20,
                    level: false,
                }),
                Event::Pulse(Pulse {
                    cycles: 30,
                    level: true,
                }),
            ],
            ..Default::default()
        };
        let mut player = TapePlayer::new(tape);

        assert_eq!(player.cycles_until_next_edge(), None);

        player.play();
        assert_eq!(player.cycles_until_next_edge(), Some(30));

        player.tick(7);
        assert_eq!(player.cycles_until_next_edge(), Some(23));
    }

    #[test]
    fn parse_voc_header_and_one_pulse() {
        // Build minimal VOC file with header + one 0x01 Sound Data block containing 2 low + 2 high samples
        let mut voc = Vec::new();

        // Header: "Creative Voice File\x1A" signature (20 bytes)
        voc.extend_from_slice(b"Creative Voice File\x1A");
        // Data block offset (u16 LE): 26
        voc.extend_from_slice(&26u16.to_le_bytes());
        // Version (u16 LE): 0x0100
        voc.extend_from_slice(&0x0100u16.to_le_bytes());
        // Checksum (u16 LE): 0x1234 - 0x0100 = 0x1134
        voc.extend_from_slice(&0x1134u16.to_le_bytes());

        // Block type 0x01: Sound Data
        voc.push(0x01);
        // Freq divisor: 231 (rate = 1_000_000 / (256 - 231) = 40,000 Hz)
        voc.push(231);
        // Codec: 0x00 (8-bit unsigned PCM)
        voc.push(0x00);
        // Data length (u24 LE): 4 bytes
        voc.extend_from_slice(&4u32.to_le_bytes()[0..3]);
        // Audio data: 2 low (0x00) + 2 high (0xFF)
        voc.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);

        // Terminator: 0x00
        voc.push(0x00);

        let tape = match parse_voc(&voc) {
            Ok(t) => t,
            Err(e) => panic!("parse_voc error: {:?}, voc.len()={}", e, voc.len()),
        };
        assert_eq!(tape.event_count(), 2, "event count mismatch");
        // Should have 2 pulses: one for low level, one for high level
    }

    #[test]
    fn parse_voc_rejects_bad_checksum() {
        let mut voc = Vec::new();
        voc.extend_from_slice(b"Creative Voice File\x1A");
        voc.extend_from_slice(&26u16.to_le_bytes());
        voc.extend_from_slice(&0x0100u16.to_le_bytes());
        voc.extend_from_slice(&0xBADBu16.to_le_bytes()); // Wrong checksum
        voc.push(0x00); // Terminator

        assert_eq!(parse_voc(&voc), Err(Error::NotAVocFile));
    }

    #[test]
    fn parse_voc_rejects_unsupported_codec() {
        let mut voc = Vec::new();
        voc.extend_from_slice(b"Creative Voice File\x1A");
        voc.extend_from_slice(&26u16.to_le_bytes());
        voc.extend_from_slice(&0x0100u16.to_le_bytes());
        voc.extend_from_slice(&0x1134u16.to_le_bytes());
        // Block type 0x01 with ADPCM codec (0x01)
        voc.push(0x01);
        voc.push(231); // freq_divisor
        voc.push(0x01); // codec: ADPCM (unsupported)
        voc.extend_from_slice(&0u32.to_le_bytes()[0..3]); // data len = 0
        voc.push(0x00); // Terminator

        assert_eq!(parse_voc(&voc), Err(Error::UnsupportedVocCodec(0x01)));
    }

    #[test]
    fn parse_voc_silence_emits_long_pulse() {
        let mut voc = Vec::new();
        voc.extend_from_slice(b"Creative Voice File\x1A");
        voc.extend_from_slice(&26u16.to_le_bytes());
        voc.extend_from_slice(&0x0100u16.to_le_bytes());
        voc.extend_from_slice(&0x1134u16.to_le_bytes());
        // Block type 0x03: Silence
        voc.push(0x03);
        // Silence length (u16 LE): 99 (means 100 samples including +1)
        voc.extend_from_slice(&99u16.to_le_bytes());
        // Freq divisor: 231
        voc.push(231);
        voc.push(0x00); // Terminator

        let tape = parse_voc(&voc).expect("parse_voc silence failed");
        assert_eq!(tape.event_count(), 1);
        // Should have one pulse representing the silence
    }

    #[test]
    fn parse_voc_repeat_unrolls() {
        let mut voc = Vec::new();
        voc.extend_from_slice(b"Creative Voice File\x1A");
        voc.extend_from_slice(&26u16.to_le_bytes());
        voc.extend_from_slice(&0x0100u16.to_le_bytes());
        voc.extend_from_slice(&0x1134u16.to_le_bytes());

        // Block 0x06: Repeat Start with count=1 (total 2 times)
        voc.push(0x06);
        voc.extend_from_slice(&1u16.to_le_bytes());

        // Block 0x01: Sound Data with 2 samples (low)
        voc.push(0x01);
        voc.push(231); // freq_divisor
        voc.push(0x00); // codec
        voc.extend_from_slice(&2u32.to_le_bytes()[0..3]);
        voc.extend_from_slice(&[0x00, 0x00]);

        // Block 0x07: Repeat End
        voc.push(0x07);

        // Block 0x00: Terminator
        voc.push(0x00);

        let tape = parse_voc(&voc).expect("parse_voc repeat failed");
        // Should have pulse repeated 2 times (original + 1 repeat)
        assert!(tape.event_count() >= 1);
    }

    #[test]
    fn parse_voc_extra_info_changes_rate() {
        let mut voc = Vec::new();
        voc.extend_from_slice(b"Creative Voice File\x1A");
        voc.extend_from_slice(&26u16.to_le_bytes());
        voc.extend_from_slice(&0x0100u16.to_le_bytes());
        voc.extend_from_slice(&0x1134u16.to_le_bytes());

        // Block 0x08: Extra Info
        voc.push(0x08);
        // Freq divisor (u16 LE): 65536 - 6250 = 59286 (rate = 256_000_000 / (1 * 6250) = 40,960 Hz)
        voc.extend_from_slice(&59286u16.to_le_bytes());
        // Codec: 0x00 (8-bit unsigned PCM)
        voc.push(0x00);
        // Channels minus 1: 0 (mono)
        voc.push(0x00);

        // Block 0x01: Sound Data
        voc.push(0x01);
        voc.push(231); // freq_divisor (ignored, uses 0x08's rate)
        voc.push(0x00); // codec
        voc.extend_from_slice(&2u32.to_le_bytes()[0..3]);
        voc.extend_from_slice(&[0x00, 0xFF]);

        voc.push(0x00); // Terminator

        let tape = parse_voc(&voc).expect("parse_voc extra_info failed");
        assert!(tape.event_count() > 0);
    }

    fn pulses_of(tape: &Tape) -> Vec<(u32, bool)> {
        tape.events()
            .iter()
            .filter_map(|event| match event {
                Event::Pulse(pulse) => Some((pulse.cycles, pulse.level)),
                Event::Stop => None,
            })
            .collect()
    }

    #[test]
    fn write_tzx_round_trips_alternating_pulses() {
        let recorded: Vec<Pulse> = (0..600u32)
            .map(|i| Pulse {
                cycles: 800 + (i % 7) * 100,
                level: i % 2 == 0,
            })
            .collect();
        let tape = Tape::from_pulses(recorded.clone());
        let bytes = write_tzx(&tape);
        assert_eq!(&bytes[..8], b"ZXTape!\x1a");
        let parsed = parse_tzx(&bytes).expect("our own TZX parses");
        // 600 pulses need 3 blocks of at most 255.
        assert_eq!(
            pulses_of(&parsed),
            recorded
                .iter()
                .map(|p| (p.cycles, p.level))
                .collect::<Vec<_>>()
                // The first recorded pulse is high, the parser starts low, so
                // a 1 T-state low pulse is inserted to flip the line.
                .into_iter()
                .fold(vec![(1, false)], |mut acc, pulse| {
                    acc.push(pulse);
                    acc
                })
        );
    }

    #[test]
    fn write_tzx_turns_long_silences_into_pauses_and_realigns_the_level() {
        let tape = Tape::from_pulses([
            Pulse {
                cycles: 1000,
                level: false,
            },
            Pulse {
                cycles: 1200,
                level: true,
            },
            // 2 seconds of silence held low, then the next burst starts high.
            Pulse {
                cycles: 7_093_800,
                level: false,
            },
            Pulse {
                cycles: 900,
                level: true,
            },
            Pulse {
                cycles: 900,
                level: false,
            },
        ]);
        let parsed = parse_tzx(&write_tzx(&tape)).unwrap();
        let pulses = pulses_of(&parsed);
        assert_eq!(&pulses[..2], &[(1000, false), (1200, true)]);
        // The pause is 2000 ms exactly, low.
        assert_eq!(pulses[2], (7_093_800, false));
        // After a pause the parser is low, so the high burst needs a flip.
        assert_eq!(&pulses[3..], &[(1, false), (900, true), (900, false)]);
    }

    #[test]
    fn from_pulses_drops_zero_length_pulses() {
        let tape = Tape::from_pulses([
            Pulse {
                cycles: 0,
                level: true,
            },
            Pulse {
                cycles: 5,
                level: true,
            },
        ]);
        assert_eq!(tape.event_count(), 1);
    }

    #[test]
    fn write_voc_round_trip() {
        // Create a simple tape with one pulse
        let original_tape = Tape {
            events: vec![
                Event::Pulse(Pulse {
                    cycles: 3_546_900, // 1 second at Z80 clock
                    level: true,
                }),
                Event::Pulse(Pulse {
                    cycles: 3_546_900,
                    level: false,
                }),
            ],
            ..Default::default()
        };

        // Write to VOC at 44,100 Hz
        let voc_bytes = write_voc(&original_tape, 44100).expect("write_voc failed");

        // Parse it back
        let parsed_tape = parse_voc(&voc_bytes).expect("parse_voc failed");

        // Should have same event count
        assert_eq!(parsed_tape.event_count(), original_tape.event_count());
    }

    /// The header's 8-bit rate divisor only encodes whole-microsecond sample
    /// periods, so the writer must generate samples at *that* rate: the total
    /// playing time has to survive a write/parse round trip to within a
    /// sample or two, at every common rate (it used to come back ~3% short
    /// at 44.1 kHz and ~4% at 96 kHz).
    #[test]
    fn write_voc_round_trip_preserves_total_time_at_common_rates() {
        let pulses = (0..2000).map(|i| Pulse {
            cycles: if i % 2 == 0 { 2440 } else { 1281 },
            level: i % 2 == 0,
        });
        let original = Tape::from_pulses(pulses);
        for rate in [22_050u32, 44_100, 48_000, 96_000, 192_000] {
            let voc = write_voc(&original, rate).expect("write");
            let parsed = parse_voc(&voc).expect("parse");
            let want = original.total_cycles() as f64;
            let got = parsed.total_cycles() as f64;
            let error = (got - want).abs() / want;
            assert!(
                error < 0.002,
                "{rate} Hz: {got} vs {want} ({:.2}% off)",
                error * 100.0
            );
        }
    }

    #[test]
    fn rescaled_converts_pulse_lengths_and_keeps_stops_and_blocks() {
        let tape = Tape::from_pulses([
            Pulse {
                cycles: 3_500,
                level: true,
            },
            Pulse {
                cycles: 7_000,
                level: false,
            },
            Pulse {
                cycles: 1,
                level: true,
            },
        ]);
        let scaled = tape.rescaled(3_500_000, 3_250_000);
        // 3500 -> 3250, 7000 -> 6500, and a 1-cycle pulse stays at least 1.
        assert_eq!(scaled.total_cycles(), 3_250 + 6_500 + 1);
        assert_eq!(scaled.event_count(), 3);
        // Scaling by the identity changes nothing.
        assert_eq!(tape.rescaled(1, 1), tape);
        // Rounds to the nearest cycle.
        let odd = Tape::from_pulses([Pulse {
            cycles: 3,
            level: true,
        }]);
        assert_eq!(odd.rescaled(2, 1).total_cycles(), 2); // 1.5 rounds up
    }

    /// A `.wav` of `samples` (already encoded) with the given format fields.
    fn wav_bytes(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits / 8)).to_le_bytes());
        fmt.extend_from_slice(&(channels * (bits / 8)).to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&((4 + 8 + fmt.len() + 8 + data.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    /// 16-bit mono samples: `runs` of `(length in samples, level)`.
    fn square_16(runs: &[(usize, bool)]) -> Vec<u8> {
        runs.iter()
            .flat_map(|&(n, high)| {
                let v: i16 = if high { 20_000 } else { -20_000 };
                std::iter::repeat_n(v.to_le_bytes(), n).flatten()
            })
            .collect()
    }

    #[test]
    fn a_16_bit_mono_wav_becomes_pulses_in_the_spectrum_clock() {
        // 441 samples high, 441 low, 882 high at 44.1 kHz: 10 ms, 10 ms, 20 ms.
        let data = square_16(&[(441, true), (441, false), (882, true)]);
        let tape = parse_wav(&wav_bytes(1, 1, 44_100, 16, &data)).unwrap();
        let cycles = |ms: u64| (ms * 3_546_900 / 1000) as u32;
        assert_eq!(
            pulses_of(&tape),
            vec![(cycles(10), true), (cycles(10), false), (cycles(20), true)]
        );
        assert_eq!(tape.blocks.len(), 0, "raw audio has no ROM-block structure");
    }

    #[test]
    fn eight_bit_stereo_and_float_wavs_decode_to_the_same_pulses() {
        let want = vec![(3_546_900 / 100, true), (3_546_900 / 100, false)]; // 10 ms each at 44.1 kHz? no: see below
        // 100 samples at 10 kHz = 10 ms: high then low.
        let rate = 10_000;
        let eight: Vec<u8> = (0..200).map(|i| if i < 100 { 230 } else { 26 }).collect();
        let t8 = parse_wav(&wav_bytes(1, 1, rate, 8, &eight)).unwrap();
        assert_eq!(pulses_of(&t8), want);
        // Stereo 16-bit: the first channel carries the signal, the second is loud noise that must be ignored.
        let stereo: Vec<u8> = (0..200)
            .flat_map(|i| {
                let left: i16 = if i < 100 { 12_000 } else { -12_000 };
                let right: i16 = if i % 2 == 0 { 30_000 } else { -30_000 };
                [left.to_le_bytes(), right.to_le_bytes()].concat()
            })
            .collect();
        let ts = parse_wav(&wav_bytes(1, 2, rate, 16, &stereo)).unwrap();
        assert_eq!(pulses_of(&ts), want);
        // 32-bit float.
        let float: Vec<u8> = (0..200)
            .flat_map(|i| (if i < 100 { 0.5f32 } else { -0.5f32 }).to_le_bytes())
            .collect();
        let tf = parse_wav(&wav_bytes(3, 1, rate, 32, &float)).unwrap();
        assert_eq!(pulses_of(&tf), want);
        // 24-bit.
        let i24: Vec<u8> = (0..200)
            .flat_map(|i| {
                let v: i32 = if i < 100 { 4_000_000 } else { -4_000_000 };
                v.to_le_bytes()[..3].to_vec()
            })
            .collect();
        let t24 = parse_wav(&wav_bytes(1, 1, rate, 24, &i24)).unwrap();
        assert_eq!(pulses_of(&t24), want);
    }

    #[test]
    fn noise_around_zero_does_not_make_edges_but_a_sine_gives_one_cycle_per_period() {
        // A square wave of +-20000 with +-1500 of wobble stays one clean pulse each way (the trigger is 10% of the peak).
        let noisy: Vec<u8> = (0..400)
            .flat_map(|i| {
                let base: i16 = if i < 200 { 20_000 } else { -20_000 };
                let wobble: i16 = if i % 3 == 0 { 1_500 } else { -1_500 };
                (base + wobble).to_le_bytes()
            })
            .collect();
        let tape = parse_wav(&wav_bytes(1, 1, 10_000, 16, &noisy)).unwrap();
        assert_eq!(pulses_of(&tape).len(), 2);
        // A 1 kHz sine at 48 kHz for 10 periods: ten high and ten low pulses (the first low edge is the start).
        let sine: Vec<u8> = (0..480)
            .flat_map(|i| {
                let v =
                    (f64::sin(2.0 * std::f64::consts::PI * f64::from(i) / 48.0) * 20_000.0) as i16;
                v.to_le_bytes()
            })
            .collect();
        let tape = parse_wav(&wav_bytes(1, 1, 48_000, 16, &sine)).unwrap();
        let pulses = pulses_of(&tape);
        let highs = pulses.iter().filter(|p| p.1).count();
        assert_eq!(highs, 10, "{pulses:?}");
    }

    #[test]
    fn wav_files_that_are_not_usable_are_refused_clearly() {
        assert_eq!(
            parse_wav(b"RIFFxxxxWAVE"),
            Err(Error::NotAWavFile),
            "no chunks"
        );
        assert_eq!(
            parse_wav(b"not a wav at all, really"),
            Err(Error::NotAWavFile)
        );
        // ADPCM (tag 2) is a real WAV format this parser does not read.
        let adpcm = wav_bytes(2, 1, 22_050, 4, &[0; 16]);
        assert_eq!(
            parse_wav(&adpcm),
            Err(Error::UnsupportedWavFormat { tag: 2, bits: 4 })
        );
        // A silent recording is one low pulse, not an error.
        let silent = wav_bytes(1, 1, 10_000, 16, &[0; 2_000]);
        assert_eq!(
            pulses_of(&parse_wav(&silent).unwrap()),
            vec![(354_690, false)]
        );
        // A data chunk that claims more than the file holds reads what is there.
        let mut cut = wav_bytes(1, 1, 10_000, 16, &square_16(&[(100, true), (100, false)]));
        let claimed = (cut.len() as u32 + 1_000).to_le_bytes();
        let data_len_at = cut.len() - 400 - 4;
        cut[data_len_at..data_len_at + 4].copy_from_slice(&claimed);
        assert_eq!(pulses_of(&parse_wav(&cut).unwrap()).len(), 2);
    }
}
