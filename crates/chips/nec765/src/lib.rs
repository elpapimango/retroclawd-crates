//! NEC765/uPD765A floppy disk controller emulation -- the real FDC of the
//! ZX Spectrum +2A/+3 and the Amstrad CPC664/6128. Protocol-only: no
//! dependency on any disk file format (see `DiskMedium` below) or any host
//! machine crate, same reasoning as `crates/ay-3-8910` (a real chip's own
//! documented protocol, usable by any host that wires up the right ports).
//!
//! # Fidelity target
//!
//! Behaviour follows the uPD765A datasheet and, where real CPC/+3 firmware
//! depends on a detail the datasheet leaves open, Caprice32's `fdc.cpp`
//! (see `CREDITS.md`) -- an emulator that runs the full CPC software
//! library:
//!
//! * The command byte's `MT`/`MF`/`SK` flag bits are decoded (`opcode &
//!   0x1F` selects the command, `SK` skips deleted/normal data marks).
//!   Real AMSDOS sends `0x46` (Read Data + MFM), not a bare `0x06`.
//! * The main status register reports `RQM`, `DIO`, `EXM` (execution
//!   mode) and `CB` (busy) so firmware can tell command, execution and
//!   result phases apart: idle `0x80`, receiving parameters `0x90`,
//!   execution-read `0xF0`, execution-write `0xB0`, result `0xD0`.
//! * Read/Write Data transfer bytes one at a time in the execution phase,
//!   chain sectors up to `EOT`, and terminate with ST0 `0x40` / ST1 `0x80`
//!   ("end of cylinder") because the CPC/+3 wire no terminal-count line --
//!   that pair *is* the normal success result on real hardware.
//! * Write Data, Write Deleted Data and Format Track (Write ID) modify the
//!   medium through `DiskMedium`'s write hooks.
//! * The head's physical cylinder is tracked separately from the `C` field
//!   of a command's sector ID, so sectors are found by matching all four
//!   ID bytes (`CHRN`) on the physical track under the head.
//!
//! Without `enable_timing` execution is instantaneous (no rotational latency,
//! no overrun -- a byte the CPU is slow to fetch is never lost). With it (the
//! CPC) the disk turns: see "Timing" below.
//!
//! # Timing (`enable_timing`)
//!
//! The disk turns at 300 rpm and the MFM data stream carries one byte every
//! 32 us, 6250 byte times a revolution. A track's sector IDs are spread evenly
//! after the 146-byte index gap (`INDEX_GAP_BYTES`), each sector's data
//! starting 60 bytes after its ID (`ID_TO_DATA_BYTES`). A Read/Write Data
//! command waits until its sector comes round, then offers or asks for one
//! byte every 32 us: the main status register shows RQM only while a byte is
//! due, and a byte the CPU does not take (or give) before the next one ends the
//! command with ST1 "overrun" (`&10`). A sector that is not found ends after two
//! index holes; Read ID waits for the next ID; Format runs from one index hole
//! to the next; Seek and Recalibrate step at the Specify step rate (`(16 - SRT)
//! x 2 ms`, the CPC's 4 MHz clock), one cylinder at a time, keeping a present
//! cylinder number per unit, and keep the unit's busy bit (MSR D0-D3) set until
//! Sense Interrupt Status reports the seek. While the chip searches for a sector
//! the main status register reads `CB` (`&10`; `fdc_read`'s "read data" reads it
//! right after the command); during Read ID and Format it reads `CB|EXM`
//! (`&30`). Kevin Thacker's `fdc_*` acid programs are the reference for these
//! behaviours; what they still find is listed in the host repository's
//! `TODO.md` ("Acid suites").
//!
//! Further simplifications (documented, not hardware claims): the `MT`
//! (multi-track) bit is accepted but not acted on;
//! two drives exist (units 0 and 1; the CPC and +3 interfaces wire only US0, so
//! units 2 and 3 are drives 0 and 1 again, each with its own present cylinder
//! number; an empty drive is not ready); seek time without `enable_timing` is
//! opt-in via `enable_seek_timing`.
//!
//! **DMA mode** (Specify with `ND` = 0): the main status register never shows
//! `EXM`, and during an execution phase not `RQM` either (the chip raises DRQ
//! instead), so the CPU moves no bytes. Neither the CPC nor the +3 wires DMA:
//! on a CPC6128 `/DACK` is pulled up through a resistor, and Kevin Thacker's
//! `fdc_read` shows a read ending with "data error" in ST1 and ST2 (`&20`,
//! `&20`) once the sector has passed; a write (`fdc_write`'s "write data
//! (dma)") ends normally. The bytes such a write puts on the disk are not
//! known; this model writes `&FF`. With timing the command ends when the
//! sector has turned past the head; without it, at the next status read.
//!
//! A drive whose ready line changes (its motor, a disk in or out) raises an
//! interrupt, which Sense Interrupt Status reports for each unit the chip polls
//! (0 to 3): ST0 `&C0` plus the unit, `&08` more when it became not ready, and
//! the unit's cylinder.

use serde::{Deserialize, Serialize};

/// One sector's real NEC765 ID field. `size_code` is the chip's own
/// `128 << size_code` sector-size encoding -- matches the `.dsk` file
/// format's own per-sector size byte 1:1, so no conversion is needed
/// either direction (see `dsk::Sector`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorId {
    pub cylinder: u8,
    pub head: u8,
    pub sector: u8,
    pub size_code: u8,
}

/// A sector's ID plus the FDC status flags recorded with it (`.dsk`
/// sector-info bytes 4/5): ST1 `0x20`/ST2 `0x20` = data error, ST2 `0x40`
/// = deleted-data address mark, and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorInfo {
    pub id: SectorId,
    pub st1: u8,
    pub st2: u8,
}

/// What the chip needs from a disk. This crate has no idea `.dsk` (or any
/// other disk file format) exists; hosts implement this for their own
/// parsed disk type.
///
/// Sectors are addressed by *index* in rotational order (the order they
/// pass under the head), which is how the chip walks a track; the ID
/// fields are then matched by the chip itself.
pub trait DiskMedium {
    /// Sectors on `(cylinder, head)` in rotational order; `0` for an
    /// unformatted or absent track.
    fn sector_count(&self, cylinder: u8, head: u8) -> usize;

    /// The `index`th sector's ID and stored status flags.
    fn sector_info(&self, cylinder: u8, head: u8, index: usize) -> Option<SectorInfo>;

    /// The `index`th sector's data. `attempt` (see `Fdc::weak_read_counter`)
    /// lets a weak/copy-protected sector return a different one of its
    /// stored copies each successive read; a medium with one copy per
    /// sector should ignore it.
    fn sector_data(&self, cylinder: u8, head: u8, index: usize, attempt: u32) -> Option<&[u8]>;

    /// How many sides the loaded image stores. A single-sided image is
    /// always read on side 0 regardless of the command's head bit.
    fn sides(&self) -> u8 {
        2
    }

    /// Whether the write-protect tab is set. Defaults to `true` so a
    /// medium that doesn't implement the write hooks is read-only.
    fn write_protected(&self) -> bool {
        true
    }

    /// Overwrites the `index`th sector's data (`deleted` records a
    /// deleted-data address mark). Returns whether the write happened.
    fn write_sector(
        &mut self,
        _cylinder: u8,
        _head: u8,
        _index: usize,
        _data: &[u8],
        _deleted: bool,
    ) -> bool {
        false
    }

    /// Replaces a track with a freshly formatted one: `ids` in rotational
    /// order, `gap3` the gap length, every data byte set to `filler`.
    fn format_track(
        &mut self,
        _cylinder: u8,
        _head: u8,
        _ids: &[SectorId],
        _gap3: u8,
        _filler: u8,
    ) -> bool {
        false
    }
}

