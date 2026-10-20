pub mod abi;
pub mod codec;
pub mod compat;
pub mod execution_mode;
pub mod guard;
pub mod protocol;

pub use abi::{
    ABI_MAJOR_V1, ABI_MINOR_V1, MachineInstanceHandle, MachinePluginVTableV1, PluginDescriptorV1,
    PluginEntryV1, PluginKind, RcBuffer, RcCallResult, RcMutSlice, RcSlice, RcStatus, StatusCode,
};
pub use codec::{CodecError, decode, decode_message, encode, encode_message};
pub use execution_mode::ExecutionMode;
pub use protocol::{
    AddressSpaceInfo, BinaryBlob, BitmapExplorerDefaults, BitmapPaletteColor, CartridgeStatus,
    ChipSnapshot, CpuFamily, CpuInfo, DebugCommand, DebugResponse, DebugValue, DisassemblyLine,
    DiskStatus, HostMouseButton, ListProfilesRequest, ListProfilesResponse,
    MACHINE_PROFILE_CONTRACT_VERSION, MachineCapability, MachineCommand, MachineCreateRequest,
    MachineCreateResponse, MachineDebugCapability, MachineInstanceCallRequest,
    MachineInstanceCallResponse, MachineKeyInfo, MachineMetadata, MachineOptionInfo,
    MachineOptionKind, MachineProfile, MachineResponse, MediaKind, MediaSlot, MediaSnapshot,
    NamedValue, PROTOCOL_VERSION, PortHitInfo, PortWatchSpec, ProtocolEnvelope,
    REQUIRED_MACHINE_DEBUG_CAPABILITIES, RegisterField, RegisterSnapshot, StepResult, TapeStatus,
    key_infos, normalize_key_name, set_key_chars,
};

/// Generates the two C entry points the host looks for in a plugin library
/// (`retroclawd_plugin_entry_v1` and its older alias `plugin_entry_v1`), both
/// forwarding to `$entry`, an `unsafe extern "C" fn() -> *const
/// PluginDescriptorV1` from the plugin's implementation crate.
///
/// Each plugin is split in two crates: an ordinary library holding all the
/// logic (no exported symbols, so a host or a test may link several of them)
/// and a tiny `cdylib` crate that only invokes this macro. The shared library
/// is therefore always a complete plugin, whatever else the workspace
/// builds or links.
#[macro_export]
macro_rules! export_plugin {
    ($entry:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn retroclawd_plugin_entry_v1() -> *const $crate::PluginDescriptorV1 {
            // SAFETY: the entry returns a pointer to immutable static data.
            unsafe { $entry() }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn plugin_entry_v1() -> *const $crate::PluginDescriptorV1 {
            // SAFETY: as above.
            unsafe { $entry() }
        }
    };
}
