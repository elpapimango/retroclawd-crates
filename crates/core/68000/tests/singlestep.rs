//! SingleStepTests/680x0 (https://github.com/SingleStepTests/680x0, commit e0d5ece, `68000/v1`): about 8 000
//! randomly generated single-instruction tests per file, each with the full CPU state, the RAM it touches, the
//! two-word prefetch queue, the total clock count and every bus transaction, before and after.
//!
//! The files are not committed (the repository states no licence): `scripts/fetch_singlesteptests.sh` downloads them
//! into `tests/sst/`; without them every test here prints a note and passes. `RC_SST_DIR` points elsewhere,
//! `RC_SST_BUS=1` also compares the bus transactions, `RC_SST_PRINT=1` prints every failure as a baseline line.
//! A failure listed in `sst_baseline.txt` (`<test name><TAB><reason>`) is allowed; any other one fails the test.

use flate2::read::GzDecoder;
use mc68000::{BusFault, Cpu, FunctionCode, IackResponse, M68kBus, Size};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Deserialize)]
struct State {
    d0: u32,
    d1: u32,
    d2: u32,
    d3: u32,
    d4: u32,
    d5: u32,
    d6: u32,
    d7: u32,
    a0: u32,
    a1: u32,
    a2: u32,
    a3: u32,
    a4: u32,
    a5: u32,
    a6: u32,
    usp: u32,
    ssp: u32,
    sr: u16,
    pc: u32,
    prefetch: [u16; 2],
    ram: Vec<(u32, u8)>,
}

impl State {
    fn d(&self) -> [u32; 8] {
        [
            self.d0, self.d1, self.d2, self.d3, self.d4, self.d5, self.d6, self.d7,
        ]
    }
    fn a(&self) -> [u32; 7] {
        [
            self.a0, self.a1, self.a2, self.a3, self.a4, self.a5, self.a6,
        ]
    }
}

#[derive(Deserialize)]
struct Case {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    expected: State,
    length: u32,
    transactions: Vec<serde_json::Value>,
}

/// One recorded bus event, in the test files' notation.
#[derive(Debug, PartialEq)]
enum Event {
    Idle(u32),
    Access {
        write: bool,
        fc: u8,
        addr: u32,
        word: bool,
        value: u32,
    },
}

#[derive(Default)]
struct TestBus {
    ram: HashMap<u32, u8>,
    events: Vec<Event>,
}

impl TestBus {
    fn byte(&self, addr: u32) -> u8 {
        self.ram.get(&(addr & 0xFF_FFFF)).copied().unwrap_or(0)
    }
}

impl M68kBus for TestBus {
    fn read(&mut self, addr: u32, size: Size, fc: FunctionCode) -> Result<u32, BusFault> {
        let value = match size {
            Size::Byte => self.byte(addr) as u32,
            Size::Word => u16::from_be_bytes([self.byte(addr), self.byte(addr + 1)]) as u32,
        };
        self.events.push(Event::Access {
            write: false,
            fc: fc.0,
            addr,
            word: size == Size::Word,
            value,
        });
        Ok(value)
    }

    fn write(
        &mut self,
        addr: u32,
        size: Size,
        value: u32,
        fc: FunctionCode,
    ) -> Result<(), BusFault> {
        match size {
            Size::Byte => {
                self.ram.insert(addr, value as u8);
            }
            Size::Word => {
                let [hi, lo] = (value as u16).to_be_bytes();
                self.ram.insert(addr, hi);
                self.ram.insert((addr + 1) & 0xFF_FFFF, lo);
            }
        }
        let value = if size == Size::Byte {
            value & 0xFF
        } else {
            value & 0xFFFF
        };
        self.events.push(Event::Access {
            write: true,
            fc: fc.0,
            addr,
            word: size == Size::Word,
            value,
        });
        Ok(())
    }

    fn iack(&mut self, _level: u8) -> IackResponse {
        IackResponse::Autovector
    }

    fn sync(&mut self, clocks: u32) {
        match self.events.last_mut() {
            Some(Event::Idle(n)) => *n += clocks,
            _ => self.events.push(Event::Idle(clocks)),
        }
    }
}

/// The test file's transactions as events: `["n", clocks]` and `[kind, len, fc, addr, ".b"/".w", value]`, a TAS
/// (`"t"`) being its byte read followed by its byte write.
fn expected_events(case: &Case) -> Vec<Event> {
    let mut out = Vec::new();
    for t in &case.transactions {
        let kind = t[0].as_str().unwrap_or("");
        if kind == "n" {
            match out.last_mut() {
                Some(Event::Idle(n)) => *n += t[1].as_u64().unwrap_or(0) as u32,
                _ => out.push(Event::Idle(t[1].as_u64().unwrap_or(0) as u32)),
            }
            continue;
        }
        let fc = t[2].as_u64().unwrap_or(0) as u8;
        let addr = t[3].as_u64().unwrap_or(0) as u32;
        let word = t[4].as_str() == Some(".w");
        let value = t[5].as_u64().unwrap_or(0) as u32;
        if kind == "t" {
            let before = case
                .initial
                .ram
                .iter()
                .find(|(a, _)| *a == addr)
                .map_or(0, |(_, v)| *v as u32);
            out.push(Event::Access {
                write: false,
                fc,
                addr,
                word: false,
                value: before,
            });
            out.push(Event::Access {
                write: true,
                fc,
                addr,
                word: false,
                value,
            });
        } else {
            out.push(Event::Access {
                write: kind == "w",
                fc,
                addr,
                word,
                value,
            });
        }
    }
    out
}

