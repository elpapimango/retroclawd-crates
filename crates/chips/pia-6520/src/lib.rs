//! MOS 6520 PIA, without machine-specific wiring or a CPU dependency.
//!
//! Register, pin and reset behavior: Avery Lee, Altirra Hardware Reference
//! Manual (2026-01-02), sections 2.5 and 14.5, pages 34-36 and 453-456:
//! <https://www.virtualdub.org/downloads/Altirra%20Hardware%20Reference%20Manual.pdf>.
//! `read`/`write` use A, CRA, B, CRB ordering; `*_alt` swap the address bits.
//! A pulse lasts one caller-supplied clock. Analogue floating-input decay is
//! not modeled. Save `PiaControlState` alongside `Pia` to preserve C2 output
//! history without changing the original serialized register/pin layout.
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
    #[serde(skip)]
    control_state: PiaControlState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum C2History {
    #[default]
    None,
    Low,
    Risen,
}

/// C2 transition history, stored separately to preserve the legacy `Pia` layout.
/// Legacy restores without this checkpoint start with no pending transition.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiaControlState {
    history: [C2History; 2],
}

impl PiaControlState {
    /// Check this extension against the register state before committing a restore.
    pub fn validate(&self, pia: &Pia) -> Result<(), String> {
        for (history, port) in self.history.iter().zip(&pia.ports) {
            let mode = port.control >> 3;
            let valid = match history {
                C2History::None => true,
                C2History::Low => matches!(mode, 4 | 6),
                C2History::Risen => matches!(mode, 4 | 7),
            };
            if !valid {
                return Err("invalid MOS 6520 PIA control history".to_string());
            }
        }
        Ok(())
    }
}

impl Pia {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset internal registers; external pin levels are board-owned.
    pub fn reset(&mut self) {
        self.control_state = PiaControlState::default();
        for p in &mut self.ports {
            *p = Port {
                input: p.input,
                c1: p.c1,
                c2: p.c2,
                ..Port::default()
            };
        }
    }

    pub fn save_control(&self) -> PiaControlState {
        self.control_state.clone()
    }

    /// Legacy states do not contain C2 history; leave their registers/pins intact.
    pub fn initialize_legacy_control(&mut self) {
        self.control_state = PiaControlState::default();
    }

