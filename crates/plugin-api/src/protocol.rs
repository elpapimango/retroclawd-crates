use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolEnvelope<T> {
    pub protocol_version: u16,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedValue {
    pub key: String,
    pub value: DebugValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CpuFamily {
    Z80,
    Mos6502,
    Other(String),
}

pub const MACHINE_PROFILE_CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MachineCapability {
    Video,
    Audio,
    Tape,
    Disk,
    Cartridge,
    Snapshot,
    SaveState,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MachineDebugCapability {
    PauseResume,
    StepInstruction,
    Breakpoints,
    Registers,
    AddressSpaces,
    MemoryRead,
    MemoryWrite,
    Disassembly,
    MachineMetadata,
    MediaSnapshot,
    ChipSnapshots,
}

pub const REQUIRED_MACHINE_DEBUG_CAPABILITIES: [MachineDebugCapability; 11] = [
    MachineDebugCapability::PauseResume,
    MachineDebugCapability::StepInstruction,
    MachineDebugCapability::Breakpoints,
    MachineDebugCapability::Registers,
    MachineDebugCapability::AddressSpaces,
    MachineDebugCapability::MemoryRead,
    MachineDebugCapability::MemoryWrite,
    MachineDebugCapability::Disassembly,
    MachineDebugCapability::MachineMetadata,
    MachineDebugCapability::MediaSnapshot,
    MachineDebugCapability::ChipSnapshots,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Tape,
    Disk,
    Cartridge,
    Snapshot,
    Program,
    RomSet,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaSlot {
    Tape,
    Disk { drive: u8 },
    Cartridge { slot: u8 },
    Snapshot,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostMouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

const fn legacy_machine_profile_contract_version() -> u16 {
    0
}

/// Describes one machine-family-specific creation-time option a machine
/// plugin owns (e.g. CPC's `ram_banks`, ZX Spectrum's `speaker_type`).
/// Declared per-`MachineProfile` (not per-plugin) since one plugin crate
/// commonly serves several `machine_id`s with different applicable
/// options -- e.g. `ram_banks`/`rom_slot` only apply to `cpc6128`/
/// `cpc6128plus`, never `gx4000`. The host uses this list to build CLI
/// flags/`retroclawd.toml` schema dynamically at runtime, including for
/// dynamically-loaded plugins the host was never compiled against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MachineOptionKind {
    Bool,
    Int {
        min: Option<i64>,
        max: Option<i64>,
    },
    Text,
    Enum {
        choices: Vec<String>,
    },
    /// A single file path; the host reads it into a `BinaryBlob` keyed by
    /// this option's `key` and adds it to `MachineCreateRequest::roms`.
    File,
    /// A numbered family of file paths (e.g. CPC upper-ROM slots 1-15),
    /// supplied repeatably as `--flag N=path`. The host reads each into a
    /// `BinaryBlob` keyed `"{key}{N}"` in `MachineCreateRequest::roms`.
    FileSet {
        min_index: u32,
        max_index: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineOptionInfo {
    /// `NamedValue`/`BinaryBlob` key this option's value is sent under in
    /// `MachineCreateRequest`.
    pub key: String,
    /// CLI long-flag name, without the leading `--`.
    pub flag: String,
    pub short: Option<char>,
    pub kind: MachineOptionKind,
    /// `None` for `File`/`FileSet` (no meaningful default path).
    pub default: Option<DebugValue>,
    /// Missing value with no default and no config-supplied override is a
    /// hard error, not a silent skip.
    pub required: bool,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineProfile {
    pub machine_id: String,
    pub display_name: String,
    pub cpu_family: CpuFamily,
    pub default_region: Option<String>,
    #[serde(default = "legacy_machine_profile_contract_version")]
    pub contract_version: u16,
    #[serde(default)]
    pub debug_capabilities: Vec<MachineDebugCapability>,
    pub capabilities: Vec<MachineCapability>,
    pub media_kinds: Vec<MediaKind>,
    #[serde(default)]
    pub options: Vec<MachineOptionInfo>,
    /// Every key name this machine's `MachineCommand::KeyEvent` understands
    /// (protocol v7): physical-key names like `KeyA`/`ArrowLeft`, machine-only
    /// keys like `SymShift`/`Joy1Fire1`, and combos like `Exclamation`. The
    /// host validates `[keys]` bindings against this list and offers it in
    /// `--list-keys`. Empty means the plugin does not report names (the host
    /// then passes configured names through unchecked).
    #[serde(default)]
    pub key_names: Vec<MachineKeyInfo>,
}

/// One key name a machine understands, plus friendly spellings the host
/// accepts in configuration and sends back as `name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineKeyInfo {
    /// The exact string `KeyEvent` expects.
    pub name: String,
    pub aliases: Vec<String>,
    /// The characters typing this key produces on the real machine (`!` for a
    /// Spectrum's Symbol Shift+1 key, `;` for the semicolon key, ...). Lets the
    /// host's symbolic layout turn a typed character into this key whatever
    /// the host keyboard layout is. Empty when the key types no ASCII
    /// character. Letters and digits need none: the host derives them from
    /// `KeyA`/`Digit1` style names.
    pub chars: String,
}

/// Lower-cases and drops `_`, `-` and spaces, so `caps_lock`, `CapsLock` and
/// `caps-lock` are one name. Used for matching config text to key names.
pub fn normalize_key_name(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl MachineKeyInfo {
    /// `name` with the aliases every key gets for free: `KeyA` -> `a`,
    /// `Digit1` -> `1`, `ArrowLeft` -> `left`. (Spellings that only differ in
    /// case/underscores, like `caps_lock`, need no alias: matching normalizes.)
    pub fn new(name: &str) -> Self {
        let mut aliases = Vec::new();
        for prefix in ["Key", "Digit", "Arrow"] {
            if let Some(rest) = name.strip_prefix(prefix)
                && !rest.is_empty()
            {
                aliases.push(rest.to_ascii_lowercase());
            }
        }
        MachineKeyInfo {
            name: name.to_string(),
            aliases,
            chars: String::new(),
        }
    }

    /// `self` typing the given characters (see `chars`).
    pub fn with_chars(mut self, chars: &str) -> Self {
        self.chars = chars.to_string();
        self
    }

    pub fn with_aliases(name: &str, extra: &[&str]) -> Self {
        let mut info = Self::new(name);
        info.aliases.extend(extra.iter().map(|a| a.to_string()));
        info
    }

    /// True when `text` names this key (its name or an alias, ignoring case
    /// and `_`/`-`).
    pub fn matches(&self, text: &str) -> bool {
        let wanted = normalize_key_name(text);
        normalize_key_name(&self.name) == wanted
            || self.aliases.iter().any(|a| normalize_key_name(a) == wanted)
    }
}

/// Records which characters each named key types, from `(name, chars)` pairs.
/// Returns the names that are not in `infos` so the caller (a plugin test) can
/// insist the table cannot drift from the key list.
pub fn set_key_chars<'a>(infos: &mut [MachineKeyInfo], table: &[(&'a str, &str)]) -> Vec<&'a str> {
    let mut missing = Vec::new();
    for (name, chars) in table {
        match infos.iter_mut().find(|info| info.name == *name) {
            Some(info) => info.chars = chars.to_string(),
            None => missing.push(*name),
        }
    }
    missing
}

/// Builds the profile's key list from names in table order, dropping repeats
/// (several host keys can map to the same machine key, and tables list each
/// accepted spelling; names that differ only in case/underscores count as one).
pub fn key_infos<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<MachineKeyInfo> {
    let mut seen = std::collections::HashSet::new();
    names
        .into_iter()
        .filter(|name| seen.insert(normalize_key_name(name)))
        .map(MachineKeyInfo::new)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListProfilesRequest {
    pub include_experimental: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListProfilesResponse {
    pub profiles: Vec<MachineProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineCreateRequest {
    pub machine_id: String,
    pub region: Option<String>,
    pub roms: Vec<BinaryBlob>,
    pub options: Vec<NamedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineCreateResponse {
    pub instance: u64,
    pub metadata: MachineMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineInstanceCallRequest {
    pub command: MachineCommand,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineInstanceCallResponse {
    pub response: MachineResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepResult {
    pub cycles: u32,
    pub pc: u16,
    pub halted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterField {
    pub bank: String,
    pub name: String,
    pub value: u64,
    pub width_bits: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterSnapshot {
    pub cpu_family: CpuFamily,
    pub registers: Vec<RegisterField>,
    pub flags: Vec<NamedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressSpaceInfo {
    pub name: String,
    pub addr_width_bits: u8,
    pub bank_context: Vec<NamedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisassemblyLine {
    pub address: u16,
    pub bytes: Vec<u8>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeStatus {
    pub attached: bool,
    pub playing: bool,
    pub finished: bool,
    pub speed_multiplier: u32,
    pub position: Option<u64>,
    pub total: Option<u64>,
    pub motor_on: Option<bool>,
    #[serde(default)]
    pub ear_bit: Option<bool>,
    #[serde(default)]
    pub cycles_until_next_edge: Option<u64>,
    #[serde(default)]
    pub spin_down_cycles: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskStatus {
    pub attached: bool,
    pub image_id: Option<String>,
    pub drive: Option<u8>,
    pub head: Option<u8>,
    pub track: Option<u16>,
    pub sector: Option<u16>,
    pub controller_phase: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartridgeStatus {
    pub attached: bool,
    pub mapper: Option<String>,
    pub active_banks: Vec<NamedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSnapshot {
    pub tape: Option<TapeStatus>,
    pub disks: Vec<DiskStatus>,
    pub cartridge: Option<CartridgeStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryBlob {
    pub key: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChipSnapshot {
    pub chip_id: String,
    pub fields: Vec<NamedValue>,
    pub blobs: Vec<BinaryBlob>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitmapPaletteColor {
    pub name: String,
    pub rgb: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitmapExplorerDefaults {
    pub address: u16,
    pub bytes_width: u16,
    pub bitplanes: u8,
    pub zoom: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineMetadata {
    pub machine_id: String,
    pub model: Option<String>,
    pub region: Option<String>,
    pub notes: Vec<String>,
    #[serde(default)]
    pub bitmap_palette: Vec<BitmapPaletteColor>,
    #[serde(default)]
    pub bitmap_defaults: Option<BitmapExplorerDefaults>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugValue {
    Bool(bool),
    U64(u64),
    I64(i64),
    Text(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugCommand {
    Pause,
    Resume,
    StepInstruction,
    StepOver,
    SetBreakpoint {
        address: u16,
    },
    ClearBreakpoint {
        address: u16,
    },
    ClearAllBreakpoints,
    ListBreakpoints,
    GetRegisters,
    SetRegister {
        name: String,
        value: u64,
    },
    GetAddressSpaces,
    ReadMemory {
        address: u16,
        len: u16,
        space: Option<String>,
    },
    WriteMemory {
        address: u16,
        bytes: Vec<u8>,
        space: Option<String>,
    },
    GetDisassembly {
        start: Option<u16>,
        lines: u16,
    },
    GetMediaSnapshot,
    GetChipSnapshots,
    GetMachineMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugResponse {
    Ack,
    Breakpoints(Vec<u16>),
    Registers(RegisterSnapshot),
    AddressSpaces(Vec<AddressSpaceInfo>),
    Memory(Vec<u8>),
    Disassembly(Vec<DisassemblyLine>),
    MediaSnapshot(MediaSnapshot),
    ChipSnapshots(Vec<ChipSnapshot>),
    MachineMetadata(MachineMetadata),
}

/// One watched I/O port range (a debugger port breakpoint): matches an access
/// when `port & mask == self.port & mask`, so `mask: 0xFFFF` is one exact port
/// and a partial mask covers every mirror a loosely-decoded machine answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortWatchSpec {
    pub port: u16,
    pub mask: u16,
    pub on_read: bool,
    pub on_write: bool,
}

/// The `IN`/`OUT` that tripped a port watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortHitInfo {
    pub port: u16,
    pub value: u8,
    pub write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MachineCommand {
    GetClockHz,
    GetFrameHz,
    RequestInterrupt,
    Step,
    IsHalted,
    GetPc,
    Reset,
    ReadMemory {
        address: u16,
        len: u16,
        space: Option<String>,
    },
    WriteMemory {
        address: u16,
        bytes: Vec<u8>,
        space: Option<String>,
    },
    LoadBytes {
        address: u16,
        bytes: Vec<u8>,
    },
    SetPc {
        address: u16,
    },
    SaveState,
    SaveSnapshot {
        extension: String,
    },
    LoadState {
        bytes: Vec<u8>,
    },
    AttachMedia {
        slot: MediaSlot,
        kind: MediaKind,
        name: Option<String>,
        bytes: Vec<u8>,
        options: Vec<NamedValue>,
    },
    DetachMedia {
        slot: MediaSlot,
    },
    /// Serialises a drive's current disk image (guest writes applied) as
    /// the same file format it was attached as. With `only_if_modified`,
    /// answers `DiskImage(None)` when nothing has been written since the
    /// image was attached or last marked saved -- the cheap "is there
    /// anything to persist?" check a host makes on exit.
    ExportDisk {
        drive: u8,
        only_if_modified: bool,
    },
    /// Records that the exported image has been persisted, so the same
    /// writes aren't reported as unsaved again.
    MarkDiskSaved {
        drive: u8,
    },
    /// Serialises everything the program wrote to the cassette output line
    /// (while the motor ran) as a tape file. `format` is `"cdt"`/`"tzx"`
    /// (pulse-sequence blocks) or `"voc"` (8-bit PCM). Answers
    /// `TapeImage(None)` when nothing was recorded; errors on a machine with
    /// no tape recorder.
    ExportTape {
        format: String,
    },
    /// Presses the freeze button of the cartridge in the expansion port
    /// (Action Replay, Final Cartridge III, ...). Errors when the machine has
    /// no cartridge or the cartridge has no freezer.
    Freeze,
    /// Everything a serial-bus printer has printed, as ASCII text
    /// (`PrinterText(None)` when nothing was); errors on a machine without one.
    ExportPrinter,
    /// Serialises the cartridge as a `.crt` file when its flash was
    /// programmed since it was attached (`CartridgeImage(None)` otherwise);
    /// errors on a machine without a writable cartridge.
    ExportCartridge,
    GetMediaSnapshot,
    SetBorder {
        enabled: bool,
    },
    KeyEvent {
        key: String,
        pressed: bool,
    },
    MouseMotion {
        dx: i8,
        dy: i8,
    },
    MouseButton {
        button: HostMouseButton,
        pressed: bool,
    },
    MouseWheel {
        delta: i8,
    },
    RenderFrame,
    DrainAudio,
    Debug(DebugCommand),
    /// Performs an `OUT (port),value` on the machine's I/O bus, as a CPU
    /// instruction would, but between instructions. Machines with no
    /// I/O-port space (the 6502 family) answer with an error.
    WriteIo {
        port: u16,
        value: u8,
    },
    /// Replaces the machine's set of watched I/O ports (the debugger's port
    /// breakpoints). Errors for a machine with no I/O-port space.
    SetPortWatches {
        watches: Vec<PortWatchSpec>,
    },
    /// Takes the first watched port access since the last call, answered as
    /// `MachineResponse::PortHit`.
    TakePortHit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MachineResponse {
    Ack,
    ClockHz(u64),
    FrameHz(u64),
    Step(StepResult),
    Halted(bool),
    Pc(u16),
    Memory(Vec<u8>),
    BytesWritten(u32),
    StateBlob(Vec<u8>),
    SnapshotBlob(Vec<u8>),
    /// Answer to `ExportDisk`: the image bytes, or `None` when there's
    /// nothing to export (no disk, or unmodified with `only_if_modified`).
    DiskImage(Option<Vec<u8>>),
    /// Answer to `ExportTape`: the tape file bytes, or `None` when nothing
    /// was recorded.
    TapeImage(Option<Vec<u8>>),
    /// Answer to `ExportPrinter`: the text bytes, or `None` when nothing was printed.
    PrinterText(Option<Vec<u8>>),
    /// Answer to `ExportCartridge`: the `.crt` bytes, or `None` when the
    /// cartridge was not modified.
    CartridgeImage(Option<Vec<u8>>),
    /// Answer to `TakePortHit`.
    PortHit(Option<PortHitInfo>),
    MediaSnapshot(MediaSnapshot),
    Frame {
        width: u32,
        height: u32,
        rgb: Vec<u8>,
    },
    AudioPcmMono {
        sample_rate: u32,
        samples: Vec<f32>,
    },
    Debug(DebugResponse),
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde::Serialize;

    use super::*;

    #[test]
    fn machine_command_round_trip_keeps_nested_debug_payload() {
        let command = MachineCommand::Debug(DebugCommand::ReadMemory {
            address: 0x8000,
            len: 64,
            space: Some("cpu".to_string()),
        });

        let bytes = bincode::serde::encode_to_vec(&command, bincode::config::standard())
            .expect("encode command");
        let (decoded, _): (MachineCommand, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                .expect("decode command");

        assert_eq!(decoded, command);
    }

    #[test]
    fn machine_command_round_trip_keeps_input_and_control_payloads() {
        let command = MachineCommand::SetBorder { enabled: true };

        let bytes = bincode::serde::encode_to_vec(&command, bincode::config::standard())
            .expect("encode command");
        let (decoded, _): (MachineCommand, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                .expect("decode command");

        assert_eq!(decoded, command);
    }

    #[test]
    fn machine_metadata_round_trip_keeps_bitmap_palette_defaults() {
        let metadata = MachineMetadata {
            machine_id: "zx-spectrum".to_string(),
            model: Some("48k".to_string()),
            region: Some("pal".to_string()),
            notes: Vec::new(),
            bitmap_palette: vec![BitmapPaletteColor {
                name: "black".to_string(),
                rgb: [0, 0, 0],
            }],
            bitmap_defaults: Some(BitmapExplorerDefaults {
                address: 0x4000,
                bytes_width: 32,
                bitplanes: 1,
                zoom: 3,
            }),
        };
        let bytes = crate::encode_message(&metadata).expect("encode metadata");
        let decoded: MachineMetadata = crate::decode_message(&bytes).expect("decode metadata");
        assert_eq!(decoded, metadata);
    }

    #[test]
    fn machine_profile_wire_format_change_requires_rebuild() {
        #[derive(Serialize)]
        struct LegacyMachineProfile {
            machine_id: String,
            display_name: String,
            cpu_family: CpuFamily,
            default_region: Option<String>,
            capabilities: Vec<MachineCapability>,
            media_kinds: Vec<MediaKind>,
        }

        let legacy = LegacyMachineProfile {
            machine_id: "zx81".to_string(),
            display_name: "zx81".to_string(),
            cpu_family: CpuFamily::Z80,
            default_region: Some("pal".to_string()),
            capabilities: vec![MachineCapability::Video, MachineCapability::Debug],
            media_kinds: vec![MediaKind::Snapshot],
        };

        let bytes = bincode::serde::encode_to_vec(&legacy, bincode::config::standard())
            .expect("encode legacy profile");
        let err = bincode::serde::decode_from_slice::<MachineProfile, _>(
            &bytes,
            bincode::config::standard(),
        )
        .expect_err("legacy payload must not decode as current MachineProfile");

        assert!(matches!(
            err,
            bincode::error::DecodeError::UnexpectedEnd { .. }
        ));
    }

    #[test]
    fn key_names_match_ignoring_case_underscores_and_the_free_aliases() {
        let key = MachineKeyInfo::new("KeyA");
        assert!(key.matches("a") && key.matches("KEYA") && key.matches("key_a"));
        let arrow = MachineKeyInfo::new("ArrowLeft");
        assert!(arrow.matches("left") && arrow.matches("arrow_left"));
        assert!(!arrow.matches("right"));
        assert!(MachineKeyInfo::new("Digit5").matches("5"));
        let caps = MachineKeyInfo::new("CapsLock");
        assert!(caps.matches("caps_lock") && !caps.matches("caps"));
        let sym = MachineKeyInfo::with_aliases("SymShift", &["symbol_shift"]);
        assert!(sym.matches("sym_shift") && sym.matches("Symbol-Shift"));
    }

    #[test]
    fn key_infos_drops_repeated_names_in_order() {
        let infos = key_infos(["KeyA", "KeyB", "KeyA", "Space"]);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["KeyA", "KeyB", "Space"]);
    }

    #[test]
    fn required_debug_capability_table_has_no_duplicates() {
        let mut seen = HashSet::new();
        for capability in REQUIRED_MACHINE_DEBUG_CAPABILITIES {
            assert!(seen.insert(capability));
        }
    }
}
