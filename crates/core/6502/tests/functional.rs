use mos6502::{Bus, Cpu};

const KLAUS_IMAGE: &[u8; 0x10000] = include_bytes!("fixtures/6502_functional_test.bin");
const KLAUS_START: u16 = 0x0400;
const KLAUS_SUCCESS: u16 = 0x3469;
const KLAUS_MAX_INSTRUCTIONS: u64 = 100_000_000;

const SEDDON_IMAGE: &[u8] = include_bytes!("fixtures/seddon_6502_tests_generic.bin");
const SEDDON_START: u16 = 0x2000;
const SEDDON_TEST_BEGIN: u16 = 0x200b;
const SEDDON_FAIL: u16 = 0x2013;
const SEDDON_FINISH: u16 = 0x202b;
const SEDDON_MAX_INSTRUCTIONS: u64 = 5_000_000_000;

struct Ram {
    bytes: [u8; 0x10000],
}

impl Ram {
    fn new() -> Self {
        Self {
            bytes: [0; 0x10000],
        }
    }

    fn load(&mut self, addr: u16, image: &[u8]) {
        let start = addr as usize;
        self.bytes[start..start + image.len()].copy_from_slice(image);
    }
}

impl Bus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        self.bytes[addr as usize]
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.bytes[addr as usize] = val;
    }
}

#[test]
fn klaus_dormann_functional_test_reaches_success_trap() {
    let mut cpu = Cpu::new();
    let mut ram = Ram::new();
    ram.load(0, KLAUS_IMAGE);
    cpu.pc = KLAUS_START;

    for instruction in 1..=KLAUS_MAX_INSTRUCTIONS {
        let pc = cpu.pc;
        cpu.step(&mut ram);

        if cpu.pc == pc {
            assert_eq!(
                pc, KLAUS_SUCCESS,
                "functional test trapped after {instruction} instructions: \
                 PC={pc:#06x}, A={:#04x}, X={:#04x}, Y={:#04x}, SP={:#04x}, P={:#04x}, \
                 test_case={:#04x}",
                cpu.a, cpu.x, cpu.y, cpu.sp, cpu.p, ram.bytes[0x0200]
            );
            return;
        }
    }

    panic!(
        "functional test exceeded {KLAUS_MAX_INSTRUCTIONS} instructions: \
         PC={:#06x}, test_case={:#04x}",
        cpu.pc, ram.bytes[0x0200]
    );
}

#[test]
#[ignore = "exhaustive external suite; run with --release --ignored"]
fn seddon_nmos_and_illegal_opcode_tests_reach_finish_callback() {
    let mut cpu = Cpu::new();
    let mut ram = Ram::new();
    ram.load(SEDDON_START, SEDDON_IMAGE);

    // Replace the suite's callbacks with self-traps. The runner can then
    // distinguish a failed case from successful completion without an I/O
    // device. Each callback reserves eight bytes specifically for host patches.
    ram.load(SEDDON_FAIL, &[0x4c, 0x13, 0x20]);
    ram.load(SEDDON_FINISH, &[0x4c, 0x2b, 0x20]);
    cpu.pc = SEDDON_START;
    let mut current_test_addr = 0u16;

    for instruction in 1..=SEDDON_MAX_INSTRUCTIONS {
        let pc = cpu.pc;
        if pc == SEDDON_TEST_BEGIN {
            current_test_addr = u16::from(cpu.x) | (u16::from(cpu.y) << 8);
        }
        cpu.step(&mut ram);

        if cpu.pc != pc {
            continue;
        }
        if pc == SEDDON_FINISH {
            return;
        }
        if pc == SEDDON_FAIL {
            let state_addr = u16::from(cpu.x) | (u16::from(cpu.y) << 8);
            let state_start = state_addr as usize;
            let name_start = current_test_addr as usize;
            let name_len = ram.bytes[name_start..]
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(0);
            let name = String::from_utf8_lossy(&ram.bytes[name_start..name_start + name_len]);
            panic!(
                "illegal-opcode suite failed {name:?} after {instruction} instructions: \
                 state_addr={state_addr:#06x}, input/output/expected={:02x?}",
                &ram.bytes[state_start..state_start + 18]
            );
        }
        panic!(
            "illegal-opcode suite trapped unexpectedly after {instruction} instructions: \
             PC={pc:#06x}, A={:#04x}, X={:#04x}, Y={:#04x}, SP={:#04x}, P={:#04x}",
            cpu.a, cpu.x, cpu.y, cpu.sp, cpu.p
        );
    }

    panic!(
        "illegal-opcode suite exceeded {SEDDON_MAX_INSTRUCTIONS} instructions: PC={:#06x}",
        cpu.pc
    );
}
