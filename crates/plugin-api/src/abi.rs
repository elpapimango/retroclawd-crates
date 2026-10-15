use core::ffi::c_void;

pub const ABI_MAJOR_V1: u16 = 1;
/// Bumped 0 -> 1 to add `MachinePluginVTableV1`'s `step`/`halted`/`pc` fast-
/// path fields (see that struct's own doc comment) -- a real vtable-shape
/// change, not just a version bump for its own sake. Safe specifically
/// because `PluginHost::from_descriptor` (host-side) checks `abi_major`/
/// `abi_minor` BEFORE ever dereferencing `descriptor.vtable` at all: an old
/// plugin built against minor `0` is cleanly rejected by that check (clear
/// error message, no vtable access), never read through the new, larger
/// struct shape. The vtable's own `size` field is a secondary, self-
/// description check for graceful *forward*-compatibility (a newer plugin
/// talking to an older host) -- it is NOT what makes shrinking/growing this
/// struct safe in the other direction, since it's only inspected after the
/// struct has already been read out of `descriptor.vtable` at its (now
/// larger) `Self` size.
pub const ABI_MINOR_V1: u16 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RcSlice {
    pub ptr: *const u8,
    pub len: usize,
}

impl RcSlice {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ptr: core::ptr::null(),
            len: 0,
        }
    }

    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        }
    }

    /// # Safety
    ///
    /// The caller must ensure `ptr` points to at least `len` readable bytes
    /// for the returned lifetime. This does not validate ownership.
    #[must_use]
    pub unsafe fn as_bytes<'a>(self) -> &'a [u8] {
        if self.len == 0 {
            return &[];
        }
        debug_assert!(!self.ptr.is_null());
        // SAFETY: upheld by caller contract above.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RcMutSlice {
    pub ptr: *mut u8,
    pub len: usize,
}

impl RcMutSlice {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ptr: core::ptr::null_mut(),
            len: 0,
        }
    }

    /// # Safety
    ///
    /// The caller must ensure `ptr` points to at least `len` writable bytes
    /// for the returned lifetime. This does not validate ownership.
    #[must_use]
    pub unsafe fn as_mut_bytes<'a>(self) -> &'a mut [u8] {
        if self.len == 0 {
            return &mut [];
        }
        debug_assert!(!self.ptr.is_null());
        // SAFETY: upheld by caller contract above.
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RcBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
}

