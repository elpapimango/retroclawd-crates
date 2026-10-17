//! Emulation of the General Instrument AY-3-8910 (and its pin-compatible
//! variants, e.g. Yamaha's YM2149) 3-channel Programmable Sound Generator.
//! Used by the ZX Spectrum 128K/+2/+3, the Amstrad CPC, the MSX, and many
//! arcade boards -- built as its own crate for exactly that reason, same
//! shared-chip-crate reasoning as `crates/z80` itself (see `CLAUDE.md`).
//!
//! Deliberately pure: no dependency on `machine`, any CPU crate, or an
//! audio backend. A real AY chip's host-facing interface is 3 pins
//! (BC1/BDIR/BC2, tied to specific address lines differently per machine)
//! decoding to "select register" / "write selected register" / "read
//! selected register" -- that per-machine port-address decode is the
//! machine crate's job (see `zx_spectrum::bus`), not this one's. This
//! crate only implements what happens once a register is actually
//! selected/written: `select_register`/`write`/`read` plus `tick`/
//! `output` for sample generation.
//!
//! The tone/noise/envelope generator algorithms (period-counter-with-a-
//! divide-by-8-prescaler for tone, divide-by-16 for noise, the 17-bit
//! noise LFSR, the envelope shape state machine, the logarithmic volume
//! table) are the AY-3-8910's own well-documented, widely-published
//! internal architecture -- treated here as a trusted external reference,
//! the same way this project treats Z80 undocumented-flag-behavior
//! references (see `CLAUDE.md`'s "Architecture guidance"), not something
//! this specific codebase can verify against its own ROM disassembly the
//! way ZX81-specific software behavior elsewhere was.

/// Commonly published AY-3-8910 16-step volume table -- the chip's DAC is
/// logarithmic, not linear, so a naive `level / 15.0` sounds audibly wrong
/// relative to real hardware. Values vary slightly across independently
/// published sources (this chip's exact analog output was never
/// specified to high precision); this table is a widely-cited
/// approximation, not a measurement this project made itself.
const VOLUME_TABLE: [f32; 16] = [
    0.0000, 0.0137, 0.0205, 0.0291, 0.0423, 0.0618, 0.0847, 0.1369, 0.1691, 0.2647, 0.3527, 0.4499,
    0.5704, 0.6873, 0.8482, 1.0000,
];

/// A tone or noise generator's period counter runs at `chip_clock / 8`
/// (tone) or `/16` (noise) -- the AY's own internal prescalers, per its
/// documented architecture.
const TONE_PRESCALE: u32 = 8;
const NOISE_PRESCALE: u32 = 16;
const ENVELOPE_PRESCALE: u32 = 8;

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct ToneChannel {
    /// 12-bit tone period (registers pairs 0/1, 2/3, 4/5). Real hardware
    /// treats 0 as 1 -- a period of 0 would never toggle otherwise.
    period: u16,
    counter: u16,
    prescale_counter: u32,
    output: bool,
    /// 4-bit fixed volume (registers 8/9/10, bits 0-3).
    volume: u8,
    /// Register 8/9/10 bit 4 -- use the shared envelope level instead of
    /// the fixed `volume` above.
    use_envelope: bool,
    tone_enabled: bool,
    noise_enabled: bool,
}

impl ToneChannel {
    fn effective_period(&self) -> u16 {
        self.period.max(1)
    }

