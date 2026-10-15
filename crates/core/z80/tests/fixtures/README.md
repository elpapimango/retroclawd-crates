# Test fixtures

`zexall.com` / `zexdoc.com` — the Z80 instruction set exerciser, originally
written by Frank D. Cringle in 1994 (GPLv2+), fetched from the prebuilt
binaries in [superzazu/z80](https://github.com/superzazu/z80)'s `roms/`
directory (`zexall.cim` / `zexdoc.cim`, renamed here to their conventional
`.com` extension; contents unchanged). `zexdoc` checks documented flag
behavior only; `zexall` additionally checks the undocumented flags/opcodes.

Run via `cargo run --release --example zexall -- zexdoc` (or `zexall`).
