//! MOS 6520 PIA, without machine-specific wiring or a CPU dependency.
//!
//! Register, pin and reset behavior: Avery Lee, Altirra Hardware Reference
//! Manual (2026-01-02), sections 2.5 and 14.5, pages 34-36 and 453-456:
//! <https://www.virtualdub.org/downloads/Altirra%20Hardware%20Reference%20Manual.pdf>.
//! `read`/`write` use A, CRA, B, CRB ordering; `*_alt` swap the address bits.
//! A pulse lasts one caller-supplied clock. Analogue floating-input decay and
//! output-to-input spurious C2 interrupts are not modeled.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Port {
    output: u8,
    direction: u8,
    control: u8,
    input: u8,
    c1: bool,
    c2: bool,
    c2_output: bool,
    pulse: bool,
    irq1: bool,
    irq2: bool,
}

impl Default for Port {
    fn default() -> Self {
        Self {
            output: 0,
            direction: 0,
            control: 0,
            input: 0xff,
            c1: true,
            c2: true,
            c2_output: true,
            pulse: false,
            irq1: false,
            irq2: false,
        }
    }
}

/// Pin names, not CPU addresses. Machine wiring supplies external levels.
#[derive(Clone, Copy, Debug)]
pub enum Side {
    A,
    B,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Pia {
    ports: [Port; 2],
}

impl Pia {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset internal registers; external pin levels are board-owned.
    pub fn reset(&mut self) {
        for p in &mut self.ports {
            *p = Port {
                input: p.input,
                c1: p.c1,
                c2: p.c2,
                ..Port::default()
            };
        }
    }

    pub fn set_input(&mut self, side: Side, value: u8) {
        self.ports[side.index()].input = value;
    }

    /// Electrical output with input pins pulled high, independent of readback.
    pub fn drive(&self, side: Side) -> u8 {
        let p = &self.ports[side.index()];
        p.output | !p.direction
    }

    pub fn pins(&self, side: Side) -> u8 {
        let p = &self.ports[side.index()];
        match side {
            // Port A senses pin levels even on bits configured as outputs.
            Side::A => p.input & self.drive(side),
            Side::B => (p.input & !p.direction) | (p.output & p.direction),
        }
    }

    pub fn c2_output(&self, side: Side) -> Option<bool> {
        let p = &self.ports[side.index()];
        (p.control & 0x20 != 0).then_some(p.c2_output)
    }

    pub fn set_c1(&mut self, side: Side, level: bool) {
        let p = &mut self.ports[side.index()];
        if p.c1 != level && level == (p.control & 2 != 0) {
            p.irq1 = true;
            if (p.control >> 3) == 4 {
                p.c2_output = true;
            }
        }
        p.c1 = level;
    }

    pub fn set_c2(&mut self, side: Side, level: bool) {
        let p = &mut self.ports[side.index()];
        if p.control & 0x20 == 0 && p.c2 != level && level == (p.control & 0x10 != 0) {
            p.irq2 = true;
        }
        p.c2 = level;
    }

    pub fn irq(&self, side: Side) -> bool {
        let p = &self.ports[side.index()];
        (p.irq1 && p.control & 1 != 0) || (p.irq2 && p.control & 0x28 == 8)
    }

    pub fn tick(&mut self, cycles: u32) {
        if cycles == 0 {
            return;
        }
        for p in &mut self.ports {
            if p.pulse {
                p.pulse = false;
                p.c2_output = true;
            }
        }
    }

    pub fn peek(&self, offset: u8) -> u8 {
        let side = if offset & 2 == 0 { Side::A } else { Side::B };
        let p = &self.ports[side.index()];
        if offset & 1 != 0 {
            p.control | (u8::from(p.irq1) << 7) | (u8::from(p.irq2) << 6)
        } else if p.control & 4 == 0 {
            p.direction
        } else {
            self.pins(side)
        }
    }

    pub fn read(&mut self, offset: u8) -> u8 {
        let value = self.peek(offset);
        let side = if offset & 2 == 0 { Side::A } else { Side::B };
        let p = &mut self.ports[side.index()];
        if offset & 1 == 0 && p.control & 4 != 0 {
            p.irq1 = false;
            p.irq2 = false;
            if matches!(side, Side::A) {
                Self::strobe(p);
            }
        }
        value
    }

    pub fn write(&mut self, offset: u8, value: u8) {
        let side = if offset & 2 == 0 { Side::A } else { Side::B };
        let p = &mut self.ports[side.index()];
        if offset & 1 != 0 {
            p.control = value & 0x3f;
            p.pulse = false;
            if p.control & 0x20 != 0 {
                p.irq2 = false;
                p.c2_output = (p.control >> 3) != 6;
            }
        } else if p.control & 4 == 0 {
            p.direction = value;
        } else {
            p.output = value;
            if matches!(side, Side::B) {
                Self::strobe(p);
            }
        }
    }

    fn strobe(p: &mut Port) {
        match p.control >> 3 {
            4 => p.c2_output = false,
            5 => {
                p.c2_output = false;
                p.pulse = true;
            }
            _ => {}
        }
    }

