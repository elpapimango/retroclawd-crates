//! The host's compatibility rules for a plugin, in one place so the machine
//! loader (and the tests that pin them) cannot drift apart.
//! `docs/plugin-abi.md` is the human-readable policy these functions enforce.

use crate::abi::{PluginDescriptorV1, PluginKind};

/// Reads only the first two fields of a descriptor -- `abi_major` and
/// `abi_minor`, whose offsets are frozen for the life of the project -- so a
/// host can tell "this is a different major version" without interpreting
/// (or copying) a descriptor whose layout it does not know.
///
/// # Safety
///
/// `ptr` must be non-null and point at memory that is readable for at least
/// four bytes (any descriptor of any major version is).
#[must_use]
pub unsafe fn peek_abi_version(ptr: *const PluginDescriptorV1) -> (u16, u16) {
    let bytes = ptr.cast::<u16>();
    // SAFETY: caller contract; both fields are `u16` at offsets 0 and 2.
    unsafe { (bytes.read_unaligned(), bytes.add(1).read_unaligned()) }
}

/// Turns the pointer a plugin's entry point returned into a descriptor, but
/// only after the major version matched `host_major` (so the full
/// `PluginDescriptorV1` layout is known to apply).
///
/// # Safety
///
/// `ptr` must be null or point at a descriptor that stays valid for `'a`
/// (the entry point contract: immutable static data of the plugin).
pub unsafe fn descriptor_from_entry<'a>(
    ptr: *const PluginDescriptorV1,
    label: &str,
    host_major: u16,
) -> Result<&'a PluginDescriptorV1, String> {
    if ptr.is_null() {
        return Err(format!("{label} plugin entry returned null descriptor"));
    }
    // SAFETY: non-null, and the first four bytes exist in every version.
    let (major, _) = unsafe { peek_abi_version(ptr) };
    if major != host_major {
        return Err(format!(
            "{label} plugin ABI major mismatch: host={host_major}, plugin={major}"
        ));
    }
    // SAFETY: the major matches, so this is a `PluginDescriptorV1`.
    Ok(unsafe { &*ptr })
}

/// The ABI part of the policy: same major, plugin minor at least the host's
/// (the host calls vtable fields added in its minor), the expected kind and a
/// vtable pointer. A newer plugin minor is accepted -- the host simply does
/// not use what it does not know.
pub fn check_descriptor(
    descriptor: &PluginDescriptorV1,
    label: &str,
    expected_kind: PluginKind,
    host_major: u16,
    host_minor: u16,
) -> Result<(), String> {
    if descriptor.abi_major != host_major {
        return Err(format!(
            "{label} plugin ABI major mismatch: host={host_major}, plugin={}",
            descriptor.abi_major
        ));
    }
    if descriptor.abi_minor < host_minor {
        return Err(format!(
            "{label} plugin ABI minor too old: host requires >= {host_minor}, plugin={}",
            descriptor.abi_minor
        ));
    }
    if descriptor.kind != expected_kind {
        return Err(format!("descriptor is not a {label} plugin"));
    }
    if descriptor.vtable.is_null() {
        return Err(format!("{label} plugin descriptor has null vtable"));
    }
    Ok(())
}

/// A vtable must be at least as large as the host's own definition of it
/// (it may be larger: a newer minor appended fields).
pub fn check_vtable_size(label: &str, expected: u32, got: u32) -> Result<(), String> {
    if got < expected {
        return Err(format!(
            "{label} plugin vtable too small: expected at least {expected}, got {got}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use core::ffi::c_void;

    use super::*;
    use crate::abi::{ABI_MAJOR_V1, ABI_MINOR_V1, RcSlice};

    fn descriptor(major: u16, minor: u16, kind: PluginKind) -> PluginDescriptorV1 {
        static ANCHOR: u8 = 0;
        PluginDescriptorV1 {
            abi_major: major,
            abi_minor: minor,
            kind,
            id: RcSlice::empty(),
            version: RcSlice::empty(),
            vtable: (&ANCHOR as *const u8).cast::<c_void>(),
        }
    }

    #[test]
    fn the_abi_prefix_is_readable_without_knowing_the_rest_of_the_layout() {
        let d = descriptor(7, 9, PluginKind::Machine);
        // SAFETY: a live descriptor.
        assert_eq!(unsafe { peek_abi_version(&d) }, (7, 9));
    }

    #[test]
    fn a_descriptor_of_another_major_is_refused_before_it_is_interpreted() {
        let d = descriptor(ABI_MAJOR_V1 + 1, 0, PluginKind::Machine);
        // SAFETY: a live descriptor.
        let err = unsafe { descriptor_from_entry(&d, "machine", ABI_MAJOR_V1) }.unwrap_err();
        assert_eq!(err, "machine plugin ABI major mismatch: host=1, plugin=2");
        // SAFETY: null is allowed.
        let null = unsafe { descriptor_from_entry(core::ptr::null(), "machine", ABI_MAJOR_V1) };
        assert!(null.unwrap_err().contains("null descriptor"));
    }

    #[test]
    fn the_policy_accepts_equal_and_newer_minors_and_rejects_older_ones() {
        let host = (ABI_MAJOR_V1, ABI_MINOR_V1);
        let check = |minor| {
            check_descriptor(
                &descriptor(ABI_MAJOR_V1, minor, PluginKind::Machine),
                "machine",
                PluginKind::Machine,
                host.0,
                host.1,
            )
        };
        assert!(check(host.1).is_ok());
        assert!(check(host.1 + 5).is_ok());
        assert_eq!(
            check(host.1 - 1).unwrap_err(),
            "machine plugin ABI minor too old: host requires >= 2, plugin=1"
        );
    }

    #[test]
    fn kind_and_vtable_are_checked() {
        let wrong = descriptor(ABI_MAJOR_V1, ABI_MINOR_V1, PluginKind::Unknown);
        let err = check_descriptor(&wrong, "machine", PluginKind::Machine, 1, 1).unwrap_err();
        assert_eq!(err, "descriptor is not a machine plugin");

        let mut null_vtable = descriptor(ABI_MAJOR_V1, ABI_MINOR_V1, PluginKind::Machine);
        null_vtable.vtable = core::ptr::null();
        let err = check_descriptor(&null_vtable, "machine", PluginKind::Machine, 1, 1).unwrap_err();
        assert_eq!(err, "machine plugin descriptor has null vtable");

        assert!(check_vtable_size("machine", 72, 72).is_ok());
        assert!(check_vtable_size("machine", 72, 96).is_ok());
        assert_eq!(
            check_vtable_size("machine", 72, 48).unwrap_err(),
            "machine plugin vtable too small: expected at least 72, got 48"
        );
    }
}
