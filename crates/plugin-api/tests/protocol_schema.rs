//! Guard against changing the plugin wire format without saying so.
//!
//! Host and plugins exchange `bincode` messages, which carry no field names
//! or types: adding, removing, reordering or retyping anything in
//! `src/protocol.rs` silently breaks every plugin built before the change.
//! `PROTOCOL_VERSION` is what turns that silent break into a clear
//! "protocol version mismatch" error, so it must be bumped with any wire
//! change. This test fingerprints the source of `protocol.rs` (comments and
//! whitespace ignored) and fails when it no longer matches the fingerprint
//! recorded for the current `PROTOCOL_VERSION`.
//!
//! When it fails:
//! * the change can affect the wire format (fields, variants, their order,
//!   types, derives that change serialization) -> bump `PROTOCOL_VERSION`,
//!   then record the new version and fingerprint below;
//! * it cannot (comments, `impl` blocks, tests) -> record the new
//!   fingerprint below, keeping the version.
//!
//! The failure message prints both numbers.

use plugin_api::PROTOCOL_VERSION;

const RECORDED_VERSION: u16 = 11;
const RECORDED_FINGERPRINT: u64 = 0x2043_827b_cfb0_7253;

fn fingerprint(source: &str) -> u64 {
    // Strip `//` comments (doc comments included) and collapse whitespace,
    // then FNV-1a 64.
    let mut text = String::new();
    for line in source.lines() {
        let code = line.split("//").next().unwrap_or("");
        for word in code.split_whitespace() {
            text.push_str(word);
            text.push(' ');
        }
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn the_wire_schema_matches_the_recorded_protocol_version() {
    // Only the part before the unit tests: they may change freely.
    let source = include_str!("../src/protocol.rs");
    let schema = source.split("#[cfg(test)]").next().unwrap();
    let actual = fingerprint(schema);

    assert_eq!(
        PROTOCOL_VERSION, RECORDED_VERSION,
        "PROTOCOL_VERSION is {PROTOCOL_VERSION}; record it (and the fingerprint {actual:#x}) in \
         tests/protocol_schema.rs"
    );
    assert_eq!(
        actual, RECORDED_FINGERPRINT,
        "crates/plugin-api/src/protocol.rs changed (fingerprint {actual:#x}, recorded \
         {RECORDED_FINGERPRINT:#x}) while PROTOCOL_VERSION is {PROTOCOL_VERSION}. If the wire \
         format changed, bump PROTOCOL_VERSION; then set RECORDED_VERSION/RECORDED_FINGERPRINT in \
         tests/protocol_schema.rs. See docs/plugin-abi.md."
    );
}
