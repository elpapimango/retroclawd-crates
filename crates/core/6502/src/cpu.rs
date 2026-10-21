use crate::Bus;

// Real 6502 status-register bit layout (NV-BDIZC). Bit 5 (U) has no real
// flip-flop -- it always reads back as 1 on real hardware -- and bit 4 (B)
// only ever exists in the byte BRK/PHP push to the stack, never in the
// live register (a `PLP`/RTI restores the other 6 bits only).
pub(crate) const FLAG_C: u8 = 0x01;
pub(crate) const FLAG_Z: u8 = 0x02;
pub(crate) const FLAG_I: u8 = 0x04;
pub(crate) const FLAG_D: u8 = 0x08;
pub(crate) const FLAG_B: u8 = 0x10;
pub(crate) const FLAG_U: u8 = 0x20;
pub(crate) const FLAG_V: u8 = 0x40;
pub(crate) const FLAG_N: u8 = 0x80;

const STACK_BASE: u16 = 0x0100;
const RESET_VECTOR: u16 = 0xFFFC;
const NMI_VECTOR: u16 = 0xFFFA;
const IRQ_VECTOR: u16 = 0xFFFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub p: u8,

    /// Set by the real NMOS "JAM"/"KIL" illegal opcodes -- on real silicon
    /// these lock the address/data bus solid, requiring a hardware reset.
    /// Modeled here as a halt, same role `z80::Cpu::halted` plays for a
    /// Z80 `HALT`.
    pub halted: bool,

    /// Set by `request_irq`, consumed (and cleared) at the top of the next
    /// `step()` -- only actually taken if `p & FLAG_I == 0`, mirroring
    /// `z80::Cpu::irq_pending`'s "checked once per instruction boundary"
    /// shape for the same reason (a level-triggered real IRQ line).
    pub(crate) irq_pending: bool,

    /// Set by `request_nmi`, consumed at the top of the next `step()`,
    /// always taken regardless of `FLAG_I` (real NMI is non-maskable) --
    /// mirrors `z80::Cpu::nmi_pending`.
    pub(crate) nmi_pending: bool,

    // Keep the published register serialization intact. Machine state writers
    // persist this separately in their versioned envelope; see irq_poll_mask().
    #[serde(skip)]
    irq_poll_mask: Option<bool>,
}

