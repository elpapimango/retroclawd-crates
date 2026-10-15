# Plugin ABI, compatibility policy and release artifacts

`retroclawd` is a host. Every emulated machine is a **plugin library**
(`*_machine_plugin.dll` / `lib*_machine_plugin.so` / `lib*_machine_plugin.dylib`)
that the host loads at startup. The debuggers (browser UI, ZRCP, log) are not
plugins: they are ordinary modules of the application. A plugin built at one time must either work
with a host built at another, or be refused with a message that says why. This
document is the rule set; the code that enforces it is in
`crates/plugin-api/src/{abi,compat,codec,guard}.rs` and the tests named below.

## 1. Four versions, four meanings

| What | Where | Changes when | Host accepts a plugin if |
|---|---|---|---|
| **ABI major** | `ABI_MAJOR_V1`, `PluginDescriptorV1::abi_major` | the C-level contract is broken: descriptor layout, vtable field order/types/removal, calling convention, ownership rules | equal |
| **ABI minor** | `ABI_MINOR_V1`, `PluginDescriptorV1::abi_minor` | a vtable gets a field **appended** at its end | plugin minor ≥ host minor |
| **Protocol version** | `PROTOCOL_VERSION` in `protocol.rs`, first field of every message | *anything* in `protocol.rs` that changes the `bincode` bytes | equal |
| **Profile contract** | `MACHINE_PROFILE_CONTRACT_VERSION`, `MachineProfile::contract_version` | the meaning of profile fields/required capabilities changes (not their encoding) | equal |

Why the protocol is exact-match while the ABI minor is not: a C vtable can grow
at the end and an old reader simply ignores the tail, but `bincode` has no field
names or types, so any difference decodes to garbage (or an absurd allocation).
The plugin's own `version` string (its crate version) is informational only.

`retroclawd --list-plugins` prints the host's numbers on its first line and each
plugin's ABI (`(ABI 1.1)`); a rejected library is listed under the scan warnings
with the reason.

## 2. What is frozen

Never changes within an ABI major:

* the two entry symbols, `retroclawd_plugin_entry_v1` and `plugin_entry_v1`
  (`unsafe extern "C" fn() -> *const PluginDescriptorV1`, returning immutable
  static data); `plugin_api::export_plugin!` generates both;
* `PluginDescriptorV1`'s layout, and in particular `abi_major` at offset 0 and
  `abi_minor` at offset 2 — the host reads those two fields **before** it
  interprets anything else, so a descriptor of another major is refused without
  being read (`compat::descriptor_from_entry`);
* each vtable starts with `size: u32` and only ever grows at the end; a vtable
  must be at least as large as the host's definition (`compat::check_vtable_size`);
* the message envelope starts with the protocol version as a variable-length
  `u16` (so the host checks the version before decoding the payload — a
  skewed plugin gets "protocol version mismatch: expected 10, got 11", never a
  decode of the wrong schema);
* buffer ownership: memory a plugin returns (`RcBuffer`) is freed by calling
  **that plugin's** `free_buffer`, never by the host's allocator; request slices
  are borrowed for the duration of the call only.

`crates/plugin-api/src/abi.rs::tests::the_binary_layout_matches_the_published_abi`
pins every offset and size on 64-bit targets; it fails if you change a struct.

## 3. Rules for changing things

* **Append a vtable field** → bump `ABI_MINOR_V1`, update the layout test in the
  same commit, and make the host use the field only after checking nothing older
  than that minor can reach it (the minor check already guarantees it). Older
  plugins are then refused with "ABI minor too old"; rebuild them.
* **Anything else in the C ABI** (reorder, retype, remove, change the descriptor,
  change ownership) → new major (`ABI_MAJOR_V2`, new entry symbols and structs
  alongside the old; the host may support both for a deprecation period).
* **Edit `protocol.rs`** → if bytes can change (fields, variants — including
  adding one in the middle —, types, serde attributes), bump `PROTOCOL_VERSION`.
  `crates/plugin-api/tests/protocol_schema.rs` fingerprints the schema and fails
  on any edit; its failure message says what to record. Adding a variant at the
  *end* of an enum is still a bump: an older plugin cannot decode it.
* **Profile semantics** (new required debug capability, changed meaning of a
  field) → bump `MACHINE_PROFILE_CONTRACT_VERSION`.