    pub fn restore_control(&mut self, state: &PiaControlState) -> Result<(), String> {
        state.validate(self)?;
        self.control_state = state.clone();
        Ok(())
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
            // Altirra Hardware Reference 2.5, "Spurious interrupts" (p.36),
            // and Acid800 1.2 pia_irq.s: handshake preserves the forced-low
            // history, pulse and input modes consume it. CA2 needs a previous
            // low and rising-edge input; CB2 needs an output low/high pair.
            let history = &mut self.control_state.history[side.index()];
            let mode = (value & 0x38) >> 3;
            if mode < 4 {
                if p.control & 0x20 != 0 {
                    p.irq2 = match side {
                        Side::A => !matches!(history, C2History::None) && mode & 2 != 0,
                        Side::B => matches!(history, C2History::Risen),
                    };
                }
                *history = C2History::None;
            } else {
                *history = match mode {
                    5 => C2History::None,
                    6 => C2History::Low,
                    7 if !matches!(history, C2History::None) => C2History::Risen,
                    _ => *history,
                };
            }
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
        self.control_state.validate(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_output_to_input_transitions_latch_c2_status() {
        // Altirra Hardware Reference 2.5, "Spurious interrupts"; Acid800 1.2
        // pia_irq.s checks these histories without an external C2 pin edge.
        let cases = [
            (Side::A, [0x3c, 0x3c, 0x3c, 0x14], 0x14, false),
            (Side::A, [0x3c, 0x3c, 0x34, 0x14], 0x54, false),
            (Side::A, [0x3c, 0x34, 0x3c, 0x14], 0x54, false),
            (Side::A, [0x3c, 0x34, 0x3c, 0x1c], 0x5c, true),
            (Side::A, [0x34, 0x34, 0x34, 0x04], 0x04, false),
            (Side::A, [0x34, 0x34, 0x2c, 0x14], 0x14, false),
            (Side::A, [0x34, 0x34, 0x24, 0x14], 0x54, false),
            (Side::A, [0x34, 0x34, 0x14, 0x34], 0x34, false),
            (Side::B, [0x34, 0x3c, 0x3c, 0x04], 0x44, false),
            (Side::B, [0x34, 0x3c, 0x3c, 0x0c], 0x4c, true),
            (Side::B, [0x34, 0x3c, 0x04, 0x04], 0x44, false),
            (Side::B, [0x34, 0x3c, 0x04, 0x24], 0x24, false),
            (Side::B, [0x34, 0x24, 0x3c, 0x04], 0x44, false),
            (Side::B, [0x34, 0x28, 0x3c, 0x04], 0x04, false),
            (Side::B, [0x34, 0x04, 0x3c, 0x04], 0x04, false),
            (Side::B, [0x34, 0x3c, 0x34, 0x04], 0x04, false),
            (Side::B, [0x3c, 0x34, 0x3c, 0x04], 0x44, false),
        ];
        for (side, writes, expected, irq) in cases {
            let mut p = Pia::new();
            let control = side.index() as u8 * 2 + 1;
            for value in writes {
                p.write(control, value);
                p.tick(8);
                assert!(p.validate().is_ok());
            }
            assert_eq!(p.read(control), expected, "{side:?}: {writes:02x?}");
            assert_eq!(p.irq(side), irq, "{side:?}: {writes:02x?}");
        }
    }

    #[test]
    fn control_generated_c2_status_obeys_acknowledgement_and_masks() {
        for side in [Side::A, Side::B] {
            let mut p = Pia::new();
            let data = side.index() as u8 * 2;
            let control = data + 1;
            for value in [0x3c, 0xc0, 0xff] {
                p.write(control, value);
                assert_eq!(p.read(control), value & 0x3f);
                assert!(!p.irq(side));
            }
            for value in [0x34, 0x3c, 0x14] {
                p.write(control, value);
            }
            assert_eq!(p.read(control), 0x54);
            assert!(!p.irq(side));
            p.write(control, 0x00);
            p.read(data);
            assert_eq!(p.read(control), 0x40);
            p.write(control, 0x18);
            p.read(data);
            assert_eq!(p.read(control), 0x58);
            assert!(p.irq(side));
            p.write(control, 0x1c);
            p.peek(data);
            p.write(data, 0xff);
            assert_eq!(p.read(control), 0x5c);
            p.read(data);
            assert_eq!(p.read(control), 0x1c);
            assert!(!p.irq(side));
            p.write(control, 0x14);
            assert_eq!(p.read(control), 0x14);
        }
    }

    #[test]
    fn c2_history_checkpoint_reset_and_invalid_restore() {
        let mut p = Pia::new();
        p.write(1, 0x34);
        p.write(1, 0x24);
        p.write(3, 0x34);
        p.write(3, 0x3c);
        let state = p.save_control();
        let mut legacy = p.clone();
        legacy.initialize_legacy_control();
        assert_eq!(legacy.peek(1), 0x24);
        assert_eq!(legacy.peek(3), 0x3c);
        for control in [1, 3] {
            legacy.write(control, 0x14);
            assert_eq!(legacy.peek(control), 0x14);
        }
        let mut restored = p.clone();
        restored.initialize_legacy_control();
        restored.restore_control(&state).unwrap();
        for (pia, expected) in [(&mut p, 0x54), (&mut restored, 0x54)] {
            pia.write(1, 0x14);
            pia.write(3, 0x14);
            assert_eq!(pia.peek(1), expected);
            assert_eq!(pia.peek(3), expected);
        }
        let previous = restored.save_control();
        assert!(restored.restore_control(&state).is_err());
        assert_eq!(restored.save_control(), previous);
        restored.write(1, 0x34);
        restored.write(3, 0x34);
        restored.write(3, 0x3c);
        restored.reset();
        for control in [1, 3] {
            restored.write(control, 0x14);
            assert_eq!(restored.peek(control), 0x14);
        }
    }

    #[test]
    fn c2_history_does_not_replace_external_edges_or_affect_other_port() {
        for side in [Side::A, Side::B] {
            let mut p = Pia::new();
            let control = side.index() as u8 * 2 + 1;
            p.write(control, 0x34);
            p.write(control, 0x3c);
            p.set_c2(side, false);
            p.write(control, 0x04);
            assert_eq!(
                p.peek(control) & 0x40,
                if side.index() == 0 { 0 } else { 0x40 }
            );
            p.read(control - 1);
            p.set_c2(side, true);
            assert_eq!(p.peek(control), 0x04);
            p.set_c2(side, false);
            assert_eq!(p.peek(control), 0x44);
            assert_eq!(p.peek(if control == 1 { 3 } else { 1 }), 0);
        }
    }

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
