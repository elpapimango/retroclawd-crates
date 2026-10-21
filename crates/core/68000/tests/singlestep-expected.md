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
