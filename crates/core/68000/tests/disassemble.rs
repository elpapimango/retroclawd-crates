//! The disassembler's instruction lengths agree with the core: for every opcode word that is not a jump, a trap
//! or an exception, the length `disassemble` reports equals how far `Cpu::step` moves the PC.

use mc68000::disassemble::disassemble;
use mc68000::{BusFault, Cpu, FunctionCode, IackResponse, M68kBus, Size};

const START: u32 = 0x2000;

struct Ram(Vec<u8>);

impl M68kBus for Ram {
    fn read(&mut self, addr: u32, size: Size, _fc: FunctionCode) -> Result<u32, BusFault> {
        let a = addr as usize;
        Ok(match size {
            Size::Byte => self.0[a] as u32,
            Size::Word => u16::from_be_bytes([self.0[a], self.0[a + 1]]) as u32,
        })
    }
    fn write(
        &mut self,
        addr: u32,
        size: Size,
        value: u32,
        _fc: FunctionCode,
    ) -> Result<(), BusFault> {
        let a = addr as usize;
        match size {
            Size::Byte => self.0[a] = value as u8,
            Size::Word => self.0[a..a + 2].copy_from_slice(&(value as u16).to_be_bytes()),
        }
        Ok(())
    }
    fn iack(&mut self, _level: u8) -> IackResponse {
        IackResponse::Autovector
    }
}

fn is_control_flow(text: &str) -> bool {
    let branch = text.starts_with('b')
        && !["bchg", "bclr", "bset", "btst"]
            .iter()
            .any(|b| text.starts_with(b));
    branch
        || ["db", "jmp", "jsr", "rt", "trap", "stop", "illegal", "dc."]
            .iter()
            .any(|p| text.starts_with(p))
}

#[test]
fn lengths_match_how_far_the_core_moves_the_pc() {
    let mut checked = 0;
    for op in 0..=u16::MAX {
        // The opcode, then extension words of $0002 (even, non-zero: no address error, no zero divide).
        let mut code = op.to_be_bytes().to_vec();
        for _ in 0..4 {
            code.extend_from_slice(&0x0002u16.to_be_bytes());
        }
        let (text, len) = disassemble(&code, START);
        if is_control_flow(&text) {
            continue;
        }
        let mut mem = vec![0u8; 1 << 24];
        mem[0..4].copy_from_slice(&0x1000u32.to_be_bytes());
        mem[4..8].copy_from_slice(&START.to_be_bytes());
        for v in 2..64u32 {
            mem[(v * 4) as usize..(v * 4 + 4) as usize]
                .copy_from_slice(&0x0080_0000u32.to_be_bytes());
        }
        mem[START as usize..START as usize + code.len()].copy_from_slice(&code);
        let mut bus = Ram(mem);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        cpu.d = [2; 8];
        cpu.a = [0x4000; 7];
        cpu.step(&mut bus);
        if cpu.pc == 0x0080_0000 {
            continue; // took an exception (privilege, CHK, ...)
        }
        assert_eq!(cpu.pc - START, len as u32, "{op:04X} {text}");
        checked += 1;
    }
    assert!(checked > 40_000, "only {checked} opcodes checked");
}