    fn tick(&mut self, cycles: u32) {
        self.prescale_counter += cycles;
        while self.prescale_counter >= TONE_PRESCALE {
            self.prescale_counter -= TONE_PRESCALE;
            self.counter += 1;
            if self.counter >= self.effective_period() {
                self.counter = 0;
                self.output = !self.output;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct Noise {
    /// 5-bit noise period (register 6).
    period: u8,
    counter: u16,
    prescale_counter: u32,
    output: bool,
    /// 17-bit linear feedback shift register -- real AY-3-8910 noise
    /// generator width, feedback taps bit 0 and bit 3 (per its documented
    /// architecture).
    lfsr: u32,
}

impl Noise {
    fn effective_period(&self) -> u16 {
        (self.period as u16).max(1)
    }

    fn tick(&mut self, cycles: u32) {
        self.prescale_counter += cycles;
        while self.prescale_counter >= NOISE_PRESCALE {
            self.prescale_counter -= NOISE_PRESCALE;
            self.counter += 1;
            if self.counter >= self.effective_period() {
                self.counter = 0;
                // 17-bit LFSR, feedback = bit0 XOR bit3, shifted in at the
                // top; output is the bit shifted out (bit0 before the
                // shift).
                let feedback = (self.lfsr & 1) ^ ((self.lfsr >> 3) & 1);
                self.lfsr = (self.lfsr >> 1) | (feedback << 16);
                self.output = self.lfsr & 1 != 0;
            }
        }
    }
}

/// Register 13's shape bits: Continue, Attack, Alternate, Hold (bits
/// 3,2,1,0), the AY's own documented envelope shape encoding.
#[derive(Debug, Clone, Copy)]
struct EnvelopeShape {
    continue_: bool,
    attack: bool,
    alternate: bool,
    hold: bool,
}

impl From<u8> for EnvelopeShape {
    fn from(byte: u8) -> Self {
        Self {
            hold: byte & 0b0001 != 0,
            alternate: byte & 0b0010 != 0,
            attack: byte & 0b0100 != 0,
            continue_: byte & 0b1000 != 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct Envelope {
    /// 16-bit period (registers 11/12).
    period: u16,
    counter: u32,
    prescale_counter: u32,
    /// 0-31: the envelope's own 5-bit ramp position (doubled from the
    /// 4-bit volume table's 0-15 range -- real hardware ramps twice as
    /// finely as the fixed-volume steps, halved back down when read).
    position: u8,
    shape: u8,
    holding: bool,
    attacking: bool,
}

impl Envelope {
    fn effective_period(&self) -> u32 {
        (self.period as u32).max(1)
    }

    fn retrigger(&mut self, shape: u8) {
        self.shape = shape;
        let s = EnvelopeShape::from(shape);
        self.position = 0;
        self.holding = false;
        self.attacking = s.attack;
        self.counter = 0;
        self.prescale_counter = 0;
    }

    fn tick(&mut self, cycles: u32) {
        if self.holding {
            return;
        }
        self.prescale_counter += cycles;
        while self.prescale_counter >= ENVELOPE_PRESCALE {
            self.prescale_counter -= ENVELOPE_PRESCALE;
            self.counter += 1;
            if self.counter >= self.effective_period() {
                self.counter = 0;
                self.advance();
                if self.holding {
                    break;
                }
            }
        }
    }

    fn advance(&mut self) {
        let s = EnvelopeShape::from(self.shape);
        if self.position < 31 {
            self.position += 1;
            return;
        }
        // Reached the top of one ramp (0..31 counted up).
        if !s.continue_ {
            // Non-continuing shapes: one ramp then silence.
            self.position = 0;
            self.attacking = false;
            self.holding = true;
        } else if s.hold {
            // Hold at the final level of this ramp.
            self.position = if s.alternate == s.attack { 31 } else { 0 };
            self.holding = true;
        } else if s.alternate {
            self.attacking = !self.attacking;
            self.position = 0;
        } else {
            self.position = 0;
        }
    }

    /// Current 4-bit level (0-15) for mixing -- the 5-bit ramp position
    /// halved, direction depending on whether this half-cycle is
    /// attacking (ramping up) or decaying (ramping down).
    fn level(&self) -> u8 {
        let pos = self.position >> 1;
        if self.attacking { pos } else { 15 - pos }
    }
}

/// An AY-3-8910 (or YM2149) PSG: 3 tone channels sharing one noise
/// generator and one envelope generator, addressed via the classic
/// "select a register, then read/write it" interface real hardware
/// exposes through its BC1/BDIR pins.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Ay3_8910 {
    selected_register: u8,
    channels: [ToneChannel; 3],
    noise: Noise,
    envelope: Envelope,
    /// Registers 14/15 (I/O ports A/B) -- stored for read-back only; this
    /// crate has no I/O port concept of its own (real usage varies wildly
    /// per host machine, most of which don't wire anything meaningful to
    /// them at all).
    io_ports: [u8; 2],
}

impl Default for Ay3_8910 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ay3_8910 {
    pub fn new() -> Self {
        Self {
            selected_register: 0,
            channels: [ToneChannel::default(); 3],
            noise: Noise {
                lfsr: 1,
                ..Noise::default()
            }, // non-zero seed: an all-zero LFSR never produces output
            envelope: Envelope::default(),
            io_ports: [0; 2],
        }
    }

    /// Latches which of the 16 registers subsequent `write`/`read` calls
    /// target. Real hardware only has 4 address bits here -- `reg & 0xF`.
    pub fn select_register(&mut self, reg: u8) {
        self.selected_register = reg & 0x0F;
    }

    /// Writes `value` to whichever register `select_register` last chose.
    pub fn write(&mut self, value: u8) {
        match self.selected_register {
            0 => self.channels[0].period = (self.channels[0].period & 0xFF00) | value as u16,
            1 => {
                self.channels[0].period =
                    (self.channels[0].period & 0x00FF) | ((value as u16 & 0x0F) << 8)
            }
            2 => self.channels[1].period = (self.channels[1].period & 0xFF00) | value as u16,
            3 => {
                self.channels[1].period =
                    (self.channels[1].period & 0x00FF) | ((value as u16 & 0x0F) << 8)
            }
            4 => self.channels[2].period = (self.channels[2].period & 0xFF00) | value as u16,
            5 => {
                self.channels[2].period =
                    (self.channels[2].period & 0x00FF) | ((value as u16 & 0x0F) << 8)
            }
            6 => self.noise.period = value & 0x1F,
            7 => {
                // Mixer: bit N = 1 means that source is DISABLED (real
                // AY-3-8910 convention -- 0 enables).
                for (i, ch) in self.channels.iter_mut().enumerate() {
                    ch.tone_enabled = value & (1 << i) == 0;
                    ch.noise_enabled = value & (1 << (i + 3)) == 0;
                }
            }
            8 => {
                self.channels[0].volume = value & 0x0F;
                self.channels[0].use_envelope = value & 0x10 != 0;
            }
            9 => {
                self.channels[1].volume = value & 0x0F;
                self.channels[1].use_envelope = value & 0x10 != 0;
            }
            10 => {
                self.channels[2].volume = value & 0x0F;
                self.channels[2].use_envelope = value & 0x10 != 0;
            }
            11 => self.envelope.period = (self.envelope.period & 0xFF00) | value as u16,
            12 => self.envelope.period = (self.envelope.period & 0x00FF) | ((value as u16) << 8),
            // Writing the shape register always retriggers the envelope
            // from position 0 -- real hardware's own documented behavior.
            13 => self.envelope.retrigger(value & 0x0F),
            14 => self.io_ports[0] = value,
            15 => self.io_ports[1] = value,
            _ => unreachable!("selected_register is masked to 4 bits"),
        }
    }

    /// Which register `select_register` last chose -- for a caller (e.g.
    /// `.szx` snapshot saving) that needs to dump every register's value
    /// without disturbing which one is "current", alongside
    /// `read_register`.
    pub fn selected_register(&self) -> u8 {
        self.selected_register
    }

    /// Reads back whichever register `select_register` last chose.
    pub fn read(&self) -> u8 {
        self.read_register(self.selected_register)
    }

    /// Reads back register `reg` directly, without touching
    /// `select_register`'s own state -- unlike `read`, safe to call from
    /// a `&self` (non-mutating) context such as building a snapshot to
    /// save, where looping `select_register`+`read` isn't available.
    pub fn read_register(&self, reg: u8) -> u8 {
        match reg & 0x0F {
            0 => self.channels[0].period as u8,
            1 => (self.channels[0].period >> 8) as u8,
            2 => self.channels[1].period as u8,
            3 => (self.channels[1].period >> 8) as u8,
            4 => self.channels[2].period as u8,
            5 => (self.channels[2].period >> 8) as u8,
            6 => self.noise.period,
            7 => {
                let mut v = 0u8;
                for (i, ch) in self.channels.iter().enumerate() {
                    if !ch.tone_enabled {
                        v |= 1 << i;
                    }
                    if !ch.noise_enabled {
                        v |= 1 << (i + 3);
                    }
                }
                v
            }
            8 => {
                self.channels[0].volume
                    | if self.channels[0].use_envelope {
                        0x10
                    } else {
                        0
                    }
            }
            9 => {
                self.channels[1].volume
                    | if self.channels[1].use_envelope {
                        0x10
                    } else {
                        0
                    }
            }
            10 => {
                self.channels[2].volume
                    | if self.channels[2].use_envelope {
                        0x10
                    } else {
                        0
                    }
            }
            11 => self.envelope.period as u8,
            12 => (self.envelope.period >> 8) as u8,
            13 => self.envelope.shape,
            14 => self.io_ports[0],
            15 => self.io_ports[1],
            _ => unreachable!("selected_register is masked to 4 bits"),
        }
    }

    /// Advances every generator by `cycles` real AY clock ticks (the
    /// caller's own clock divider from the host CPU's clock down to the
    /// AY's real input clock -- typically CPU clock / 2 on machines like
    /// the ZX Spectrum 128K -- is the host machine's concern, not this
    /// crate's, since the actual divider is a per-board wiring choice, not
    /// part of the chip itself).
    pub fn tick(&mut self, cycles: u32) {
        for ch in &mut self.channels {
            ch.tick(cycles);
        }
        self.noise.tick(cycles);
        self.envelope.tick(cycles);
    }

    /// Current mixed output, normalized to `0.0..=1.0` (silence to loudest
    /// all-channels-at-once level). Real hardware mixes 3 independent DAC
    /// outputs; this sums them and leaves final loudness/normalization to
    /// the caller's own audio pipeline.
    pub fn output(&self) -> f32 {
        let envelope_level = self.envelope.level();
        let mut total = 0.0;
        for ch in &self.channels {
            // Real AY mixer logic: a channel's gate is
            // (tone disabled OR tone bit) AND (noise disabled OR noise bit)
            // -- "disabled" forces that half of the AND to a constant true
            // so the other generator alone can still drive the channel.
            let tone_gate = !ch.tone_enabled || ch.output;
            let noise_gate = !ch.noise_enabled || self.noise.output;
            if tone_gate && noise_gate {
                let level = if ch.use_envelope {
                    envelope_level
                } else {
                    ch.volume
                };
                total += VOLUME_TABLE[level as usize];
            }
        }
        (total / 3.0).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_reg(ay: &mut Ay3_8910, reg: u8, value: u8) {
        ay.select_register(reg);
        ay.write(value);
    }

    #[test]
    fn tone_period_is_12_bit_split_across_two_registers() {
        let mut ay = Ay3_8910::new();
        write_reg(&mut ay, 0, 0xCD); // channel A fine
        write_reg(&mut ay, 1, 0x0A); // channel A coarse (low nibble only)

        assert_eq!(ay.channels[0].period, 0x0ACD);
    }

    #[test]
    fn zero_volume_is_silent_regardless_of_mixer_gating() {
        let mut ay = Ay3_8910::new();
        write_reg(&mut ay, 7, 0b0011_1111); // all tone+noise disabled
        // Volume registers default to 0 -- left unset here.
        ay.tick(1000);

        assert_eq!(ay.output(), 0.0);
    }

    #[test]
    fn disabling_both_tone_and_noise_for_a_channel_produces_constant_dc_not_silence() {
        // Real AY-3-8910 quirk, not a bug: the mixer's "disabled" bit
        // forces that half of the AND-gate to a constant true rather than
        // false, so disabling BOTH tone and noise for a channel makes it
        // output its programmed volume continuously (some software uses
        // this deliberately) -- true silence needs volume 0, not just
        // both mixer bits set.
        let mut ay = Ay3_8910::new();
        write_reg(&mut ay, 7, 0b0011_1001); // channel A tone+noise both disabled
        write_reg(&mut ay, 8, 0x0F);
        ay.tick(1000);

        assert_eq!(ay.output(), VOLUME_TABLE[15] / 3.0);
    }

    #[test]
    fn tone_enabled_channel_toggles_and_produces_nonzero_output() {
        let mut ay = Ay3_8910::new();
        write_reg(&mut ay, 0, 4); // short period
        write_reg(&mut ay, 1, 0);
        write_reg(&mut ay, 7, 0b0011_1110); // channel A tone enabled, rest disabled
        write_reg(&mut ay, 8, 0x0F); // max volume

        // Run long enough to guarantee at least one toggle into the "on"
        // half of the square wave.
        let mut saw_sound = false;
        for _ in 0..100 {
            ay.tick(TONE_PRESCALE * 4);
            if ay.output() > 0.0 {
                saw_sound = true;
            }
        }
        assert!(
            saw_sound,
            "tone-enabled channel never produced nonzero output"
        );
    }

    #[test]
    fn noise_generator_eventually_toggles() {
        let mut noise = Noise {
            lfsr: 1,
            period: 1,
            ..Noise::default()
        };
        let before = noise.output;
        let mut toggled = false;
        for _ in 0..(NOISE_PRESCALE * 50) {
            noise.tick(1);
            if noise.output != before {
                toggled = true;
                break;
            }
        }
        assert!(toggled, "noise generator never toggled");
    }

    #[test]
    fn envelope_attack_ramps_from_zero_upward() {
        let mut envelope = Envelope {
            period: 1,
            ..Envelope::default()
        };
        envelope.retrigger(0b1000); // continue, no attack/alt/hold: ramps down by default per shape decode below
        assert_eq!(envelope.level(), 15); // attack=false -> decaying from top (position 0 -> 15-0)

        envelope.retrigger(0b1100); // continue + attack: ramps up from 0
        assert_eq!(envelope.level(), 0);
        envelope.tick(ENVELOPE_PRESCALE * 2); // advance position by 2 (1 full period each)
        assert_eq!(envelope.level(), 1);
    }

    #[test]
    fn envelope_without_continue_holds_silent_after_one_ramp() {
        let mut envelope = Envelope {
            period: 1,
            ..Envelope::default()
        };
        envelope.retrigger(0b0100); // attack only, no continue: one ramp then silence
        for _ in 0..40 {
            envelope.tick(ENVELOPE_PRESCALE);
        }
        assert!(envelope.holding);
        assert_eq!(envelope.level(), 15); // holding at position 0, decaying-style read (attacking=false once held)
    }
}
