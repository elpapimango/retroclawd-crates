use std::env;
use std::fs;
use std::io::Write;
use std::process::ExitCode;

use z80::{Bus, Cpu};

struct FlatBus {
    mem: [u8; 0x10000],
}

impl Bus for FlatBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.mem[addr as usize] = val;
    }

    fn read_io(&mut self, _port: u16) -> u8 {
        0
    }

    fn write_io(&mut self, _port: u16, _val: u8) {}
}

fn main() -> ExitCode {
    let which = env::args().nth(1).unwrap_or_else(|| "zexdoc".to_string());
    let path = format!("{}/tests/fixtures/{which}.com", env!("CARGO_MANIFEST_DIR"));
    let program = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("failed to read {path}: {e}");
            eprintln!("usage: cargo run --release --example zexall -- [zexdoc|zexall]");
            return ExitCode::FAILURE;
        }
    };

    let mut bus = FlatBus { mem: [0; 0x10000] };
    bus.mem[0x100..0x100 + program.len()].copy_from_slice(&program);

    let mut cpu = Cpu::new();
    cpu.pc = 0x100;
    cpu.sp = 0xF000;

    // CP/M convention: 0x0000 is the warm-boot vector (program exit) and
    // 0x0005 is the BDOS entry point. We never execute real code at either
    // address -- both are trapped by PC value before dispatch, below.
    let mut steps: u64 = 0;
    loop {
        if cpu.pc == 0x0000 {
            break;
        }
        if cpu.pc == 0x0005 {
            bdos_call(&mut cpu, &mut bus);
            continue;
        }

        cpu.step(&mut bus);

        steps += 1;
        if steps.is_multiple_of(200_000_000) {
            eprintln!(
                "[zexall] {steps} instructions executed, still running (pc=0x{:04X})...",
                cpu.pc
            );
        }
    }

    println!();
    ExitCode::SUCCESS
}

fn bdos_call(cpu: &mut Cpu, bus: &mut FlatBus) {
    let function = (cpu.bc & 0xFF) as u8; // C register
    match function {
        2 => {
            // C_WRITE: character in E
            print_byte((cpu.de & 0xFF) as u8);
        }
        9 => {
            // C_WRITESTR: DE points to a '$'-terminated string
            let mut addr = cpu.de;
            loop {
                let byte = bus.read(addr);
                if byte == b'$' {
                    break;
                }
                print_byte(byte);
                addr = addr.wrapping_add(1);
            }
        }
        other => {
            eprintln!("\n[zexall] unhandled BDOS call: C={other}");
        }
    }

    // Simulate the RET this BDOS stub would have executed.
    let lo = bus.read(cpu.sp);
    cpu.sp = cpu.sp.wrapping_add(1);
    let hi = bus.read(cpu.sp);
    cpu.sp = cpu.sp.wrapping_add(1);
    cpu.pc = ((hi as u16) << 8) | lo as u16;
}

fn print_byte(byte: u8) {
    print!("{}", byte as char);
    let _ = std::io::stdout().flush();
}