A plugin and the host built from the same commit are always compatible; the
rules matter when they come from different commits or different repositories.
A plugin repository pins the `plugin-api` version it builds against and states
it in its release notes (`retroclawd --list-plugins` prints the same numbers).

## 4. Failure containment

* A Rust panic must not unwind through an `extern "C"` function (it aborts the
  process). Every exported function of every plugin wraps its body in
  `plugin_api::guard::guarded` / `guarded_call`: the panic is caught, the first
  one is remembered, the call returns `InternalError` with the text, and from
  then on the plugin's fast-path `step` idles (returns 4 cycles) and `halted`
  reads 1 so the host's loop cannot spin. A crashed plugin stays crashed until
  the program restarts.
* `bincode` decoding is limited to `codec::MAX_MESSAGE_BYTES` (256 MiB), so a
  corrupt or mismatched length prefix is an error, not an allocation.
* Libraries that fail any check are skipped with a warning naming the file and
  the reason; the host keeps running with the rest.

## 5. Tests that hold this together

* `plugin-api`: `compat::tests` (policy), `codec::tests` (version-first decode,
  length limit, varint prefix), `guard::tests`, `abi::tests` (layout),
  `tests/protocol_schema.rs` (schema fingerprint).
* `apps/retroclawd` `plugin_bridge::tests`: a newer plugin with a larger vtable
  loads; an ABI-minor-0 plugin is refused without its short vtable being read; a
  plugin speaking another protocol version is reported as such; contract
  version skew both ways; a library that is not a plugin is skipped; and
  `a_built_machine_library_loads_on_its_own_and_lists_its_machines`, which loads a
  real built library (run `cargo build --workspace` first; CI does).

## 6. Release artifacts

One archive per platform, built by `.github/scripts/package_release.sh` (run
manually for all three platforms with `.github/workflows/release-artifacts.yml`):

```
retroclawd-<version>-<rust target triple>.tar.gz   (.zip on Windows)
  retroclawd-<version>-<rust target triple>/
    retroclawd            retroclawd.exe on Windows
    plugins/              one library per machine plugin
    PLUGINS.txt           `retroclawd --list-plugins` of exactly this layout
    SHA256SUMS            every file in the archive
```

Library file names are what cargo produces for the `crates/plugin-dlls/<id>`
crates and what the host's scan requires (it looks for names containing both
`machine` and `plugin` with the platform extension):

| Platform | Executable | Machine plugin |
|---|---|---|
| Windows | `retroclawd.exe` | `zx81_machine_plugin.dll` |
| Linux | `retroclawd` | `libzx81_machine_plugin.so` |
| macOS | `retroclawd` | `libzx81_machine_plugin.dylib` |

Rules:

* Plugins load from `plugins/` next to the executable (also the executable's own
  folder and `./plugins`; `RETROCLAWD_MACHINE_PLUGIN_DIRS` overrides). Adding a
  machine is dropping its library in; removing one is deleting its file.
* A plugin library is only valid with a host whose ABI/protocol/contract numbers
  satisfy section 1; `PLUGINS.txt` and the first line of `--list-plugins` are the
  place to read them. Publish a third-party plugin per platform and state the
  `plugin-api` version it was built with.
* Machine plugin ids look like `retroclawd.<name>.machine`. The machine ids a
  plugin provides (`c64`, `cpc464`, ...) must be unique across all loaded
  plugins; a duplicate is an error naming the plugin that already provides it.
* Only the Linux packaging path has been run. The Windows (`zip`/PowerShell) and
  macOS branches and the three-platform workflow are written but unverified.

## Where the machines live (2026-10-12)

The shared crates in this repository (`plugin-api`, `machine`, `z80`, `mos6502`, ...) are consumed by the
machine family repositories (`retroclawd-sinclair`, `-amstrad`, `-commodore`) as git dependencies pinned to a
commit (`rev = "..."` in the manifests; a tag would do, but tags cannot be pushed from the sandbox this was done in). The pinned commit is therefore the ABI contract for that family: moving a family to a newer
commit changes which `ABI_MAJOR`/`ABI_MINOR`/`PROTOCOL_VERSION` its libraries export, and must be checked against
this document and against the host release the libraries will be shipped with. Release an ABI-affecting change
as a new commit/tag, then bump each family in turn.
