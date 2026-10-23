# SingleStepTests 68000/v1: the expected failures

`tests/singlestep.rs` runs SingleStepTests/680x0 (commit `e0d5ece`, `68000/v1`; fetched by
`scripts/fetch_singlesteptests.sh`, never committed). Of 1 000 060 cases, 3 738 fail on purpose. They are listed in
`EXPECTED` in that file, per test file and reason; a test fails if any other case fails, or if a listed count changes
(a listed case that starts passing included). This file gives each reason with its source.

| File | Reason | Cases |
|---|---|---|
| `ASR.b` | ASR carry | 1 642 |
| `ASR.w` | ASR carry | 1 063 |
| `ASR.l` | ASR carry | 1 031 |
| `ASL.b` | bad upstream data | 2 |

## ASR carry (3 736 cases)

ASR by a data-register count larger than the operand size (9-63 for a byte, 17-63 for a word, 33-63 for a long),
on a negative operand. The test data has C = X = 0 after the shift. The *M68000 Family Programmer's Reference Manual*
(M68000PM/AD), section 4, the ASL/ASR entry, its Condition Codes table, gives "C — Set according to the last bit
shifted out of the operand", and X the same as C for a non-zero count. An arithmetic right shift copies the sign bit in, so every bit
shifted out past the operand's width is the sign bit, 1: C = X = 1. The core follows the manual. Only the
count > size cases disagree; at count == size the data has C = X = 1 too, which supports the manual's reading.
`known_disagreement` in `singlestep.rs` accepts exactly this difference (the status register with C and X set
against the data's value) and nothing else. Reported to the owner 2026-10-19; no real-chip measurement is available
here to settle it.

## Bad upstream data (2 cases)

`e502 [ASL.b Q, D2] 1583` and `e502 [ASL.b Q, D2] 1761`: a byte-sized shift whose expected D2 has changed upper
bytes, which a byte-sized shift does not touch (the issue below shows the data). Upstream issue: <https://github.com/SingleStepTests/680x0/issues/4>. Listed by name in
`BAD_UPSTREAM`.

## Bus transactions (Phase 3, 2026-10-22)

The suite compares the bus transactions too: the accesses with their function codes, sizes and order, and the internal
clocks between them (`Idle`). Before Phase 3 that comparison passed 557 843 of 983 930 cases (`RC_SST_BUS=1`); now it
passes every case the functional comparison does, so the list above is also the list of bus disagreements, and the three
instruction groups below were the ones the data taught:

- **Register operations read the next word before their internal clocks** (shifts and rotates, `EXG`, `MULU/MULS`, long
  `ADD/SUB/AND/OR/CMP`, `ADDA/SUBA/CMPA`, `ABCD/SBCD/NBCD`, `Scc`, `BTST` and friends, `CHK` without a trap, `MOVE SR`):
  the fetch, then the idle clocks. `DIVU/DIVS` keep their clocks before it.
- **The indexed modes add the index while the queue refills:** their 2 internal clocks come before the refill read.
- **Long read-modify-write results go out low word first** (`ADD.L Dn,<ea>`, `NEG.L`, `CLR.L`, ...), after the fetch of the
  next word; `ADDX/SUBX.L` on `-(An)` read both operands low word first and refill the queue between the two writes;
  `RTE/RTR` read the PC's high word, then the status word, then the low word; `JSR` fetches the first target word before
  it stacks the return address; `PEA` stacks before the final fetch for the two absolute modes and after it for the others.
- **TAS** is one 10-clock cycle in the data (read, 2 internal clocks, write); the harness expands it to the read, an
  `Idle(2)` and the write.
