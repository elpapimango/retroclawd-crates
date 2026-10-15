# 6502 validation fixtures

These third-party fixtures validate `mos6502` independently of its hand-written unit tests.
They are GPL-3.0-or-later test programs, compatible with the GPL-3.0-or-later licence of the rest of this
repository.

## Klaus Dormann functional test

Source: <https://github.com/Klaus2m5/6502_65C02_functional_tests>

Pinned revision: `fe99e5616243a1bdbceaf5907390ce4443de7db0` (2017-12-07). This is the
revision that last updated the upstream prebuilt binary, so the vendored `.a65` source and `.bin`
match. Upstream changed the source again in 2020 without rebuilding the checked-in binary; this
fixture deliberately does not mix those revisions.

License: GPL-3.0-or-later; see `LICENSE-GPL-3.0.txt` and the source header.

Configuration and runner contract:

- Full 64K image loaded at `$0000`.
- Entry point `$0400`.
- Success self-trap `$3469`.
- Any other one-instruction self-trap is a failure.
- Tests documented NMOS 6502 instructions and addressing modes. It does not test illegal opcodes.

SHA-256:

```text
fa12bfc761e6f9057e4cc01a665a7b800ff01ae91f598af1e39a1201d01953fd  6502_functional_test.bin
81e1028c24650cb218e129deb19a71d5fab5ecccf1ef55705951d3a2b9595fca  6502_functional_test.a65
8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903  LICENSE-GPL-3.0.txt
```

Run with:

```sh
cargo test -p mos6502 --test functional klaus_dormann
```

## Tom Seddon NMOS and illegal-opcode tests

Source: <https://github.com/tom-seddon/6502-tests>

Pinned revision: `6ecccbfebdb9e10588b1019166bcb5b3b6e617bc`, release
`6502-tests-generic.20221022T231937.dat`. The suite was designed to cover the NMOS and illegal
opcode gaps left by the Klaus test, drawing on Wolfgang Lorenz's tests and checking its model on
real BBC Micro hardware.

License: GPL-3.0-or-later; see `LICENSE-Seddon-GPL-3.0.txt` and the source headers. The generic
binary's corresponding `6502-tests.s65` and `lib.s65` sources are vendored beside it; upstream builds
them with 64tass.

Runner contract:

- Binary loaded at `$2000`, its documented entry point.
- The eight-byte failure callback at `$2013` is patched in emulated RAM to a self-trap.
- The eight-byte finish callback at `$202b` is patched in emulated RAM to a self-trap.
- The source fixture and binary on disk remain unmodified.
- Covers NMOS decimal arithmetic, stable illegal opcodes, and invalid BCD inputs exhaustively.
- Ignored in ordinary debug test runs because it executes an exhaustive input matrix; run it in
  release mode explicitly.

SHA-256:

```text
f71222211665e67b18d3ea7fa8091c7a23abc46ddfbf4554e6e45b05b565c8d9  seddon_6502_tests_generic.bin
9e1edadd5007b3eac5b0e7f1cc86ef6debef8016ea97722fca5c92014b24e481  seddon_6502_tests.s65
6a680e97b4444b11dbf3e81503a976aae24e19e6eb968eec755b55cd65efa69b  seddon_lib.s65
3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986  LICENSE-Seddon-GPL-3.0.txt
```

Run with:

```sh
cargo test -p mos6502 --test functional --release -- --ignored --nocapture
```