impl Default for Cpu {
    fn default() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xFD,
            pc: 0,
            p: FLAG_U | FLAG_I,
            halted: false,
            irq_pending: false,
            nmi_pending: false,
            irq_poll_mask: None,
        }
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ordered register name/value pairs, matching `z80::Cpu::registers`'s
    /// shape so a future UI can render either core uniformly.
    pub fn registers(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("A", self.a as u64),
            ("X", self.x as u64),
            ("Y", self.y as u64),
            ("S", self.sp as u64),
            ("PC", self.pc as u64),
            ("P", self.p as u64),
        ]
    }
    pub fn set_register(&mut self, name: &str, value: u64) -> Result<(), String> {
        let reg = name.trim();
        if reg.is_empty() {
            return Err("empty register name".to_string());
        }
        let parse_u8 = |label: &str| -> Result<u8, String> {
            u8::try_from(value).map_err(|_| format!("value out of range for register {label}"))
        };
        let parse_u16 = |label: &str| -> Result<u16, String> {
            u16::try_from(value).map_err(|_| format!("value out of range for register {label}"))
        };

        match reg.to_ascii_uppercase().as_str() {
            "A" => self.a = parse_u8("A")?,
            "X" => self.x = parse_u8("X")?,
            "Y" => self.y = parse_u8("Y")?,
            "S" | "SP" => self.sp = parse_u8("S")?,
            "PC" => self.pc = parse_u16("PC")?,
            "P" | "SR" => {
                let p = parse_u8("P")?;
                self.p = (p | FLAG_U) & !FLAG_B;
                self.irq_poll_mask = None;
            }
            _ => return Err(format!("unknown register {name:?}")),
        }
        Ok(())
    }

    /// Real 6502 reset sequence: loads `PC` from the reset vector at
    /// `$FFFC`/`$FFFD`, sets the I flag, and drops `SP` by 3 -- real
    /// hardware runs the same microcode as an interrupt (which would push
    /// PC/P) with writes suppressed, so the stack pointer still moves as if
    /// 3 bytes had been pushed even though nothing actually was. Does not
    /// touch `A`/`X`/`Y` (undefined on real hardware, left as-is here).
    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        self.sp = self.sp.wrapping_sub(3);
        self.p |= FLAG_I;
        self.pc = Self::read_vector(bus, RESET_VECTOR);
        self.halted = false;
        self.irq_pending = false;
        self.nmi_pending = false;
        self.irq_poll_mask = None;
    }

    /// The I flag sampled before the last CLI, SEI or PLP, if applicable.
    /// The legacy `Cpu` serde layout excludes this latch. A machine must
    /// save it in its versioned state envelope and restore it along with
    /// the CPU for exact continuation. Old states restore `None`, since
    /// their instruction-boundary IRQ check used the live I flag.
    pub fn irq_poll_mask(&self) -> Option<bool> {
        self.irq_poll_mask
    }

    pub fn restore_irq_poll_mask(&mut self, mask: Option<bool>) {
        self.irq_poll_mask = mask;
    }

    /// Raises this CPU's maskable IRQ line. Only actually taken once
    /// `step()` next checks it and the sampled I flag is clear (CLI/SEI/PLP
    /// use their previous I flag), same "request now,
    /// take at the next instruction boundary" shape as
    /// `z80::Cpu::request_interrupt`.
    pub fn request_irq(&mut self) {
        self.irq_pending = true;
    }

    /// Raises this CPU's NMI line -- always taken at the next `step()`
    /// regardless of `FLAG_I`, same shape as `z80::Cpu::request_nmi`.
    pub fn request_nmi(&mut self) {
        self.nmi_pending = true;
    }

    /// Raises this CPU's dedicated SO (Set Overflow) input pin -- a real,
    /// distinct 6502 hardware pin (documented on every 6502 datasheet),
    /// separate from IRQ/NMI: unlike those, it has no "pending" concept
    /// on real silicon at all -- the V flag is simply set the instant the
    /// pin transitions, with no gating by any interrupt-enable flag and
    /// no vectoring. This project's own per-instruction-granularity
    /// `step()` model can't reproduce the exact real sub-instruction-cycle
    /// timing of when within an instruction the flag becomes visible, so
    /// this sets it immediately, visible to whichever instruction the CPU
    /// executes next -- the same "request now, effect visible at the next
    /// instruction boundary" tier `request_irq`/`request_nmi` already
    /// accept. Used by `c1541::C1541` to model the real 1541 disk
    /// mechanism's own byte-ready signal, wired on real hardware directly
    /// to this same pin (bypassing VIA/IRQ latency entirely, letting a
    /// tight `BVC`-polling loop read each disk byte as fast as it becomes
    /// available) -- confirmed via VICE's own `rotation.c` source, see
    /// `CREDITS.md`.
    pub fn set_overflow(&mut self) {
        self.p |= FLAG_V;
    }

    pub(crate) fn set_zn(&mut self, val: u8) {
        self.p &= !(FLAG_Z | FLAG_N);
        if val == 0 {
            self.p |= FLAG_Z;
        }
        self.p |= val & FLAG_N;
    }

    pub(crate) fn fetch<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let b = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        b
    }

    pub(crate) fn fetch_u16<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        lo | (hi << 8)
    }

    pub(crate) fn push<B: Bus>(&mut self, bus: &mut B, val: u8) {
        bus.write(STACK_BASE | self.sp as u16, val);
        self.sp = self.sp.wrapping_sub(1);
    }

    /// Address of the stack slot `SP` currently points at (`$01xx`).
    pub(crate) fn stack_addr(&self) -> u16 {
        STACK_BASE | self.sp as u16
    }

    pub(crate) fn pop<B: Bus>(&mut self, bus: &mut B) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(STACK_BASE | self.sp as u16)
    }

    pub(crate) fn push_u16<B: Bus>(&mut self, bus: &mut B, val: u16) {
        self.push(bus, (val >> 8) as u8);
        self.push(bus, val as u8);
    }

    pub(crate) fn pop_u16<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.pop(bus) as u16;
        let hi = self.pop(bus) as u16;
        lo | (hi << 8)
    }

    fn read_vector<B: Bus>(bus: &mut B, addr: u16) -> u16 {
        let lo = bus.read(addr) as u16;
        let hi = bus.read(addr.wrapping_add(1)) as u16;
        lo | (hi << 8)
    }

    pub(crate) fn irq_vector<B: Bus>(bus: &mut B) -> u16 {
        // https://www.nesdev.org/wiki/CPU_interrupts#Interrupt_hijacking:
        // select after cycle 4, before pushing P; BRK retains its B flag.
        if bus.take_nmi_edge() {
            NMI_VECTOR
        } else {
            IRQ_VECTOR
        }
    }

    /// Real 7-cycle NMI/IRQ sequence: pushes `PC` then `P` (with `FLAG_B`
    /// clear -- only BRK's own pushed byte ever has it set, the real
    /// distinction between a software and hardware break the 6502's IRQ
    /// handler can tell apart by checking the stacked `P`), sets `FLAG_I`,
    /// loads `PC` from `vector`.
    pub(crate) fn interrupt_sequence<B: Bus>(&mut self, bus: &mut B, vector: u16) -> u32 {
        // Cycles 1-2: the CPU re-reads the interrupted instruction's
        // opcode and next byte (PC is not advanced) and throws them away.
        bus.dummy_cycle(self.pc, false, 0);
        bus.dummy_cycle(self.pc, false, 0);
        self.push_u16(bus, self.pc);
        let vector = if vector == IRQ_VECTOR {
            Self::irq_vector(bus)
        } else {
            vector
        };
        self.push(bus, (self.p | FLAG_U) & !FLAG_B);
        self.p |= FLAG_I;
        self.pc = Self::read_vector(bus, vector);
        7
    }

    /// Executes one instruction (or services a pending NMI/IRQ first).
    /// Returns its cost in clock cycles (this crate's own term for what
    /// `machine::Machine::step`'s doc comment calls "whatever the
    /// underlying chip calls them" -- the 6502 datasheet itself says
    /// "cycles", not T-states).
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        let irq_masked = self.irq_poll_mask.take().unwrap_or(self.p & FLAG_I != 0);
        if self.nmi_pending {
            self.nmi_pending = false;
            self.halted = false;
            return self.interrupt_sequence(bus, NMI_VECTOR);
        }
        if self.irq_pending {
            self.irq_pending = false;
            if !irq_masked {
                self.halted = false;
                return self.interrupt_sequence(bus, IRQ_VECTOR);
            }
        }
        if self.halted {
            // A JAMmed CPU is stuck re-reading; two dummy cycles keep the
            // bus's cycle count equal to the cost reported below.
            bus.dummy_cycle(self.pc, false, 0);
            bus.dummy_cycle(self.pc, false, 0);
            return 2;
        }
        let opcode = self.fetch(bus);
        let old_i = self.p & FLAG_I != 0;
        let cycles = self.execute(opcode, bus);
        // Altirra Hardware Reference, chapter 3, "Clearing/Setting the I
        // flag with an interrupt pending": CLI/SEI/PLP change I after the
        // poll; RTI restores it early enough for the same instruction.
        // https://www.virtualdub.org/downloads/Altirra%20Hardware%20Reference%20Manual.pdf
        if matches!(opcode, 0x58 | 0x78 | 0x28) {
            self.irq_poll_mask = Some(old_i);
        }
        cycles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FlatBus;

    /// The M0 architecture proof, kept as a standing regression test: a
    /// hand-written 6502 program executed through a `machine::Bus` impl
    /// (`FlatBus`) that overrides only `read`/`write` -- no `read_io`/
    /// `write_io` at all.
    ///
    /// ```text
    /// 0200: LDX #$00
    /// 0202: loop: LDA #$01
    /// 0204:       STA $10
    /// 0206:       STA $0300
    /// 0209:       INX
    /// 020A:       CPX #$05
    /// 020C:       BNE loop      ; -12 -> back to 0202
    /// 020E: JMP $0210
    /// 0210: JAM
    /// ```
    #[test]
    fn smoke_test_program_runs_through_a_bus_with_no_io_ports() {
        let mut bus = FlatBus::new();
        let base: u16 = 0x0200;
        bus.load(
            base,
            &[
                0xA2, 0x00, // 0200 LDX #$00
                0xA9, 0x01, // 0202 loop: LDA #$01
                0x85, 0x10, // 0204       STA $10
                0x8D, 0x00, 0x03, // 0206       STA $0300
                0xE8, // 0209       INX
                0xE0, 0x05, // 020A       CPX #$05
                0xD0, 0xF4, // 020C       BNE loop (-12 -> 0202)
                0x4C, 0x10, 0x02, // 020E JMP $0210
                0x02, // 0210 JAM
            ],
        );

        let mut cpu = Cpu::new();
        cpu.pc = base;

        // Bounded loop guard against a real bug hanging the test.
        for _ in 0..1000 {
            cpu.step(&mut bus);
            if cpu.halted {
                break;
            }
        }

        assert!(cpu.halted, "program should reach JAM and halt");
        assert_eq!(cpu.x, 5, "INX/CPX/BNE loop should count X up to 5");
        assert_eq!(bus.read(0x0010), 1, "STA zero page should have landed");
        assert_eq!(bus.read(0x0300), 1, "STA absolute should have landed");
    }

    #[test]
    fn set_overflow_sets_the_v_flag_immediately_with_no_pending_concept() {
        let mut cpu = Cpu::new();
        assert_eq!(cpu.p & FLAG_V, 0);
        cpu.set_overflow();
        assert_eq!(
            cpu.p & FLAG_V,
            FLAG_V,
            "V flag should be set right away, not deferred"
        );
    }

    fn set_register_updates_named_6502_registers() {
        let mut cpu = Cpu::new();

        cpu.set_register("A", 0x42).unwrap();
        cpu.set_register("x", 0x10).unwrap();
        cpu.set_register("pc", 0x1234).unwrap();

        assert_eq!(cpu.a, 0x42);
        assert_eq!(cpu.x, 0x10);
        assert_eq!(cpu.pc, 0x1234);

        cpu.set_register("P", 0xFF).unwrap();
        assert_eq!(cpu.p & FLAG_U, FLAG_U);
        assert_eq!(cpu.p & FLAG_B, 0);
    }

    fn set_register_rejects_unknown_or_out_of_range_values() {
        let mut cpu = Cpu::new();

        let err = cpu.set_register("A", 0x100).unwrap_err();
        assert!(err.contains("out of range"));

        let err = cpu.set_register("QQ", 0).unwrap_err();
        assert!(err.contains("unknown register"));
    }
    /// Merged (every check kept): `set_register_updates_named_6502_registers`, `set_register_rejects_unknown_or_out_of_range_values`.
    #[test]
    fn set_register_cases() {
        set_register_updates_named_6502_registers();
        set_register_rejects_unknown_or_out_of_range_values();
    }
}
