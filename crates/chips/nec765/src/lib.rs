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
//! Deliberate simplifications (documented, not hardware claims): execution
//! is instantaneous (no rotational latency, no overrun -- a byte the CPU is
//! slow to fetch is never lost); the `MT` (multi-track) bit is accepted but
//! not acted on; the Scan commands report "invalid command"; two drives exist
//! (units 0 and 1; a command addressing another unit, or an empty one, finds it not ready); seek time
//! is opt-in via `enable_seek_timing`.

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
        | op::READ_DELETED_DATA => Some(8),
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
}

impl Kind {
    fn is_write(self) -> bool {
        matches!(self, Kind::WriteData | Kind::WriteDeleted | Kind::Format)
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
    },
    Result {
        bytes: Vec<u8>,
        pos: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SeekState {
    None,
    InFlight { remaining: u32, st0: u8, pcn: u8 },
    Done { st0: u8, pcn: u8 },
}

/// How many drives the controller addresses (unit select 0 and 1).
pub const DRIVES: usize = 2;

/// `Fdc`'s own live state, for save states -- deliberately excludes the
/// inserted disk (`Fdc::disk`); a host that lets the disk be modified
/// saves the modified image itself (see `cpc`/`zx_spectrum` save states).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FdcState {
    cylinders: [u8; DRIVES],
    unit: usize,
    current_sector: usize,
    phase: Phase,
    seek: SeekState,
    seek_timing_enabled: bool,
    weak_read_counter: u32,
    motor: bool,
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
            Phase::Execution { .. } => "execution",
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
    seek: SeekState,
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
            seek: SeekState::None,
            seek_timing_enabled: false,
            weak_read_counter: 0,
            motor: true,
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
        if let SeekState::InFlight {
            remaining,
            st0,
            pcn,
        } = self.seek
        {
            let remaining = remaining.saturating_sub(tstates);
            self.seek = if remaining == 0 {
                SeekState::Done { st0, pcn }
            } else {
                SeekState::InFlight {
                    remaining,
                    st0,
                    pcn,
                }
            };
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
    }

    /// Removes and returns the disk in `unit`, if any.
    pub fn eject_disk_from(&mut self, unit: usize) -> Option<D> {
        self.disks.get_mut(unit).and_then(Option::take)
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
            seek: self.seek,
            seek_timing_enabled: self.seek_timing_enabled,
            weak_read_counter: self.weak_read_counter,
            motor: self.motor,
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
        self.seek = state.seek;
        self.seek_timing_enabled = state.seek_timing_enabled;
        self.weak_read_counter = state.weak_read_counter;
        self.motor = state.motor;
    }

    /// Main status register. Idle `0x80`; receiving parameters `0x90`
    /// (busy); execution `EXM|CB|RQM` plus `DIO` when the controller is
    /// sending data (`0xF0`) or without it when receiving (`0xB0`); result
    /// `0xD0`. The first read after an execution phase starts reports
    /// not-ready (`0x10`) once, like Caprice32.
    pub fn read_msr(&mut self) -> u8 {
        match &mut self.phase {
            Phase::Idle => RQM,
            // The command phase only exists once the opcode byte has
            // arrived, so the controller is already busy.
            Phase::Command { .. } => RQM | CB,
            Phase::Execution { dir, delay, .. } => {
                if *delay > 0 {
                    *delay -= 1;
                    return CB;
                }
                let mut v = RQM | EXM | CB;
                if *dir == Dir::ToCpu {
                    v |= DIO;
                }
                v
            }
            Phase::Result { .. } => RQM | DIO | CB,
        }
    }

    /// Data register read (execution-read data or result byte). Returns
    /// `0xFF` when the controller has nothing to send.
    pub fn read_data(&mut self) -> u8 {
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
            } => {
                let byte = buffer.get(pos).copied().unwrap_or(0xFF);
                let pos = pos + 1;
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
            } => {
                buffer.push(val);
                if buffer.len() >= expected {
                    self.finish_write_transfer(active, buffer);
                } else {
                    self.phase = Phase::Execution {
                        active,
                        dir: Dir::FromCpu,
                        buffer,
                        pos,
                        expected,
                        delay,
                    };
                }
            }
            other => self.phase = other,
        }
    }

    fn execute(&mut self, opcode: u8, params: &[u8]) {
        let skip = opcode & 0x20 != 0;
        match opcode & 0x1F {
            op::SPECIFY => self.phase = Phase::Idle, // step/head timing not modelled
            op::SENSE_DRIVE_STATUS => self.sense_drive_status(params[0]),
            op::RECALIBRATE => self.seek(params[0], 0),
            op::SEEK => self.seek(params[0], params[1]),
            op::SENSE_INTERRUPT_STATUS => self.sense_interrupt_status(),
            op::READ_ID => self.read_id(params[0]),
            op::READ_DATA => self.begin_transfer(Kind::ReadData, skip, params),
            op::READ_DELETED_DATA => self.begin_transfer(Kind::ReadDeleted, skip, params),
            op::READ_TRACK => self.begin_transfer(Kind::ReadTrack, false, params),
            op::WRITE_DATA => self.begin_transfer(Kind::WriteData, false, params),
            op::WRITE_DELETED_DATA => self.begin_transfer(Kind::WriteDeleted, false, params),
            op::FORMAT_TRACK => self.begin_format(params),
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
        let unit = usize::from(unit_byte & 0x03);
        if unit < DRIVES {
            self.unit = unit;
        }
    }

    /// The addressed unit must exist, hold a disk and have its motor spinning (the CPC has one motor line for both drives).
    fn ready(&self, unit_byte: u8) -> bool {
        let unit = usize::from(unit_byte & 0x03);
        unit < DRIVES && self.disks[unit].is_some() && self.motor
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

    fn seek(&mut self, unit_byte: u8, target: u8) {
        self.select_unit(unit_byte);
        let ready = self.ready(unit_byte);
        let mut st0 = unit_byte & 0x07;
        let mut distance = 1;
        if ready {
            distance = u32::from(target.abs_diff(self.cylinders[self.unit]).max(1));
            self.cylinders[self.unit] = target;
        } else {
            st0 |= 0x48; // abnormal termination + not ready
        }
        let pcn = self.cylinders[self.unit];
        self.seek = if self.seek_timing_enabled {
            SeekState::InFlight {
                remaining: distance * SEEK_TSTATES_PER_TRACK,
                st0,
                pcn,
            }
        } else {
            SeekState::Done { st0, pcn }
        };
        self.phase = Phase::Idle; // Seek/Recalibrate have no result phase
    }

    fn sense_interrupt_status(&mut self) {
        let bytes = match self.seek {
            SeekState::Done { st0, pcn } => {
                self.seek = SeekState::None;
                vec![(st0 & 0xFB) | 0x20, pcn] // ST0 keeps US1/US0: the unit that finished the seek
            }
            // Nothing pending (or the seek is still travelling): the chip
            // answers "invalid command", and firmware polls again.
            _ => vec![0x80],
        };
        self.phase = Phase::Result { bytes, pos: 0 };
    }

    fn read_id(&mut self, unit_byte: u8) {
        self.select_unit(unit_byte);
        let mut st0 = unit_byte & 0x07;
        let mut st1 = 0;
        let (mut c, mut h, mut r, mut n) = (self.cylinders[self.unit], (unit_byte >> 2) & 1, 0, 0);
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
            } else {
                let mut idx = self.current_sector;
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
        self.phase = Phase::Result {
            bytes: vec![st0, st1, 0, c, h, r, n],
            pos: 0,
        };
    }

    // ---- data transfer commands ----------------------------------------

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

    fn start_read_sector(&mut self, mut a: Active) {
        loop {
            let Some((idx, info)) = self.find_sector(&mut a) else {
                a.st0 |= 0x40;
                a.st1 |= 0x04; // no data
                self.finish(&a);
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
            self.phase = Phase::Execution {
                active: a,
                dir: Dir::ToCpu,
                buffer,
                pos: 0,
                expected: 0,
                delay: 1,
            };
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
            self.finish(&a);
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
        self.phase = Phase::Execution {
            active: a,
            dir: Dir::ToCpu,
            buffer,
            pos: 0,
            expected: 0,
            delay: 1,
        };
    }

    fn start_write_sector(&mut self, mut a: Active) {
        let Some((idx, _info)) = self.find_sector(&mut a) else {
            a.st0 |= 0x40;
            a.st1 |= 0x04;
            self.finish(&a);
            return;
        };
        a.sector_index = idx;
        let expected = Self::transfer_size(&a);
        self.phase = Phase::Execution {
            active: a,
            dir: Dir::FromCpu,
            buffer: Vec::with_capacity(expected),
            pos: 0,
            expected,
            delay: 1,
        };
    }

    /// The CPU has supplied a full sector (or the format ID table).
    fn finish_write_transfer(&mut self, mut a: Active, buffer: Vec<u8>) {
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
            self.finish(&a);
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
        self.phase = Phase::Execution {
            active: a,
            dir: Dir::FromCpu,
            buffer: Vec::with_capacity(expected),
            pos: 0,
            expected,
            delay: 1,
        };
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
        self.phase = Phase::Result {
            bytes: vec![a.st0, a.st1, a.st2, chrn[0], chrn[1], chrn[2], chrn[3]],
            pos: 0,
        };
    }
}

#[cfg(feature = "dsk")]
mod dsk_medium;

#[cfg(test)]
mod tests;