/// Runs one case; `Err` names the first mismatch.
fn run(case: &Case, compare_bus: bool) -> Result<(), String> {
    let init = &case.initial;
    let mut bus = TestBus::default();
    for &(addr, value) in &init.ram {
        bus.ram.insert(addr, value);
    }
    let mut cpu = Cpu::new();
    cpu.d = init.d();
    cpu.a = init.a();
    cpu.usp = init.usp;
    cpu.ssp = init.ssp;
    cpu.set_sr(init.sr);
    cpu.pc = init.pc;
    cpu.set_prefetch(init.prefetch);
    let clocks = cpu.step(&mut bus);

    let fin = &case.expected;
    let mut diffs = Vec::new();
    let regs = |label: &str, got: &[u32], want: &[u32], diffs: &mut Vec<String>| {
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            if g != w {
                diffs.push(format!("{label}{i} {g:08X} want {w:08X}"));
            }
        }
    };
    regs("d", &cpu.d, &fin.d(), &mut diffs);
    regs("a", &cpu.a, &fin.a(), &mut diffs);
    regs("usp", &[cpu.usp], &[fin.usp], &mut diffs);
    regs("ssp", &[cpu.ssp], &[fin.ssp], &mut diffs);
    if cpu.sr() != fin.sr {
        diffs.push(format!("sr {:04X} want {:04X}", cpu.sr(), fin.sr));
    }
    if cpu.pc != fin.pc {
        diffs.push(format!("pc {:06X} want {:06X}", cpu.pc, fin.pc));
    }
    if cpu.prefetch() != fin.prefetch {
        diffs.push(format!(
            "prefetch {:04X?} want {:04X?}",
            cpu.prefetch(),
            fin.prefetch
        ));
    }
    for &(addr, want) in &fin.ram {
        let got = bus.byte(addr);
        if got != want {
            diffs.push(format!("ram[{addr:06X}] {got:02X} want {want:02X}"));
        }
    }
    if clocks != case.length {
        diffs.push(format!("clocks {clocks} want {}", case.length));
    }
    if compare_bus && diffs.is_empty() {
        let want = expected_events(case);
        if bus.events != want {
            diffs.push(format!("bus {:?}\n      want {:?}", bus.events, want));
        }
    }
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(diffs.join(", "))
    }
}

/// Known disagreements between the test data and the Programmer's Reference Manual, kept to the manual's
/// behaviour. Each returns true when `diff` is exactly that disagreement for `case`.
///
/// ASR by a register count larger than the operand size on a negative operand: the data has C = X = 0, PRM 4
/// (ASL, ASR: "C - Set according to the last bit shifted out of the operand") gives the sign bit, 1. Only the
/// count > size cases disagree (count == size has C = X = 1 in the data too). Reported to the owner 2026-10-19.
fn known_disagreement(case: &Case, diff: &str) -> bool {
    let op = u16::from_str_radix(&case.name[..4], 16).unwrap_or(0);
    let asr_register_count =
        op & 0xF018 == 0xE000 && op & 0x01C0 != 0x00C0 && op & 0x0120 == 0x0020;
    if !asr_register_count {
        return false;
    }
    let bits = 8u32 << ((op >> 6) & 3);
    let count = case.initial.d()[((op >> 9) & 7) as usize] & 63;
    let value = case.initial.d()[(op & 7) as usize];
    let negative = value >> (bits - 1) & 1 != 0;
    let ours = case.expected.sr | 0x11;
    count > bits && negative && diff == format!("sr {ours:04X} want {:04X}", case.expected.sr)
}

fn dir() -> PathBuf {
    std::env::var_os("RC_SST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sst"))
}

fn baseline() -> HashSet<String> {
    let text = include_str!("sst_baseline.txt");
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').next().unwrap_or("").to_string())
        .collect()
}

