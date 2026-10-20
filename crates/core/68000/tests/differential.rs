//! Differential test against an independent core, `m68k` 0.14.6 (MIT, https://github.com/benletchford/m68k-rs),
//! used only here as an oracle and never linked into a machine. Random legal instructions with random operands,
//! one step each from the same state; D0-D7, A0-A7, SR and PC must agree. Long, so `#[ignore]`:
//!
//!     cargo test --release -p mc68000 --test differential -- --ignored
//!
//! Exceptions are compared by SingleStepTests, not here: a case where either core ends in an exception handler,
//! or `m68k` runs on to an odd address without the address error the test data has, is not counted. Known
//! disagreements are listed in `differs` (with why we keep our behaviour).

use mc68000::disassemble::disassemble;
use mc68000::opcodes::find;
use mc68000::{BusFault, Cpu, FunctionCode, IackResponse, M68kBus, Size};

const START: u32 = 0x2000;
const MASK: u32 = 0xFF_FFFF;
const HANDLERS: u32 = 0x0080_0000;

#[derive(Clone, Default)]
struct Ram(std::collections::HashMap<u32, u8>);

impl Ram {
    fn b(&self, a: u32) -> u8 {
        self.0.get(&(a & MASK)).copied().unwrap_or(0)
    }
    fn set(&mut self, a: u32, v: u8) {
        self.0.insert(a & MASK, v);
    }
    fn put(&mut self, a: u32, bytes: &[u8]) {
        for (i, b) in bytes.iter().enumerate() {
            self.set(a + i as u32, *b);
        }
    }
}

impl M68kBus for Ram {
    fn read(&mut self, addr: u32, size: Size, _fc: FunctionCode) -> Result<u32, BusFault> {
        Ok(match size {
            Size::Byte => self.b(addr) as u32,
            Size::Word => u16::from_be_bytes([self.b(addr), self.b(addr + 1)]) as u32,
        })
    }
    fn write(
        &mut self,
        addr: u32,
        size: Size,
        value: u32,
        _fc: FunctionCode,
    ) -> Result<(), BusFault> {
        match size {
            Size::Byte => self.set(addr, value as u8),
            Size::Word => {
                self.set(addr, (value >> 8) as u8);
                self.set(addr + 1, value as u8);
            }
        }
        Ok(())
    }
    fn iack(&mut self, _level: u8) -> IackResponse {
        IackResponse::Autovector
    }
}

impl m68k::AddressBus for Ram {
    fn read_byte(&mut self, addr: u32) -> u8 {
        self.b(addr)
    }
    fn read_word(&mut self, addr: u32) -> u16 {
        u16::from_be_bytes([self.b(addr), self.b(addr + 1)])
    }
    fn read_long(&mut self, addr: u32) -> u32 {
        (self.read_word(addr) as u32) << 16 | self.read_word(addr + 2) as u32
    }
    fn write_byte(&mut self, addr: u32, value: u8) {
        self.set(addr, value);
    }
    fn write_word(&mut self, addr: u32, value: u16) {
        self.set(addr, (value >> 8) as u8);
        self.set(addr + 1, value as u8);
    }
    fn write_long(&mut self, addr: u32, value: u32) {
        self.write_word(addr, (value >> 16) as u16);
        self.write_word(addr + 2, value as u16);
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

/// Instructions whose outcome the two cores legitimately differ on, with the reason.
fn differs(text: &str) -> bool {
    // Line A/F and TRAP are reported to the host by `m68k` instead of taken as exceptions.
    text.starts_with("dc.") || text.starts_with("trap") || text.starts_with("illegal")
        // ASR by more than the operand size: see tests/singlestep.rs `known_disagreement`.
        || text.starts_with("asr")
        // CHK without a trap keeps N, and DIVU/DIVS overflow clears C, as SingleStepTests has it; `m68k`
        // clears N and keeps C.
        || text.starts_with("chk")
        || text.starts_with("div")
}

#[test]
#[ignore]
fn random_instruction_streams_match_m68k() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut mismatches = Vec::new();
    let mut checked = 0;
    while checked < 20_000 {
        let op = rng.next() as u16;
        if find(op).is_none() {
            continue;
        }
        let mut code = op.to_be_bytes().to_vec();
        for _ in 0..4 {
            code.extend_from_slice(&((rng.next() as u16) & 0xFFFE).to_be_bytes());
        }
        let (text, _) = disassemble(&code, START);
        if differs(&text) {
            continue;
        }
        let mut mem = Ram::default();
        for v in 0..64u32 {
            mem.put(v * 4, &(HANDLERS + v * 0x100).to_be_bytes());
        }
        mem.put(0, &0x0010_0000u32.to_be_bytes());
        mem.put(4, &START.to_be_bytes());
        mem.put(START, &code);
        let d: Vec<u32> = (0..8).map(|_| rng.next()).collect();
        // Even address registers inside RAM keep both cores away from address errors and wrapping.
        let a: Vec<u32> = (0..7)
            .map(|_| (rng.next() & 0x000F_FFFE) | 0x0010_0000)
            .collect();
        let sr = 0x2700 | (rng.next() as u16 & 0x1F);

        let mut ours_bus = mem.clone();
        let mut ours = Cpu::new();
        ours.reset(&mut ours_bus);
        ours.d.copy_from_slice(&d);
        ours.a.copy_from_slice(&a);
        ours.set_sr(sr);
        ours.step(&mut ours_bus);

        let mut theirs_bus = mem;
        let mut theirs = m68k::CpuCore::new();
        theirs.set_cpu_type(m68k::CpuType::M68000);
        theirs.reset(&mut theirs_bus);
        for (i, &v) in d.iter().enumerate() {
            theirs.set_d(i, v);
        }
        for (i, &v) in a.iter().enumerate() {
            theirs.set_a(i, v);
        }
        theirs.set_sr(sr);
        theirs.invalidate_prefetch();
        theirs.step(&mut theirs_bus);

        let exception = |pc: u32| (pc & MASK) >= HANDLERS && (pc & MASK) < HANDLERS + 0x4000;
        if exception(ours.pc) || exception(theirs.pc) || theirs.pc & 1 != 0 {
            continue;
        }
        let ours_regs: Vec<u32> = ours
            .d
            .iter()
            .chain(ours.a.iter())
            .copied()
            .chain([ours.a7()])
            .collect();
        let theirs_regs: Vec<u32> = (0..8)
            .map(|i| theirs.d(i))
            .chain((0..8).map(|i| theirs.a(i)))
            .collect();
        if ours_regs != theirs_regs
            || ours.sr() != theirs.get_sr()
            || ours.pc & MASK != theirs.pc & MASK
        {
            mismatches.push(format!(
                "{op:04X} {text}: sr {:04X}/{:04X} pc {:06X}/{:06X} regs {:08X?}/{:08X?}",
                ours.sr(),
                theirs.get_sr(),
                ours.pc,
                theirs.pc,
                ours_regs,
                theirs_regs
            ));
        }
        checked += 1;
    }
    assert!(
        mismatches.is_empty(),
        "{} of {checked} differ, first:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