/// Real T-states-per-track-step Seek/Recalibrate timing constant --
/// ZEsarUX's own `src/storage/pd765.c` (`signal_se.max = 80`), cited in
/// `CREDITS.md`. Only applied once `Fdc::enable_seek_timing` has been
/// called on a given instance.
const SEEK_TSTATES_PER_TRACK: u32 = 80;

/// Microseconds per byte of the MFM data stream (250 kbit/s).
const BYTE_US: u64 = 32;
/// Byte times in one revolution at 300 rpm (200 ms / 32 us).
const REVOLUTION_BYTES: u64 = 6250;
/// Gap 4a, sync, index mark and gap 1: where the first ID starts.
const INDEX_GAP_BYTES: u64 = 146;
/// The ID field (sync, address mark, ID, CRC).
const ID_FIELD_BYTES: u64 = 22;
/// From the start of an ID field to its sector's first data byte (ID field,
/// gap 2, sync, data mark).
const ID_TO_DATA_BYTES: u64 = 60;
/// The data CRC, read before the next sector or the result phase.
const DATA_CRC_BYTES: u64 = 2;

/// The clock the chip's timing runs against (`enable_timing`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Timing {
    /// Host T-states per byte time.
    byte_t: u64,
    /// Host T-states per millisecond.
    ms_t: u64,
    /// Host T-states since timing was enabled.
    now: u64,
}

const RQM: u8 = 0x80;
const DIO: u8 = 0x40;
const EXM: u8 = 0x20;
const CB: u8 = 0x10;

/// The commands this controller implements (`opcode & 0x1F`).
mod op {
    pub const READ_TRACK: u8 = 0x02;
    pub const SPECIFY: u8 = 0x03;
    pub const SENSE_DRIVE_STATUS: u8 = 0x04;
    pub const WRITE_DATA: u8 = 0x05;
    pub const READ_DATA: u8 = 0x06;
    pub const RECALIBRATE: u8 = 0x07;
    pub const SENSE_INTERRUPT_STATUS: u8 = 0x08;
    pub const WRITE_DELETED_DATA: u8 = 0x09;
    pub const READ_ID: u8 = 0x0A;
    pub const READ_DELETED_DATA: u8 = 0x0C;
    pub const FORMAT_TRACK: u8 = 0x0D;
    pub const SEEK: u8 = 0x0F;
    pub const SCAN_EQUAL: u8 = 0x11;
    pub const SCAN_LOW_OR_EQUAL: u8 = 0x19;
    pub const SCAN_HIGH_OR_EQUAL: u8 = 0x1D;
}