    fn alt(offset: u8) -> u8 {
        ((offset & 1) << 1) | ((offset & 2) >> 1)
    }

    pub fn peek_alt(&self, offset: u8) -> u8 {
        self.peek(Self::alt(offset))
    }

    pub fn read_alt(&mut self, offset: u8) -> u8 {
        self.read(Self::alt(offset))
    }

    pub fn write_alt(&mut self, offset: u8, value: u8) {
        self.write(Self::alt(offset), value);
    }

    /// Validate deserialized Chip state before committing a Machine restore.
    pub fn validate(&self) -> Result<(), String> {
        for p in &self.ports {
            if p.control & 0xc0 != 0
                || (p.pulse && ((p.control >> 3) != 5 || p.c2_output))
                || (p.control & 0x20 != 0 && p.irq2)
                || ((p.control >> 3) == 6 && p.c2_output)
                || ((p.control >> 3) == 7 && !p.c2_output)
            {
                return Err("invalid MOS 6520 PIA state".to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_selection_and_asymmetric_pin_readback() {
        let mut p = Pia::new();
        for side in [Side::A, Side::B] {
            let base = side.index() as u8 * 2;
            p.write(base, 0xf0);
            p.write(base + 1, 4);
            p.write(base, 0xa5);
            p.set_input(side, 0x5c);
            assert_eq!(p.drive(side), 0xaf);
        }
        assert_eq!(p.read(0), 0x0c);
        assert_eq!(p.read(2), 0xac);
    }

    #[test]
    fn masked_edges_latch_until_data_read_not_peek_or_direction_read() {
        let mut p = Pia::new();
        p.set_c1(Side::A, false);
        assert_eq!(p.peek(1), 0x80);
        assert!(!p.irq(Side::A));
        p.write(1, 1);
        assert!(p.irq(Side::A));
        p.read(0);
        assert!(p.irq(Side::A));
        p.write(1, 5);
        p.peek(0);
        assert!(p.irq(Side::A));
        p.read(0);
        assert!(!p.irq(Side::A));
        assert_eq!(p.peek(1), 5);
    }

    #[test]
    fn rising_edges_and_c2_output_clear_only_the_second_status() {
        let mut p = Pia::new();
        p.write(3, 0x1f);
        p.set_c1(Side::B, false);
        p.set_c2(Side::B, false);
        assert_eq!(p.peek(3) & 0xc0, 0);
        p.set_c1(Side::B, true);
        p.set_c2(Side::B, true);
        assert_eq!(p.peek(3), 0xdf);
        assert!(p.irq(Side::B));
        p.write(3, 0x34);
        assert_eq!(p.peek(3), 0xb4);
        assert_eq!(p.c2_output(Side::B), Some(false));
        p.set_c2(Side::B, false);
        assert_eq!(p.peek(3), 0xb4);
        p.read(2);
        assert_eq!(p.peek(3), 0x34);
    }

    #[test]
    fn read_a_and_write_b_strobe_pulses_last_one_clock() {
        let mut p = Pia::new();
        p.write(1, 0x2c);
        p.write(3, 0x2c);
        p.write(0, 7);
        p.read(2);
        assert_eq!(p.c2_output(Side::A), Some(true));
        assert_eq!(p.c2_output(Side::B), Some(true));
        p.read(0);
        p.write(2, 7);
        assert_eq!(p.c2_output(Side::A), Some(false));
        assert_eq!(p.c2_output(Side::B), Some(false));
        assert!(p.validate().is_ok());
        p.tick(0);
        assert_eq!(p.c2_output(Side::A), Some(false));
        p.tick(1);
        assert_eq!(p.c2_output(Side::A), Some(true));
        assert_eq!(p.c2_output(Side::B), Some(true));
    }

    #[test]
    fn handshake_waits_for_selected_c1_edge() {
        let mut p = Pia::new();
        p.write(1, 0x26);
        p.read(0);
        assert_eq!(p.c2_output(Side::A), Some(false));
        p.tick(10);
        p.set_c1(Side::A, false);
        assert_eq!(p.c2_output(Side::A), Some(false));
        p.set_c1(Side::A, true);
        assert_eq!(p.c2_output(Side::A), Some(true));
    }

    #[test]
    fn alternate_decode_and_reset_preserve_only_external_levels() {
        let mut p = Pia::new();
        p.write_alt(0, 0xff);
        p.write_alt(2, 0x3f);
        p.write_alt(0, 0);
        p.set_input(Side::A, 0x12);
        p.set_c1(Side::A, false);
        assert_eq!(p.peek_alt(4), 0);
        p.reset();
        assert_eq!(p.peek_alt(2), 0);
        assert_eq!(p.peek_alt(0), 0);
        p.write_alt(2, 4);
        assert_eq!(p.read_alt(0), 0x12);
        assert_eq!(p.c2_output(Side::A), None);
        assert!(p.validate().is_ok());
    }
}