impl RcBuffer {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ptr: core::ptr::null_mut(),
            len: 0,
            cap: 0,
        }
    }

    #[must_use]
    pub fn from_vec(mut bytes: Vec<u8>) -> Self {
        let out = Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            cap: bytes.capacity(),
        };
        core::mem::forget(bytes);
        out
    }

    /// # Safety
    ///
    /// `self` must have been produced by `RcBuffer::from_vec` in this same
    /// allocator domain, and must not be consumed more than once.
    #[must_use]
    pub unsafe fn into_vec(self) -> Vec<u8> {
        if self.ptr.is_null() {
            return Vec::new();
        }
        // SAFETY: upheld by caller contract above.
        unsafe { Vec::from_raw_parts(self.ptr, self.len, self.cap) }
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginKind {
    Unknown = 0,
    Machine = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusCode {
    Ok = 0,
    InvalidRequest = 1,
    NotFound = 2,
    Unsupported = 3,
    InternalError = 4,
    AbiMajorMismatch = 5,
    AbiMinorMismatch = 6,
    DecodeError = 7,
    EncodeError = 8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RcStatus {
    pub code: u32,
}

impl RcStatus {
    #[must_use]
    pub const fn ok() -> Self {
        Self {
            code: StatusCode::Ok as u32,
        }
    }

    #[must_use]
    pub const fn from_code(code: StatusCode) -> Self {
        Self { code: code as u32 }
    }

    #[must_use]
    pub const fn is_ok(self) -> bool {
        self.code == StatusCode::Ok as u32
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RcCallResult {
    pub status: RcStatus,
    pub payload: RcBuffer,
}

impl RcCallResult {
    #[must_use]
    pub const fn empty_ok() -> Self {
        Self {
            status: RcStatus::ok(),
            payload: RcBuffer::empty(),
        }
    }

    #[must_use]
    pub const fn error(status: StatusCode) -> Self {
        Self {
            status: RcStatus::from_code(status),
            payload: RcBuffer::empty(),
        }
    }

    /// An error carrying a human-readable explanation as UTF-8 text in the
    /// payload (the host shows it instead of just the status code). Plugins
    /// that do not attach one still work: the host treats an empty error
    /// payload as "no message".
    #[must_use]
    pub fn error_with_message(status: StatusCode, message: &str) -> Self {
        Self {
            status: RcStatus::from_code(status),
            payload: RcBuffer::from_vec(message.as_bytes().to_vec()),
        }
    }

    #[must_use]
    pub fn ok_bytes(bytes: Vec<u8>) -> Self {
        Self {
            status: RcStatus::ok(),
            payload: RcBuffer::from_vec(bytes),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PluginDescriptorV1 {
    pub abi_major: u16,
    pub abi_minor: u16,
    pub kind: PluginKind,
    pub id: RcSlice,
    pub version: RcSlice,
    pub vtable: *const c_void,
}

pub type PluginEntryV1 = unsafe extern "C" fn() -> *const PluginDescriptorV1;

pub type MachineInstanceHandle = u64;

pub type FreeBufferFnV1 = unsafe extern "C" fn(buffer: RcBuffer);

pub type MachineListProfilesFnV1 = unsafe extern "C" fn(req: RcSlice) -> RcCallResult;
pub type MachineCreateInstanceFnV1 = unsafe extern "C" fn(req: RcSlice) -> RcCallResult;
pub type MachineDestroyInstanceFnV1 =
    unsafe extern "C" fn(instance: MachineInstanceHandle) -> RcStatus;
pub type MachineInstanceCallFnV1 =
    unsafe extern "C" fn(instance: MachineInstanceHandle, req: RcSlice) -> RcCallResult;

/// Fast-path single-instruction step: returns cycles consumed directly, no
/// `MachineCommand`/bincode round trip at all. See `MachinePluginVTableV1`'s
/// own doc comment for why this exists and what it deliberately excludes.
/// Returns `0` for an instance handle the plugin doesn't recognize -- same
/// "never happens in practice, host always passes a handle it just got back
/// from `create_instance`" contract `instance_call` itself already relies on
/// implicitly (it would simply return a `NotFound` status instead), just
/// with no status code slot to report it through here.
pub type MachineStepFnV1 = unsafe extern "C" fn(instance: MachineInstanceHandle) -> u32;
/// Fast-path halted query -- `0`/`1` rather than `bool` so the FFI boundary
/// never depends on `bool`'s representation being read the same way on both
/// sides of a `dylib` boundary (matches this ABI's existing preference for
/// plain integer types everywhere else in this file). Reads `1` for an
/// unrecognized instance handle -- the safer of the two possible sentinel
/// values for a caller that only ever uses this to decide whether to keep
/// stepping.
pub type MachineHaltedFnV1 = unsafe extern "C" fn(instance: MachineInstanceHandle) -> u8;
/// Fast-path program-counter query, mirroring `Machine::pc`'s own "cheap
/// enough to call after every single `step()`" contract -- see that
/// method's doc comment in `crates/machine/src/lib.rs`. Reads `0` for an
/// unrecognized instance handle.
pub type MachinePcFnV1 = unsafe extern "C" fn(instance: MachineInstanceHandle) -> u16;

/// `size` + the 5 original fields are ABI-minor-`0`'s complete shape;
/// `step`/`halted`/`pc` were appended in ABI minor `1` (see `ABI_MINOR_V1`'s
/// own doc comment for why appending is safe here specifically). These
/// three exist purely to bypass the `instance_call`/bincode round trip for
/// the handful of `Machine` trait methods actually proven hot -- every
/// emulated CPU instruction calls `step()` once, and a debugger-active
/// step-over loop calls `pc()`/`halted()` per instruction too (see
/// `apps/retroclawd/src/debugger.rs`'s `execute_step_over_instruction`) --
/// profiled and confirmed via this project's own investigation into why
/// the plugin split gave no measured speed win: every single step was
/// paying a full heap-allocating serialize+deserialize round trip even in
/// bundled mode, where no real process/address-space boundary exists to
/// justify one. Deliberately NOT extended to `clock_hz`/`frame_hz`
/// (called once per tick, not once per instruction) or `render_frame`/
/// `drain_audio` (already `Vec<u8>`-returning and called only 50-60x/sec,
/// nowhere near hot) -- see `CLAUDE.md`'s own note on this change for the
/// full rationale; no optimization without a measured bottleneck.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MachinePluginVTableV1 {
    pub size: u32,
    pub list_profiles: MachineListProfilesFnV1,
    pub create_instance: MachineCreateInstanceFnV1,
    pub destroy_instance: MachineDestroyInstanceFnV1,
    pub instance_call: MachineInstanceCallFnV1,
    pub free_buffer: FreeBufferFnV1,
    pub step: MachineStepFnV1,
    pub halted: MachineHaltedFnV1,
    pub pc: MachinePcFnV1,
}

impl MachinePluginVTableV1 {
    #[must_use]
    pub const fn expected_size() -> u32 {
        core::mem::size_of::<Self>() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_buffer_round_trips_vec() {
        let original = vec![1, 2, 3, 4, 5];
        let buffer = RcBuffer::from_vec(original.clone());
        // SAFETY: `buffer` was created by `RcBuffer::from_vec` above and is
        // consumed exactly once in this allocator domain.
        let restored = unsafe { buffer.into_vec() };
        assert_eq!(restored, original);
    }

    #[test]
    fn rc_status_ok_reports_ok() {
        assert!(RcStatus::ok().is_ok());
        assert!(!RcStatus::from_code(StatusCode::InternalError).is_ok());
    }

    #[test]
    fn vtable_sizes_are_non_zero() {
        assert!(MachinePluginVTableV1::expected_size() > 0);
    }

    /// The binary layout is the ABI. These pin it on the 64-bit targets the
    /// project ships for (Windows, Linux, macOS): a failure here means a
    /// struct was changed in a way existing plugin libraries cannot follow.
    /// The fix is never to edit the numbers: appending a vtable field is a
    /// minor bump (and these sizes change with it, deliberately, in the same
    /// commit); anything else is a new major version. See `docs/plugin-abi.md`.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_binary_layout_matches_the_published_abi() {
        use core::mem::{offset_of, size_of};

        assert_eq!(size_of::<RcSlice>(), 16);
        assert_eq!(size_of::<RcMutSlice>(), 16);
        assert_eq!(size_of::<RcBuffer>(), 24);
        assert_eq!(size_of::<RcStatus>(), 4);
        assert_eq!(size_of::<RcCallResult>(), 32);

        // The descriptor is frozen for the whole major version; the first
        // two fields are what a host reads from a plugin it cannot interpret.
        assert_eq!(size_of::<PluginDescriptorV1>(), 48);
        assert_eq!(offset_of!(PluginDescriptorV1, abi_major), 0);
        assert_eq!(offset_of!(PluginDescriptorV1, abi_minor), 2);
        assert_eq!(offset_of!(PluginDescriptorV1, kind), 4);
        assert_eq!(offset_of!(PluginDescriptorV1, id), 8);
        assert_eq!(offset_of!(PluginDescriptorV1, version), 24);
        assert_eq!(offset_of!(PluginDescriptorV1, vtable), 40);

        // Vtables start with `size`, then only ever grow at the end.
        assert_eq!(offset_of!(MachinePluginVTableV1, size), 0);
        assert_eq!(offset_of!(MachinePluginVTableV1, list_profiles), 8);
        assert_eq!(offset_of!(MachinePluginVTableV1, create_instance), 16);
        assert_eq!(offset_of!(MachinePluginVTableV1, destroy_instance), 24);
        assert_eq!(offset_of!(MachinePluginVTableV1, instance_call), 32);
        assert_eq!(offset_of!(MachinePluginVTableV1, free_buffer), 40);
        assert_eq!(offset_of!(MachinePluginVTableV1, step), 48);
        assert_eq!(offset_of!(MachinePluginVTableV1, halted), 56);
        assert_eq!(offset_of!(MachinePluginVTableV1, pc), 64);
        assert_eq!(size_of::<MachinePluginVTableV1>(), 72);
        // ABI minor 0 ended after `free_buffer`.
        assert_eq!(ABI_MINOR_V1, 1);
    }

    #[test]
    fn the_vtable_size_field_describes_its_own_struct() {
        assert_eq!(
            MachinePluginVTableV1::expected_size() as usize,
            core::mem::size_of::<MachinePluginVTableV1>()
        );
    }
}
