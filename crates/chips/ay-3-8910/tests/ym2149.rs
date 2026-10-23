//! The YM2149 variant against Yamaha's YM2149 catalog (September 1992): register array (Table 3), tone frequency
//! `fT = fMaster / (16 TP)`, envelope frequency `fE = fMaster / (256 EP)` with 32 steps per period, the envelope
//! shapes of Table 4 and the D/A converter levels of Fig. 1.

use ay_3_8910::{Ay3_8910, Variant};

fn write_reg(ay: &mut Ay3_8910, reg: u8, value: u8) {
    ay.select_register(reg);
    ay.write(value);
}

/// Envelope levels sampled once per envelope step (8 master clocks with EP = 1), starting right after the shape write.
fn envelope_steps(ay: &mut Ay3_8910, shape: u8, steps: usize) -> Vec<u8> {
    write_reg(ay, 11, 1);
    write_reg(ay, 12, 0);
    write_reg(ay, 13, shape);
    let mut levels = Vec::with_capacity(steps);
    for _ in 0..steps {
        levels.push(ay.envelope_level());
        ay.tick(8);
    }
    levels
}

#[test]
fn ym_envelope_counts_32_steps_per_period() {
    let mut ym = Ay3_8910::with_variant(Variant::Ym2149);
    let levels = envelope_steps(&mut ym, 0x0C, 33);
    let expected: Vec<u8> = (0..32).chain([0]).collect();
    assert_eq!(levels, expected);
}

#[test]
fn ay_envelope_is_unchanged() {
    let mut ay = Ay3_8910::new();
    let levels = envelope_steps(&mut ay, 0x0C, 33);
    let expected: Vec<u8> = (0..32).map(|p| p >> 1).chain([0]).collect();
    assert_eq!(levels, expected);
}

#[test]
fn ym_tone_period_matches_datasheet() {
    let mut ym = Ay3_8910::with_variant(Variant::Ym2149);
    write_reg(&mut ym, 0, 100);
    write_reg(&mut ym, 1, 0);
    write_reg(&mut ym, 7, 0b0011_1110); // tone A only
    write_reg(&mut ym, 8, 0x0F);
    let mut edges = Vec::new();
    let mut last = ym.output();
    for clock in 1..=4000u32 {
        ym.tick(1);
        let now = ym.output();
        if now != last {
            edges.push(clock);
            last = now;
        }
    }
    // A full period is 16 × TP = 1600 master clocks: the output changes every 800.
    assert_eq!(edges, vec![800, 1600, 2400, 3200, 4000]);
}

/// Table 4, one row per shape: the level over two envelope periods (64 steps).
#[test]
fn ym_shapes_follow_table_4() {
    let down: Vec<u8> = (0..32).rev().collect();
    let up: Vec<u8> = (0..32).collect();
    let hold = |v: u8| vec![v; 32];
    let cat = |a: &[u8], b: &[u8]| [a, b].concat();
    for shape in 0u8..16 {
        let expected = match shape {
            0x00..=0x03 | 0x09 => cat(&down, &hold(0)),
            0x04..=0x07 | 0x0F => cat(&up, &hold(0)),
            0x08 => cat(&down, &down),
            0x0A => cat(&down, &up),
            0x0B => cat(&down, &hold(31)),
            0x0C => cat(&up, &up),
            0x0D => cat(&up, &hold(31)),
            0x0E => cat(&up, &down),
            _ => unreachable!(),
        };
        let mut ym = Ay3_8910::with_variant(Variant::Ym2149);
        assert_eq!(
            envelope_steps(&mut ym, shape, 64),
            expected,
            "shape {shape:#04x}"
        );
    }
}

/// Fig. 1: step 31 is full scale, each step down is 2^(-1/4) (the axis reads 1.0, .841, .707, … .125), step 0 is
/// silence; a fixed level L sits at envelope step 2L + 1.
#[test]
fn ym_dac_follows_fig_1() {
    let mut ym = Ay3_8910::with_variant(Variant::Ym2149);
    write_reg(&mut ym, 7, 0b0011_1111); // tone and noise off: the channel outputs its level
    write_reg(&mut ym, 8, 0x0F);
    let full = ym.output();
    write_reg(&mut ym, 8, 0x0E);
    let fourteen = ym.output();
    assert!(
        (fourteen / full - 0.707).abs() < 0.001,
        "{}",
        fourteen / full
    );
    write_reg(&mut ym, 8, 0x0A); // step 21: 2^(-10/4)
    assert!((ym.output() / full - 0.177).abs() < 0.001);
    write_reg(&mut ym, 8, 0x00);
    assert_eq!(ym.output(), 0.0);
}

/// The variant is configuration, not state: it is never serialized, so the save states machines already wrote with
/// a non-self-describing format (bincode, in the Spectrum and CPC) keep their layout. The owner re-applies it with
/// `set_variant` after loading.
#[test]
fn variant_is_not_serialized() {
    let ay = serde_json::to_value(Ay3_8910::new()).unwrap();
    let ym = serde_json::to_value(Ay3_8910::with_variant(Variant::Ym2149)).unwrap();
    assert!(ay.get("variant").is_none(), "{ay}");
    assert_eq!(ay, ym, "an AY and a YM in the same state serialize alike");
    let mut back: Ay3_8910 = serde_json::from_value(ym).unwrap();
    assert_eq!(back.variant(), Variant::Ay38910);
    back.set_variant(Variant::Ym2149);
    assert_eq!(back.variant(), Variant::Ym2149);
}