/// Number of parameter bytes the command phase needs after the opcode, per
/// the uPD765A datasheet's command tables. `None` = not a command this
/// controller implements (answered with the single-byte invalid result).
fn param_count(command: u8) -> Option<usize> {
    match command {
        op::READ_TRACK
        | op::WRITE_DATA
        | op::READ_DATA
        | op::WRITE_DELETED_DATA
        | op::READ_DELETED_DATA
        | op::SCAN_EQUAL
        | op::SCAN_LOW_OR_EQUAL
        | op::SCAN_HIGH_OR_EQUAL => Some(8),
        op::SPECIFY => Some(2),
        op::SENSE_DRIVE_STATUS | op::RECALIBRATE | op::READ_ID => Some(1),
        op::SENSE_INTERRUPT_STATUS => Some(0),
        op::FORMAT_TRACK => Some(5),
        op::SEEK => Some(2),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Kind {
    ReadData,
    ReadDeleted,
    ReadTrack,
    WriteData,
    WriteDeleted,
    Format,
    /// The Scan commands: the CPU supplies a sector's worth of bytes, which
    /// the chip compares with the sector's.
    ScanEqual,
    ScanLowOrEqual,
    ScanHighOrEqual,
}

impl Kind {
    fn is_write(self) -> bool {
        matches!(self, Kind::WriteData | Kind::WriteDeleted | Kind::Format)
    }

    fn is_scan(self) -> bool {
        matches!(
            self,
            Kind::ScanEqual | Kind::ScanLowOrEqual | Kind::ScanHighOrEqual
        )
    }
}

/// A data-transfer command in flight: its parameters plus the running
/// status flags that end up in the result phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Active {
    kind: Kind,
    /// Parameter byte 0: `HD` (bit 2) / `US1` / `US0`.
    unit_byte: u8,
    c: u8,
    h: u8,
    r: u8,
    n: u8,
    eot: u8,
    gpl: u8,
    dtl: u8,
    skip: bool,
    st0: u8,
    st1: u8,
    st2: u8,
    /// Physical side being accessed.
    side: u8,
    /// Track index of the sector currently in transfer.
    sector_index: usize,
}

impl Active {
    /// The 7 result bytes: ST0/ST1/ST2 then the command's current CHRN.
    fn result_bytes(&self) -> Vec<u8> {
        vec![self.st0, self.st1, self.st2, self.c, self.h, self.r, self.n]
    }

    /// Caprice32's `LOAD_RESULT_WITH_STATUS`: normal termination without a
    /// terminal-count line ends with "abnormal termination" + "end of
    /// cylinder", cleared again if the transfer hit a real error; a bare
    /// deleted-data mark is *not* an error.
    fn load_status(&mut self) {
        self.st0 |= 0x40;
        self.st1 |= 0x80;
        if self.kind != Kind::ReadTrack && ((self.st1 & 0x7F) != 0 || (self.st2 & 0x7F) != 0) {
            self.st1 &= 0x7F;
            if (self.st1 & 0x20) != 0 || (self.st2 & 0x20) != 0 {
                self.st2 &= 0xBF;
            } else if (self.st2 & 0x40) != 0 {
                self.st0 &= 0x3F;
                self.st1 &= 0x7F;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Dir {
    ToCpu,
    FromCpu,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Phase {
    Idle,
    Command {
        opcode: u8,
        params_needed: usize,
        params: Vec<u8>,
    },
    Execution {
        active: Active,
        dir: Dir,
        /// `ToCpu`: the bytes still to hand over. `FromCpu`: the bytes
        /// collected so far.
        buffer: Vec<u8>,
        pos: usize,
        /// `FromCpu`: how many bytes complete the transfer.
        expected: usize,
        /// Reads of the main status register that still report "not
        /// ready" before data is offered (Caprice32's `read_status_delay`).
        delay: u8,
        /// With timing: when the byte at `pos` (or the next one to write) is due.
        #[serde(default)]
        ready_at: u64,
    },
    /// With timing: the command waits for the disk (or for its result); the
    /// main status register reads `msr` until `until`, then `next` begins.
    Waiting {
        until: u64,
        msr: u8,
        next: Box<Phase>,
    },
    Result {
        bytes: Vec<u8>,
        pos: usize,
    },
}

/// One unit's Seek or Recalibrate. The chip keeps a present cylinder number
/// (PCN) per unit (0 to 3) and steps the drive the unit selects; units 0 and 2
/// (1 and 3) step the same head here, so two overlapped seeks on them move it
/// both ways (`fdc_seek`'s "seek drive n and drive n+2").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum SeekState {
    #[default]
    None,
    /// Stepping towards `target`, one cylinder every `step_t` T-states,
    /// `remaining` until the next step. A recalibrate steps out until the
    /// drive's track 0 sensor (at most 77 steps).
    InFlight {
        remaining: u32,
        st0: u8,
        target: u8,
        step_t: u32,
        recalibrate: bool,
        steps: u8,
    },
    /// Finished; Sense Interrupt Status reports it.
    Done { st0: u8 },
}

/// How many drives the controller addresses (unit select 0 and 1).
pub const DRIVES: usize = 2;

/// Units the chip polls for ready changes (US1/US0).
const UNITS: usize = 4;

/// The drive a unit-select byte reaches: only US0 is wired, so units 2 and 3
/// are drives 0 and 1 (Kevin Thacker's `fdc_gen` "sense drive status (2
/// drives)" reads unit 2 as drive 0 on a CPC).
fn drive_of(unit_byte: u8) -> usize {
    usize::from(unit_byte & 0x01)
}

/// `Fdc`'s own live state, for save states -- deliberately excludes the
/// inserted disk (`Fdc::disk`); a host that lets the disk be modified
/// saves the modified image itself (see `cpc`/`zx_spectrum` save states).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FdcState {
    cylinders: [u8; DRIVES],
    unit: usize,
    current_sector: usize,
    phase: Phase,
    #[serde(default)]
    seeks: [SeekState; UNITS],
    #[serde(default)]
    pcn: [u8; UNITS],
    seek_timing_enabled: bool,
    weak_read_counter: u32,
    motor: bool,
    #[serde(default)]
    ready_seen: [bool; UNITS],
    #[serde(default)]
    ready_changed: [bool; UNITS],
    #[serde(default)]
    timing: Option<Timing>,
    #[serde(default)]
    step_rate: u8,
    #[serde(default)]
    busy: u8,
    #[serde(default)]
    dma: bool,
}

impl FdcState {
    /// Current cylinder of the unit last addressed (PCN source for Sense Interrupt Status).
    pub fn cylinder(&self) -> u8 {
        self.cylinders[self.unit]
    }

    /// Current cylinder of `unit` (0 or 1; anything else reads 0).
    pub fn cylinder_of(&self, unit: usize) -> u8 {
        self.cylinders.get(unit).copied().unwrap_or(0)
    }

    /// High-level controller phase label for diagnostics.
    pub fn phase_label(&self) -> &'static str {
        match self.phase {
            Phase::Idle => "idle",
            Phase::Command { .. } => "command",
            Phase::Execution { .. } | Phase::Waiting { .. } => "execution",
            Phase::Result { .. } => "result",
        }
    }
}

/// The NEC765/uPD765A itself. Generic over `D: DiskMedium` rather than a
/// trait object -- one concrete disk type per real machine instance, no
/// need for dynamic dispatch, and this crate's own tests can use a
/// trivial in-memory fake without depending on `dsk`.
#[derive(Clone)]
pub struct Fdc<D: DiskMedium> {
    /// The two drives a CPC or +3 interface can address (unit select 0 and 1). A command for a unit with no disk reports
    /// not ready; units 2 and 3 do not exist.
    disks: [Option<D>; DRIVES],
    /// The unit the command in progress (or the last one) addressed: `disks` and `cylinders` are indexed by it.
    unit: usize,
    /// Physical cylinder each drive's head is over.
    cylinders: [u8; DRIVES],
    /// Rotational position: index of the next sector to pass under the
    /// head on the current track.
    current_sector: usize,
    phase: Phase,
    /// Each unit's Seek/Recalibrate and present cylinder number.
    seeks: [SeekState; UNITS],
    pcn: [u8; UNITS],
    /// Opt-in (`enable_seek_timing`): whether Seek/Recalibrate report Seek
    /// End only after a bounded real-time delay instead of instantly.
    seek_timing_enabled: bool,
    /// Rolling count incremented on every sector read and passed as
    /// `DiskMedium::sector_data`'s `attempt`, so weak/copy-protected
    /// sectors return a different stored copy each time. A single global
    /// counter (not keyed per sector like Caprice32's) -- copy-protection
    /// checks typically re-read the same sector back to back, which this
    /// handles; interleaved reads of several weak sectors see a less
    /// precise rotation than real hardware.
    weak_read_counter: u32,
    /// Drive motor. A drive is only ready while its motor spins; hosts
    /// mirror their motor latch here with `set_motor`.
    motor: bool,
    /// Each polled unit's ready line as last seen, and whether it changed
    /// since Sense Interrupt Status last reported it.
    ready_seen: [bool; UNITS],
    ready_changed: [bool; UNITS],
    /// `enable_timing`: the disk turns and the data stream has a rate.
    timing: Option<Timing>,
    /// The Specify command's step rate (SRT).
    step_rate: u8,
    /// MSR D0-D3: a seek in progress or not yet reported, per unit.
    busy: u8,
    /// DMA mode (Specify `ND` = 0): see "DMA mode" in the module doc.
    dma: bool,
}

impl<D: DiskMedium> Default for Fdc<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: DiskMedium> Fdc<D> {
    pub fn new() -> Self {
        Self {
            disks: [None, None],
            unit: 0,
            cylinders: [0; DRIVES],
            current_sector: 0,
            phase: Phase::Idle,
            seeks: [SeekState::None; UNITS],
            pcn: [0; UNITS],
            seek_timing_enabled: false,
            weak_read_counter: 0,
            motor: true,
            ready_seen: [false; UNITS],
            ready_changed: [false; UNITS],
            timing: None,
            // The step rate before any Specify: the value AMSDOS and +3DOS both
            // specify (`&A`, 12 ms), so a machine resumed from a snapshot -- which
            // does not carry the FDC -- seeks at the speed its DOS waits for.
            step_rate: 0x0A,
            busy: 0,
            dma: false,
        }
    }

    /// Turns the disk and gives the data stream its rate (see "Timing" in the
    /// module doc): the host then calls `tick` with the T-states of a
    /// `cpu_hz` clock. Implies `enable_seek_timing`.
    pub fn enable_timing(&mut self, cpu_hz: u32) {
        self.seek_timing_enabled = true;
        self.timing = Some(Timing {
            byte_t: u64::from(cpu_hz) * BYTE_US / 1_000_000,
            ms_t: u64::from(cpu_hz) / 1000,
            now: 0,
        });
    }

    fn now(&self) -> u64 {
        self.timing.map_or(0, |t| t.now)
    }

    /// Where the head is on the track, in byte times since the index hole.
    fn rotation(&self) -> u64 {
        self.timing
            .map_or(0, |t| (t.now / t.byte_t.max(1)) % REVOLUTION_BYTES)
    }

    /// Byte times from the head's position to `target` (0 = the index hole).
    fn bytes_until(&self, target: u64) -> u64 {
        (target % REVOLUTION_BYTES + REVOLUTION_BYTES - self.rotation()) % REVOLUTION_BYTES
    }

    /// Where the `index`th of `count` sector IDs starts.
    fn id_start(count: usize, index: usize) -> u64 {
        let spacing = (REVOLUTION_BYTES - INDEX_GAP_BYTES) / (count.max(1) as u64);
        INDEX_GAP_BYTES + index as u64 * spacing
    }

    /// The first sector ID to pass under the head from now on.
    fn next_sector_index(&self, count: usize) -> usize {
        let pos = self.rotation();
        (0..count)
            .find(|&i| Self::id_start(count, i) >= pos)
            .unwrap_or(0)
    }

    /// Enters `phase` after `bytes` byte times (at once without timing),
    /// reading `msr` meanwhile. An execution phase's first byte is due then.
    fn after(&mut self, bytes: u64, msr: u8, mut phase: Phase) {
        let Some(t) = self.timing else {
            self.phase = phase;
            return;
        };
        let until = t.now + bytes * t.byte_t;
        if let Phase::Execution {
            ready_at, delay, ..
        } = &mut phase
        {
            *ready_at = until;
            *delay = 0;
        }
        self.phase = Phase::Waiting {
            until,
            msr,
            next: Box::new(phase),
        };
    }

    /// Polls every unit's ready line and latches an interrupt for each one
    /// that changed.
    fn poll_ready(&mut self) {
        for unit in 0..UNITS {
            let ready = self.ready(unit as u8);
            if ready != self.ready_seen[unit] {
                self.ready_seen[unit] = ready;
                self.ready_changed[unit] = true;
            }
        }
    }

    /// Enables bounded, real-time-gated Seek/Recalibrate completion. The
    /// caller must also drive `tick` with real elapsed T-states, or Seek
    /// End will never report -- opt-in per instance since hosts that never
    /// call `tick` must keep their instant-Seek-End behaviour.
    pub fn enable_seek_timing(&mut self) {
        self.seek_timing_enabled = true;
    }

    /// Advances real elapsed T-states -- only has an effect once
    /// `enable_seek_timing` has been called.
    pub fn tick(&mut self, tstates: u32) {
        if !self.seek_timing_enabled {
            return;
        }
        if let Some(t) = &mut self.timing {
            t.now += u64::from(tstates);
            self.advance_phase();
        }
        for unit in 0..UNITS {
            self.step_unit(unit, tstates);
        }
    }

    /// Lets `tstates` pass for `unit`'s seek: each step moves its PCN and its
    /// drive's head one cylinder.
    fn step_unit(&mut self, unit: usize, tstates: u32) {
        let SeekState::InFlight {
            mut remaining,
            mut st0,
            target,
            step_t,
            recalibrate,
            mut steps,
        } = self.seeks[unit]
        else {
            return;
        };
        let mut left = tstates;
        loop {
            if left < remaining {
                self.seeks[unit] = SeekState::InFlight {
                    remaining: remaining - left,
                    st0,
                    target,
                    step_t,
                    recalibrate,
                    steps,
                };
                return;
            }
            left -= remaining;
            remaining = step_t.max(1);
            let head = &mut self.cylinders[drive_of(unit as u8)];
            if recalibrate || self.pcn[unit] > target {
                self.pcn[unit] = self.pcn[unit].saturating_sub(1);
                *head = head.saturating_sub(1);
            } else if self.pcn[unit] < target {
                self.pcn[unit] += 1;
                *head = head.saturating_add(1);
            }
            steps = steps.saturating_add(1);
            let done = if recalibrate {
                if *head == 0 {
                    self.pcn[unit] = 0;
                    true
                } else if steps >= 77 {
                    st0 |= 0x50; // abnormal termination + equipment check: no track 0 after 77 steps
                    true
                } else {
                    false
                }
            } else {
                self.pcn[unit] == target
            };
            if done {
                self.seeks[unit] = SeekState::Done { st0 };
                return;
            }
        }
    }

    /// Moves a waiting command on, and ends a transfer whose byte the CPU let
    /// pass (overrun).
    fn advance_phase(&mut self) {
        let Some(t) = self.timing else { return };
        loop {
            if self.dma
                && let Phase::Execution {
                    dir,
                    buffer,
                    pos,
                    expected,
                    ready_at,
                    ..
                } = &self.phase
            {
                let left = match dir {
                    Dir::ToCpu => buffer.len().saturating_sub(*pos),
                    Dir::FromCpu => expected.saturating_sub(buffer.len()),
                };
                if t.now >= *ready_at + left as u64 * t.byte_t {
                    self.finish_dma_transfer();
                }
                return;
            }
            match &self.phase {
                Phase::Waiting { until, .. } if t.now >= *until => {
                    let Phase::Waiting { next, .. } =
                        std::mem::replace(&mut self.phase, Phase::Idle)
                    else {
                        unreachable!()
                    };
                    self.phase = *next;
                }
                Phase::Execution { ready_at, .. } if t.now >= *ready_at + t.byte_t => {
                    let Phase::Execution { mut active, .. } =
                        std::mem::replace(&mut self.phase, Phase::Idle)
                    else {
                        unreachable!()
                    };
                    active.st0 = (active.st0 & 0x3F) | 0x40;
                    active.st1 |= 0x10; // overrun
                    self.finish(&active);
                    return;
                }
                _ => return,
            }
        }
    }

    /// Replaces any disk already in drive 0 -- a real drive can only hold one at a time.
    pub fn insert_disk(&mut self, disk: D) {
        self.insert_disk_in(0, disk);
    }

    /// Removes and returns the disk in drive 0, if any.
    pub fn eject_disk(&mut self) -> Option<D> {
        self.eject_disk_from(0)
    }

    /// The disk in drive 0, if any.
    pub fn disk(&self) -> Option<&D> {
        self.disk_in(0)
    }

    pub fn disk_mut(&mut self) -> Option<&mut D> {
        self.disk_in_mut(0)
    }

    /// Puts `disk` in `unit` (0 or 1), replacing what was there. A unit that does not exist is ignored.
    pub fn insert_disk_in(&mut self, unit: usize, disk: D) {
        if let Some(slot) = self.disks.get_mut(unit) {
            *slot = Some(disk);
        }
        self.poll_ready();
    }

    /// Removes and returns the disk in `unit`, if any.
    pub fn eject_disk_from(&mut self, unit: usize) -> Option<D> {
        let disk = self.disks.get_mut(unit).and_then(Option::take);
        self.poll_ready();
        disk
    }

    /// The disk in `unit`, if any.
    pub fn disk_in(&self, unit: usize) -> Option<&D> {
        self.disks.get(unit).and_then(Option::as_ref)
    }

    pub fn disk_in_mut(&mut self, unit: usize) -> Option<&mut D> {
        self.disks.get_mut(unit).and_then(Option::as_mut)
    }

    /// Mirrors the host's drive-motor latch. A drive whose motor is off is
    /// not ready: data commands terminate with ST0 "not ready" and Sense
    /// Drive Status reports RY clear.
    pub fn set_motor(&mut self, on: bool) {
        self.motor = on;
        self.poll_ready();
    }

    pub fn motor(&self) -> bool {
        self.motor
    }

    /// This `Fdc`'s own live state, for save states -- see `FdcState`'s
    /// own doc comment for what's deliberately excluded.
    pub fn dump_state(&self) -> FdcState {
        FdcState {
            cylinders: self.cylinders,
            unit: self.unit,
            current_sector: self.current_sector,
            phase: self.phase.clone(),
            seeks: self.seeks,
            pcn: self.pcn,
            seek_timing_enabled: self.seek_timing_enabled,
            weak_read_counter: self.weak_read_counter,
            motor: self.motor,
            ready_seen: self.ready_seen,
            ready_changed: self.ready_changed,
            timing: self.timing,
            step_rate: self.step_rate,
            busy: self.busy,
            dma: self.dma,
        }
    }

    /// Restores previously-dumped state -- the currently inserted disk
    /// (if any) is left untouched; the caller is responsible for having
    /// already inserted the disk a save state expects.
    pub fn restore_state(&mut self, state: FdcState) {
        self.cylinders = state.cylinders;
        self.unit = state.unit.min(DRIVES - 1);
        self.current_sector = state.current_sector;
        self.phase = state.phase;
        self.seeks = state.seeks;
        self.pcn = state.pcn;
        self.seek_timing_enabled = state.seek_timing_enabled;
        self.weak_read_counter = state.weak_read_counter;
        self.motor = state.motor;
        self.ready_seen = state.ready_seen;
        self.ready_changed = state.ready_changed;
        if state.timing.is_some() {
            self.timing = state.timing;
        }
        self.step_rate = state.step_rate;
        self.busy = state.busy;
        self.dma = state.dma;
    }

    /// Main status register. Idle `0x80`; receiving parameters `0x90`
    /// (busy); execution `EXM|CB|RQM` plus `DIO` when the controller is
    /// sending data (`0xF0`) or without it when receiving (`0xB0`); result
    /// `0xD0`. The first read after an execution phase starts reports
    /// not-ready (`0x10`) once, like Caprice32.
    pub fn read_msr(&mut self) -> u8 {
        let now = self.now();
        let timed = self.timing.is_some();
        let busy = self.busy;
        if self.dma && matches!(self.phase, Phase::Execution { .. }) {
            if !timed {
                self.finish_dma_transfer();
                return self.read_msr();
            }
            return CB | busy;
        }
        let status = match &mut self.phase {
            Phase::Idle => RQM,
            // The command phase only exists once the opcode byte has
            // arrived, so the controller is already busy.
            Phase::Command { .. } => RQM | CB,
            Phase::Waiting { msr, .. } => *msr,
            Phase::Execution {
                dir,
                delay,
                ready_at,
                ..
            } => {
                if *delay > 0 {
                    *delay -= 1;
                    return CB | busy;
                }
                if timed && now < *ready_at {
                    return EXM | CB | busy;
                }
                let mut v = RQM | EXM | CB;
                if *dir == Dir::ToCpu {
                    v |= DIO;
                }
                v
            }
            Phase::Result { .. } => RQM | DIO | CB,
        };
        status | busy
    }

    /// Data register read (execution-read data or result byte). Returns
    /// `0xFF` when the controller has nothing to send.
    pub fn read_data(&mut self) -> u8 {
        if self.dma && matches!(self.phase, Phase::Execution { .. }) {
            return 0xFF; // DMA mode: the data goes over DACK, not the data register
        }
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Result { bytes, pos } => {
                let byte = bytes.get(pos).copied().unwrap_or(0);
                if pos + 1 < bytes.len() {
                    self.phase = Phase::Result {
                        bytes,
                        pos: pos + 1,
                    };
                }
                byte
            }
            Phase::Execution {
                active,
                dir: Dir::ToCpu,
                buffer,
                pos,
                expected,
                delay,
                ready_at,
            } => {
                if self.timing.is_some() && self.now() < ready_at {
                    // No byte has arrived yet: the data register still holds the last one.
                    let byte = pos
                        .checked_sub(1)
                        .and_then(|p| buffer.get(p))
                        .copied()
                        .unwrap_or(0xFF);
                    self.phase = Phase::Execution {
                        active,
                        dir: Dir::ToCpu,
                        buffer,
                        pos,
                        expected,
                        delay,
                        ready_at,
                    };
                    return byte;
                }
                let byte = buffer.get(pos).copied().unwrap_or(0xFF);
                let pos = pos + 1;
                let byte_t = self.timing.map_or(0, |t| t.byte_t);
                if pos >= buffer.len() {
                    self.finish_read_sector(active);
                } else {
                    self.phase = Phase::Execution {
                        active,
                        dir: Dir::ToCpu,
                        buffer,
                        pos,
                        expected,
                        delay,
                        ready_at: ready_at + byte_t,
                    };
                }
                byte
            }
            other => {
                self.phase = other;
                0xFF
            }
        }
    }

    /// Data register write (command byte, parameter, or execution-write
    /// data). Ignored while the controller is sending.
    pub fn write_data(&mut self, val: u8) {
        if self.dma && matches!(self.phase, Phase::Execution { .. }) {
            return; // DMA mode: see `read_data`
        }
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Idle => match param_count(val & 0x1F) {
                Some(0) => self.execute(val, &[]),
                Some(needed) => {
                    self.phase = Phase::Command {
                        opcode: val,
                        params_needed: needed,
                        params: Vec::new(),
                    }
                }
                None => {
                    self.phase = Phase::Result {
                        bytes: vec![0x80],
                        pos: 0,
                    }
                }
            },
            Phase::Command {
                opcode,
                params_needed,
                mut params,
            } => {
                params.push(val);
                if params.len() == params_needed {
                    self.execute(opcode, &params);
                } else {
                    self.phase = Phase::Command {
                        opcode,
                        params_needed,
                        params,
                    };
                }
            }
            Phase::Execution {
                active,
                dir: Dir::FromCpu,
                mut buffer,
                pos,
                expected,
                delay,
                ready_at,
            } => {
                if self.timing.is_some() && self.now() < ready_at {
                    // Not asked for yet: the byte is lost.
                    self.phase = Phase::Execution {
                        active,
                        dir: Dir::FromCpu,
                        buffer,
                        pos,
                        expected,
                        delay,
                        ready_at,
                    };
                    return;
                }
                buffer.push(val);
                if buffer.len() >= expected {
                    self.finish_write_transfer(active, buffer);
                } else {
                    let byte_t = self.timing.map_or(0, |t| t.byte_t);
                    // Format asks for the four ID bytes of each sector as that sector comes round.
                    let gap = if active.kind == Kind::Format && buffer.len() % 4 == 0 {
                        (REVOLUTION_BYTES - INDEX_GAP_BYTES) / u64::from(active.h.max(1)) - 3
                    } else {
                        1
                    };
                    self.phase = Phase::Execution {
                        active,
                        dir: Dir::FromCpu,
                        buffer,
                        pos,
                        expected,
                        delay,
                        ready_at: ready_at + gap * byte_t,
                    };
                }
            }
            other => self.phase = other,
        }
    }

    fn execute(&mut self, opcode: u8, params: &[u8]) {
        let skip = opcode & 0x20 != 0;
        match opcode & 0x1F {
            op::SPECIFY => {
                // SRT in the high nibble; head load/unload times are not modelled.
                self.step_rate = params[0] >> 4;
                self.dma = params[1] & 0x01 == 0;
                self.phase = Phase::Idle;
            }
            op::SENSE_DRIVE_STATUS => self.sense_drive_status(params[0]),
            op::RECALIBRATE => self.seek(params[0], 0, true),
            op::SEEK => self.seek(params[0], params[1], false),
            op::SENSE_INTERRUPT_STATUS => self.sense_interrupt_status(),
            op::READ_ID => self.read_id(params[0]),
            op::READ_DATA => self.begin_transfer(Kind::ReadData, skip, params),
            op::READ_DELETED_DATA => self.begin_transfer(Kind::ReadDeleted, skip, params),
            op::READ_TRACK => self.begin_transfer(Kind::ReadTrack, false, params),
            op::WRITE_DATA => self.begin_transfer(Kind::WriteData, false, params),
            op::WRITE_DELETED_DATA => self.begin_transfer(Kind::WriteDeleted, false, params),
            op::FORMAT_TRACK => self.begin_format(params),
            op::SCAN_EQUAL => self.begin_transfer(Kind::ScanEqual, skip, params),
            op::SCAN_LOW_OR_EQUAL => self.begin_transfer(Kind::ScanLowOrEqual, skip, params),
            op::SCAN_HIGH_OR_EQUAL => self.begin_transfer(Kind::ScanHighOrEqual, skip, params),
            _ => {
                self.phase = Phase::Result {
                    bytes: vec![0x80],
                    pos: 0,
                }
            }
        }
    }

    // ---- drive state ---------------------------------------------------

    /// Makes the unit a command's unit-select bits name the one `disks` and `cylinders` refer to. Units 2 and 3 do not exist: they
    /// leave the selection alone and are never ready.
    fn select_unit(&mut self, unit_byte: u8) {
        self.unit = drive_of(unit_byte);
    }

    /// The addressed drive must hold a disk and have its motor spinning (the CPC has one motor line for both drives).
    fn ready(&self, unit_byte: u8) -> bool {
        self.disks[drive_of(unit_byte)].is_some() && self.motor
    }

    /// A single-sided image is always read on side 0.
    fn side_for(&self, unit_byte: u8) -> u8 {
        let want = (unit_byte >> 2) & 1;
        match &self.disks[self.unit] {
            Some(d) if d.sides() > 1 => want,
            _ => 0,
        }
    }

    fn sense_drive_status(&mut self, unit_byte: u8) {
        self.select_unit(unit_byte);
        let mut st3 = unit_byte & 0x07;
        let wp = self.disks[self.unit]
            .as_ref()
            .is_none_or(D::write_protected);
        if wp {
            st3 |= 0x48; // write protected (+ two-sided, as real CPC drives report)
        }
        if self.ready(unit_byte) {
            st3 |= 0x20;
        }
        if self.cylinders[self.unit] == 0 {
            st3 |= 0x10;
        }
        self.phase = Phase::Result {
            bytes: vec![st3],
            pos: 0,
        };
    }

    fn seek(&mut self, unit_byte: u8, target: u8, recalibrate: bool) {
        self.select_unit(unit_byte);
        let unit = usize::from(unit_byte & 0x03);
        let ready = self.ready(unit_byte);
        let mut st0 = unit_byte & 0x03;
        self.phase = Phase::Idle; // Seek/Recalibrate have no result phase
        if !ready {
            // A drive that is not ready does not step.
            st0 |= 0x48; // abnormal termination + not ready
            self.seeks[unit] = SeekState::Done { st0 };
            return;
        }
        if !self.seek_timing_enabled {
            self.pcn[unit] = target;
            self.cylinders[self.unit] = target;
            self.seeks[unit] = SeekState::Done { st0 };
            return;
        }
        let step_t = match self.timing {
            // The Specify step rate, in 2 ms units at the CPC's 4 MHz FDC clock.
            Some(t) => u64::from(16 - self.step_rate) * 2 * t.ms_t,
            None => u64::from(SEEK_TSTATES_PER_TRACK),
        };
        let step_t = u32::try_from(step_t).unwrap_or(u32::MAX);
        if self.timing.is_some() {
            self.busy |= 1 << unit;
        }
        // A seek issued while the head still moves only changes where it goes; one that needs no step (already on
        // the cylinder, or a recalibrate at track 0) ends at once (`fdc_seek`'s "seek command repeat (while seeking)"
        // re-sends its Seek until Sense Interrupt Status reports the end).
        let remaining = match self.seeks[unit] {
            SeekState::InFlight { remaining, .. } => remaining,
            _ if (!recalibrate && self.pcn[unit] == target)
                || (recalibrate && self.cylinders[self.unit] == 0) =>
            {
                self.pcn[unit] = target;
                self.seeks[unit] = SeekState::Done { st0 };
                return;
            }
            _ => step_t,
        };
        self.seeks[unit] = SeekState::InFlight {
            remaining,
            st0,
            target,
            step_t,
            recalibrate,
            steps: 0,
        };
    }

    fn sense_interrupt_status(&mut self) {
        let finished = (0..UNITS).find_map(|unit| match self.seeks[unit] {
            SeekState::Done { st0 } => Some((unit, st0)),
            _ => None,
        });
        let bytes = match finished {
            Some((unit, st0)) => {
                self.seeks[unit] = SeekState::None;
                self.busy &= !(1 << unit);
                vec![(st0 & 0xFB) | 0x20, self.pcn[unit]] // ST0 keeps US1/US0: the unit that finished the seek
            }
            None => match (0..UNITS).find(|&unit| self.ready_changed[unit]) {
                Some(unit) => {
                    self.ready_changed[unit] = false;
                    let not_ready = if self.ready_seen[unit] { 0 } else { 0x08 };
                    vec![0xC0 | not_ready | unit as u8, self.pcn[unit]]
                }
                // Nothing pending (or the seek is still travelling): the chip
                // answers "invalid command", and firmware polls again.
                None => vec![0x80],
            },
        };
        self.phase = Phase::Result { bytes, pos: 0 };
    }

    fn read_id(&mut self, unit_byte: u8) {
        self.select_unit(unit_byte);
        let mut st0 = unit_byte & 0x07;
        let mut st1 = 0;
        let (mut c, mut h, mut r, mut n) = (self.cylinders[self.unit], (unit_byte >> 2) & 1, 0, 0);
        let mut wait = 0;
        // Byte times spent searching for the ID (`CB` only) before the chip
        // reads it (`EXM|CB`, in DMA mode too: `fdc_gen`'s "read id dma mode"
        // reads `&10` at once and sees `EXM` later).
        let mut search = 0;
        if !self.ready(unit_byte) {
            st0 |= 0x48;
        } else {
            let side = self.side_for(unit_byte);
            let count = self.disks[self.unit]
                .as_ref()
                .map_or(0, |d| d.sector_count(self.cylinders[self.unit], side));
            if count == 0 {
                st0 |= 0x40;
                st1 |= 0x01; // missing address mark
                wait = self.bytes_until(0) + REVOLUTION_BYTES;
            } else {
                let mut idx = self.current_sector;
                if self.timing.is_some() {
                    idx = self.next_sector_index(count);
                    search = self.bytes_until(Self::id_start(count, idx));
                    wait = ID_FIELD_BYTES;
                }
                if idx >= count {
                    idx = 0;
                }
                if let Some(info) = self.disks[self.unit]
                    .as_ref()
                    .and_then(|d| d.sector_info(self.cylinders[self.unit], side, idx))
                {
                    c = info.id.cylinder;
                    h = info.id.head;
                    r = info.id.sector;
                    n = info.id.size_code;
                }
                self.current_sector = idx + 1;
            }
        }
        self.after(
            wait,
            EXM | CB,
            Phase::Result {
                bytes: vec![st0, st1, 0, c, h, r, n],
                pos: 0,
            },
        );
        if let (Some(t), Phase::Waiting { .. }) = (self.timing, &self.phase)
            && search > 0
        {
            let reading = std::mem::replace(&mut self.phase, Phase::Idle);
            self.phase = Phase::Waiting {
                until: t.now + search * t.byte_t,
                msr: CB,
                next: Box::new(reading),
            };
            if let Phase::Waiting { next, .. } = &mut self.phase
                && let Phase::Waiting { until, .. } = next.as_mut()
            {
                *until += search * t.byte_t;
            }
        }
    }

    /// `EXM` for a data command's execution phase: never in DMA mode.
    fn exm(&self) -> u8 {
        if self.dma { 0 } else { EXM }
    }

    // ---- data transfer commands ----------------------------------------

    /// Ends a DMA-mode execution phase as the unanswered DMA leaves it (see
    /// "DMA mode" in the module doc): a read with data errors, a write or
    /// format with `&FF` bytes.
    fn finish_dma_transfer(&mut self) {
        let Phase::Execution {
            mut active,
            dir,
            mut buffer,
            expected,
            ..
        } = std::mem::replace(&mut self.phase, Phase::Idle)
        else {
            return;
        };
        match dir {
            Dir::ToCpu => {
                self.current_sector = active.sector_index + 1;
                active.st0 = (active.st0 & 0x3F) | 0x40;
                active.st1 |= 0x20;
                // A data error drops the control mark, as `Active::load_status` does.
                active.st2 = (active.st2 & !0x40) | 0x20;
                self.finish(&active);
            }
            Dir::FromCpu => {
                buffer.resize(expected, 0xFF);
                self.finish_write_transfer(active, buffer);
            }
        }
    }

    fn finish(&mut self, active: &Active) {
        self.phase = Phase::Result {
            bytes: active.result_bytes(),
            pos: 0,
        };
    }

    fn begin_transfer(&mut self, kind: Kind, skip: bool, params: &[u8]) {
        let mut a = Active {
            kind,
            unit_byte: params[0],
            c: params[1],
            h: params[2],
            r: params[3],
            n: params[4],
            eot: params[5],
            gpl: params[6],
            dtl: params[7],
            skip,
            st0: params[0] & 0x07,
            st1: 0,
            st2: 0,
            side: 0,
            sector_index: 0,
        };
        self.select_unit(a.unit_byte);
        if !self.ready(a.unit_byte) {
            a.st0 |= 0x48;
            self.finish(&a);
            return;
        }
        a.side = self.side_for(a.unit_byte);
        if kind.is_write()
            && self.disks[self.unit]
                .as_ref()
                .is_none_or(D::write_protected)
        {
            a.st0 |= 0x40;
            a.st1 |= 0x02; // not writable
            self.finish(&a);
            return;
        }
        let count = self.disks[self.unit]
            .as_ref()
            .map_or(0, |d| d.sector_count(self.cylinders[self.unit], a.side));
        if count == 0 {
            a.st0 |= 0x40;
            a.st1 |= 0x01; // missing address mark: unformatted track
            self.finish(&a);
            return;
        }
        match kind {
            Kind::ReadData | Kind::ReadDeleted => self.start_read_sector(a),
            Kind::WriteData | Kind::WriteDeleted => self.start_write_sector(a),
            Kind::ScanEqual | Kind::ScanLowOrEqual | Kind::ScanHighOrEqual => {
                self.start_write_sector(a)
            }
            Kind::ReadTrack => self.start_read_track(a),
            Kind::Format => unreachable!("format uses begin_format"),
        }
    }

    /// Bytes a sector transfer moves: `128 << N`, or `DTL` (capped at 128)
    /// when `N == 0`.
    fn transfer_size(a: &Active) -> usize {
        if a.n == 0 {
            usize::from(a.dtl.min(0x80))
        } else {
            128usize << a.n.min(8)
        }
    }

    /// Walks the current track from the head's rotational position for the
    /// sector whose full ID matches the command's CHRN, giving up after
    /// the index hole has passed twice. Records Bad/No Cylinder flags for
    /// the sectors it passes, like the real chip.
    fn find_sector(&mut self, a: &mut Active) -> Option<(usize, SectorInfo)> {
        let count = self.disks[self.unit]
            .as_ref()
            .map_or(0, |d| d.sector_count(self.cylinders[self.unit], a.side));
        if count == 0 {
            return None;
        }
        if self.timing.is_some() {
            self.current_sector = self.next_sector_index(count);
        }
        let want = SectorId {
            cylinder: a.c,
            head: a.h,
            sector: a.r,
            size_code: a.n,
        };
        let mut idx = self.current_sector % count;
        let mut loops = 0;
        let mut found = None;
        while loops < 2 {
            let Some(info) = self.disks[self.unit]
                .as_ref()
                .and_then(|d| d.sector_info(self.cylinders[self.unit], a.side, idx))
            else {
                break;
            };
            if info.id == want {
                a.st2 &= !(0x02 | 0x10);
                found = Some((idx, info));
                break;
            }
            if info.id.cylinder == 0xFF {
                a.st2 |= 0x02; // bad cylinder
            } else if info.id.cylinder != a.c {
                a.st2 |= 0x10; // wrong cylinder
            }
            idx += 1;
            if idx >= count {
                idx = 0;
                loops += 1;
            }
        }
        if a.st2 & 0x02 != 0 {
            a.st2 &= !0x10;
        }
        self.current_sector = idx;
        found
    }

    /// The result phase of a command that searched the disk for a sector and
    /// did not find it: two index holes later.
    fn finish_not_found(&mut self, a: &Active) {
        let wait = self.bytes_until(0) + REVOLUTION_BYTES;
        self.after(
            wait,
            CB,
            Phase::Result {
                bytes: a.result_bytes(),
                pos: 0,
            },
        );
    }

    /// Byte times until the data of the `idx`th sector of the current track.
    fn wait_for_data(&self, a: &Active, idx: usize) -> u64 {
        let count = self.disks[self.unit]
            .as_ref()
            .map_or(1, |d| d.sector_count(self.cylinders[self.unit], a.side));
        self.bytes_until(Self::id_start(count, idx)) + ID_TO_DATA_BYTES
    }

    fn start_read_sector(&mut self, mut a: Active) {
        loop {
            let Some((idx, info)) = self.find_sector(&mut a) else {
                a.st0 |= 0x40;
                a.st1 |= 0x04; // no data
                self.finish_not_found(&a);
                return;
            };
            a.st1 = info.st1 & 0x25;
            a.st2 = info.st2 & 0x61;
            if a.kind == Kind::ReadDeleted {
                a.st2 ^= 0x40; // invert control mark: we want the deleted mark
            }
            let control_mark = a.st2 & 0x40 != 0;
            if a.skip && control_mark {
                if a.r != a.eot {
                    a.r = a.r.wrapping_add(1);
                    continue;
                }
                a.load_status();
                self.finish(&a);
                return;
            }
            if control_mark {
                a.eot = a.r; // execution ends on this sector
            }
            let size = Self::transfer_size(&a);
            let attempt = self.weak_read_counter;
            self.weak_read_counter = self.weak_read_counter.wrapping_add(1);
            let mut buffer = self.disks[self.unit]
                .as_ref()
                .and_then(|d| d.sector_data(self.cylinders[self.unit], a.side, idx, attempt))
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
            buffer.resize(size, 0);
            a.sector_index = idx;
            let wait = self.wait_for_data(&a, idx);
            self.after(
                wait,
                CB,
                Phase::Execution {
                    active: a,
                    dir: Dir::ToCpu,
                    buffer,
                    pos: 0,
                    expected: 0,
                    delay: 1,
                    ready_at: 0,
                },
            );
            return;
        }
    }

    /// A sector's worth of read data has been handed over.
    fn finish_read_sector(&mut self, mut a: Active) {
        self.current_sector = a.sector_index + 1;
        let errored = (a.st1 & 0x31) != 0 || (a.st2 & 0x21) != 0;
        if !errored && a.r != a.eot {
            a.r = a.r.wrapping_add(1);
            self.start_read_sector(a);
        } else {
            a.load_status();
            self.after(
                DATA_CRC_BYTES,
                self.exm() | CB,
                Phase::Result {
                    bytes: a.result_bytes(),
                    pos: 0,
                },
            );
        }
    }

    fn start_read_track(&mut self, mut a: Active) {
        let count = self.disks[self.unit]
            .as_ref()
            .map_or(0, |d| d.sector_count(self.cylinders[self.unit], a.side));
        self.current_sector = 0;
        a.r = 1;
        let size = Self::transfer_size(&a);
        let wanted = usize::from(a.eot).min(count);
        let mut buffer = Vec::with_capacity(size * wanted);
        for idx in 0..wanted {
            let attempt = self.weak_read_counter;
            self.weak_read_counter = self.weak_read_counter.wrapping_add(1);
            let Some(info) = self.disks[self.unit]
                .as_ref()
                .and_then(|d| d.sector_info(self.cylinders[self.unit], a.side, idx))
            else {
                break;
            };
            if idx == 0 {
                let want = SectorId {
                    cylinder: a.c,
                    head: a.h,
                    sector: a.r,
                    size_code: a.n,
                };
                if info.id != want {
                    a.st1 |= 0x04;
                }
            }
            a.st1 |= info.st1 & 0x25;
            a.st2 |= info.st2 & 0x61;
            let mut data = self.disks[self.unit]
                .as_ref()
                .and_then(|d| d.sector_data(self.cylinders[self.unit], a.side, idx, attempt))
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
            data.resize(size, 0);
            buffer.extend(data);
        }
        a.st2 &= 0xBF;
        self.current_sector = wanted % count.max(1);
        if buffer.is_empty() {
            a.load_status();
            self.finish(&a);
            return;
        }
        let wait = self.bytes_until(0) + INDEX_GAP_BYTES + ID_TO_DATA_BYTES;
        self.after(
            wait,
            CB,
            Phase::Execution {
                active: a,
                dir: Dir::ToCpu,
                buffer,
                pos: 0,
                expected: 0,
                delay: 1,
                ready_at: 0,
            },
        );
    }

    fn start_write_sector(&mut self, mut a: Active) {
        let Some((idx, _info)) = self.find_sector(&mut a) else {
            // A scan that finds no sector still ends "normally" (`fdctest3`'s "scan equal (sector not found)").
            if !a.kind.is_scan() {
                a.st0 |= 0x40;
            }
            a.st1 |= 0x04;
            self.finish_not_found(&a);
            return;
        };
        a.sector_index = idx;
        let expected = Self::transfer_size(&a);
        let wait = self.wait_for_data(&a, idx);
        self.after(
            wait,
            CB,
            Phase::Execution {
                active: a,
                dir: Dir::FromCpu,
                buffer: Vec::with_capacity(expected),
                pos: 0,
                expected,
                delay: 1,
                ready_at: 0,
            },
        );
    }

    /// The CPU has supplied a full sector (or the format ID table).
    /// A scanned sector's bytes are in: compare, then end or go on to the next
    /// sector `STP` (the DTL byte) further. Only a CPU byte of `&FF` is "don't
    /// care"; satisfied ends the command with ST2 SH (`&08`) if every byte was
    /// equal, unsatisfied at EOT with ST2 SN (`&04`); ST0 reports a normal end
    /// either way (`fdctest3`'s Scan cases).
    fn finish_scan(&mut self, mut a: Active, cpu: Vec<u8>) {
        let sector = self.disks[self.unit]
            .as_ref()
            .and_then(|d| d.sector_data(self.cylinders[self.unit], a.side, a.sector_index, 0))
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        self.current_sector = a.sector_index + 1;
        let mut equal = true;
        let mut satisfied = true;
        for (i, &want) in cpu.iter().enumerate() {
            if want == 0xFF {
                continue;
            }
            let have = sector.get(i).copied().unwrap_or(0);
            equal &= have == want;
            satisfied &= match a.kind {
                Kind::ScanEqual => have == want,
                Kind::ScanLowOrEqual => have <= want,
                _ => have >= want,
            };
        }
        if satisfied {
            a.st2 = (a.st2 & !0x0C) | if equal { 0x08 } else { 0 };
        } else if a.r != a.eot {
            a.r = a.r.wrapping_add(a.dtl.max(1));
            self.start_write_sector(a);
            return;
        } else {
            a.st2 = (a.st2 & !0x0C) | 0x04;
        }
        self.after(
            DATA_CRC_BYTES,
            self.exm() | CB,
            Phase::Result {
                bytes: a.result_bytes(),
                pos: 0,
            },
        );
    }

    fn finish_write_transfer(&mut self, mut a: Active, buffer: Vec<u8>) {
        if a.kind.is_scan() {
            self.finish_scan(a, buffer);
            return;
        }
        if a.kind == Kind::Format {
            self.finish_format(a, &buffer);
            return;
        }
        let deleted = a.kind == Kind::WriteDeleted;
        let wrote = self.disks[self.unit].as_mut().is_some_and(|d| {
            d.write_sector(
                self.cylinders[self.unit],
                a.side,
                a.sector_index,
                &buffer,
                deleted,
            )
        });
        if !wrote {
            a.st0 |= 0x40;
            a.st1 |= 0x02;
            self.finish(&a);
            return;
        }
        self.current_sector = a.sector_index + 1;
        if a.r != a.eot {
            a.r = a.r.wrapping_add(1);
            self.start_write_sector(a);
        } else {
            a.st0 |= 0x40;
            a.st1 |= 0x80;
            self.after(
                DATA_CRC_BYTES,
                self.exm() | CB,
                Phase::Result {
                    bytes: a.result_bytes(),
                    pos: 0,
                },
            );
        }
    }

    // ---- Format Track (Write ID) ---------------------------------------

    fn begin_format(&mut self, params: &[u8]) {
        // params: HD/US, N, SC, GPL, D
        let mut a = Active {
            kind: Kind::Format,
            unit_byte: params[0],
            c: 0,
            h: params[2], // SC (sectors per track) rides in `h`, like Caprice32
            r: 0,
            n: params[1],
            eot: 0,
            gpl: params[3],
            dtl: params[4], // filler byte D
            skip: false,
            st0: params[0] & 0x07,
            st1: 0,
            st2: 0,
            side: 0,
            sector_index: 0,
        };
        self.select_unit(a.unit_byte);
        if !self.ready(a.unit_byte) {
            a.st0 |= 0x48;
            self.finish_format_result(&a, [0; 4]);
            return;
        }
        a.side = self.side_for(a.unit_byte);
        if self.disks[self.unit]
            .as_ref()
            .is_none_or(D::write_protected)
        {
            a.st0 |= 0x40;
            a.st1 |= 0x02;
            self.finish_format_result(&a, [0; 4]);
            return;
        }
        let expected = usize::from(a.h) * 4;
        if expected == 0 {
            self.finish_format(a, &[]);
            return;
        }
        // From the index hole: the first ID's bytes are asked for after the index gap.
        let wait = self.bytes_until(0) + INDEX_GAP_BYTES;
        self.after(
            wait,
            self.exm() | CB,
            Phase::Execution {
                active: a,
                dir: Dir::FromCpu,
                buffer: Vec::with_capacity(expected),
                pos: 0,
                expected,
                delay: 1,
                ready_at: 0,
            },
        );
    }

    fn finish_format(&mut self, mut a: Active, id_bytes: &[u8]) {
        let ids: Vec<SectorId> = id_bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| SectorId {
                cylinder: c[0],
                head: c[1],
                sector: c[2],
                size_code: c[3],
            })
            .collect();
        let ok = self.disks[self.unit]
            .as_mut()
            .is_some_and(|d| d.format_track(self.cylinders[self.unit], a.side, &ids, a.gpl, a.dtl));
        self.current_sector = 0;
        let last = ids
            .last()
            .map_or([0; 4], |id| [id.cylinder, id.head, id.sector, a.n]);
        if !ok {
            a.st0 |= 0x40;
            a.st1 |= 0x02;
        }
        self.finish_format_result(&a, last);
    }

    fn finish_format_result(&mut self, a: &Active, chrn: [u8; 4]) {
        let result = Phase::Result {
            bytes: vec![a.st0, a.st1, a.st2, chrn[0], chrn[1], chrn[2], chrn[3]],
            pos: 0,
        };
        // A format ends at the next index hole (not at once when it was refused).
        if a.st0 & 0xC0 == 0 {
            let wait = self.bytes_until(0);
            self.after(wait, self.exm() | CB, result);
        } else {
            self.phase = result;
        }
    }
}

#[cfg(feature = "dsk")]
mod dsk_medium;

#[cfg(test)]
mod tests;