fn run_file(file: &str) {
    let path = dir().join(format!("{file}.json.gz"));
    let Ok(f) = std::fs::File::open(&path) else {
        eprintln!(
            "SingleStepTests not fetched ({}): run scripts/fetch_singlesteptests.sh",
            path.display()
        );
        return;
    };
    let cases: Vec<Case> = serde_json::from_reader(GzDecoder::new(f)).expect("parse test file");
    let compare_bus = std::env::var_os("RC_SST_BUS").is_some();
    let print = std::env::var_os("RC_SST_PRINT").is_some();
    let allowed = baseline();
    let started = std::time::Instant::now();
    let mut unexpected = Vec::new();
    let mut failed = 0;
    for case in &cases {
        if let Err(diff) = run(case, compare_bus) {
            failed += 1;
            if print {
                println!("{}\t{}", case.name, diff);
            }
            if !allowed.contains(&case.name) && !known_disagreement(case, &diff) {
                unexpected.push(format!("{}: {}", case.name, diff));
            }
        }
    }
    let per_sec = cases.len() as f64 / started.elapsed().as_secs_f64().max(1e-9);
    eprintln!(
        "{file}: {} of {} pass ({per_sec:.0} cases/s)",
        cases.len() - failed,
        cases.len()
    );
    assert!(
        unexpected.is_empty(),
        "{file}: {} unexpected failures, first:\n  {}",
        unexpected.len(),
        unexpected
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

macro_rules! sst {
    ($($name:ident => $file:literal),* $(,)?) => {
        $(#[test] fn $name() { run_file($file); })*
    };
}

sst! {
    nop => "NOP",
    move_b => "MOVE.b",
    move_w => "MOVE.w",
    move_l => "MOVE.l",
    movea_w => "MOVEA.w",
    movea_l => "MOVEA.l",
    move_q => "MOVE.q",
    lea => "LEA",
    pea => "PEA",
    clr_b => "CLR.b",
    clr_w => "CLR.w",
    clr_l => "CLR.l",
    tst_b => "TST.b",
    tst_w => "TST.w",
    tst_l => "TST.l",
    exg => "EXG",
    swap => "SWAP",
    ext_w => "EXT.w",
    ext_l => "EXT.l",
    add_b => "ADD.b",
    add_w => "ADD.w",
    add_l => "ADD.l",
    sub_b => "SUB.b",
    sub_w => "SUB.w",
    sub_l => "SUB.l",
    cmp_b => "CMP.b",
    cmp_w => "CMP.w",
    cmp_l => "CMP.l",
    neg_b => "NEG.b",
    neg_w => "NEG.w",
    neg_l => "NEG.l",
    negx_b => "NEGX.b",
    negx_w => "NEGX.w",
    negx_l => "NEGX.l",
    addx_b => "ADDX.b",
    addx_w => "ADDX.w",
    addx_l => "ADDX.l",
    subx_b => "SUBX.b",
    subx_w => "SUBX.w",
    subx_l => "SUBX.l",
    adda_w => "ADDA.w",
    adda_l => "ADDA.l",
    suba_w => "SUBA.w",
    suba_l => "SUBA.l",
    cmpa_w => "CMPA.w",
    cmpa_l => "CMPA.l",
    and_b => "AND.b",
    and_w => "AND.w",
    and_l => "AND.l",
    or_b => "OR.b",
    or_w => "OR.w",
    or_l => "OR.l",
    eor_b => "EOR.b",
    eor_w => "EOR.w",
    eor_l => "EOR.l",
    not_b => "NOT.b",
    not_w => "NOT.w",
    not_l => "NOT.l",
    asl_b => "ASL.b",
    asl_w => "ASL.w",
    asl_l => "ASL.l",
    asr_b => "ASR.b",
    asr_w => "ASR.w",
    asr_l => "ASR.l",
    lsl_b => "LSL.b",
    lsl_w => "LSL.w",
    lsl_l => "LSL.l",
    lsr_b => "LSR.b",
    lsr_w => "LSR.w",
    lsr_l => "LSR.l",
    rol_b => "ROL.b",
    rol_w => "ROL.w",
    rol_l => "ROL.l",
    ror_b => "ROR.b",
    ror_w => "ROR.w",
    ror_l => "ROR.l",
    roxl_b => "ROXL.b",
    roxl_w => "ROXL.w",
    roxl_l => "ROXL.l",
    roxr_b => "ROXR.b",
    roxr_w => "ROXR.w",
    roxr_l => "ROXR.l",
    bcc => "Bcc",
    bsr => "BSR",
    dbcc => "DBcc",
    scc => "Scc",
    jmp => "JMP",
    jsr => "JSR",
    rts => "RTS",
    rtr => "RTR",
    rte => "RTE",
    link => "LINK",
    unlink => "UNLINK",
    anditoccr => "ANDItoCCR",
    anditosr => "ANDItoSR",
    oritoccr => "ORItoCCR",
    oritosr => "ORItoSR",
    eoritoccr => "EORItoCCR",
    eoritosr => "EORItoSR",
    movetoccr => "MOVEtoCCR",
    movetosr => "MOVEtoSR",
    movefromsr => "MOVEfromSR",
    movetousp => "MOVEtoUSP",
    movefromusp => "MOVEfromUSP",
    movem_w => "MOVEM.w",
    movem_l => "MOVEM.l",
    movep_w => "MOVEP.w",
    movep_l => "MOVEP.l",
    bchg => "BCHG",
    bclr => "BCLR",
    bset => "BSET",
    btst => "BTST",
    tas => "TAS",
    reset => "RESET",
    mulu => "MULU",
    muls => "MULS",
    divu => "DIVU",
    divs => "DIVS",
    abcd => "ABCD",
    sbcd => "SBCD",
    nbcd => "NBCD",
    chk => "CHK",
    trap => "TRAP",
    trapv => "TRAPV",
}
