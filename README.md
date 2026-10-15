# retroclawd-crates

The shared crates of [retroclawd](https://github.com/elpapimango/retroclawd): everything more than one
repository needs. They carry no machine and no application code.

- `crates/machine`
- `crates/plugin-api`
- `crates/core/z80`
- `crates/core/6502`
- `crates/chips/ay-3-8910`
- `crates/chips/nec765`
- `crates/formats/dsk`
- `crates/formats/zx-tape`

- `machine` -- the `Machine`/`Bus` traits, the pacing clock, `MemoryBus`, and `SymbolTable` (address names for
  disassembly: symbol-file parser and a CPU-agnostic text annotator).
- `plugin-api` -- the C ABI between the application and the machine libraries
  (policy and compatibility rules: `docs/plugin-abi.md`; read it before touching the crate).
- `z80`, `mos6502` -- the CPU cores (Z80 validated with ZEXALL/ZEXDOC, 6502 with Klaus Dormann's and
  Tom Seddon's suites). Each has a `disassemble` module that also assembles one instruction
  (`disassemble`/`assemble_one`; the `_at` variants show and take branch targets as absolute addresses).
- `ay-3-8910`, `nec765`, `dsk`, `zx-tape` -- the chip and file formats shared by the Sinclair and Amstrad
  machines.

## Who uses it

The application (`retroclawd`) checks this repository out as a git submodule (`shared/`) and uses the crates by
path. The machine repositories (`retroclawd-sinclair`, `-amstrad`, `-commodore`) depend on them **by git
revision** (`git = "...retroclawd-crates", rev = "..."`). To move a repository to a newer revision, use
`tools/bump_pin.sh` in the application repository.

## Build and test

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Z80 instruction accuracy: `cargo run --release -p z80 --example zexall -- zexall` (and `zexdoc`).
A change to `plugin-api` that alters the wire schema fails `tests/protocol_schema.rs` until
`PROTOCOL_VERSION` is bumped and the new fingerprint recorded.

## License

Free software under the **GNU General Public License v3.0 or later** (see `LICENSE`). The vendored test programs under
`crates/core/6502/tests/fixtures/` (Klaus Dormann, Tom Seddon) and `crates/core/z80/tests/fixtures/` (ZEXALL/ZEXDOC,
Frank D. Cringle, GPL-2.0-or-later) keep their own licences, stated in the README beside them.

## Contributing / issues

Not accepted right now -- this is a personal work-in-progress project. Pull requests and issues will be closed
unreviewed.
