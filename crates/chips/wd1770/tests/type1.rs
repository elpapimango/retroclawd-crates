//! Type I status against the WD177X-00 data sheet ("Status Register Description": S5 "When set, this bit indicates
//! that the Motor Spin-Up sequence has completed (6 revolutions) on Type I commands").

use wd1770::{Floppy, Wd1770};

/// A drive with no disk: the head is at track 0, no index pulses, write-protect sensed.
struct Empty;

impl Floppy for Empty {
    fn index(&self) -> bool {
        false
    }
    fn index_count(&self) -> u32 {
        0
    }
    fn index_count_reset(&mut self) {}
    fn rotate(&mut self, _: u64) {}
    fn track0(&self) -> bool {
        true
    }
    fn write_protect(&self) -> bool {
        true
    }
    fn seek_pulse(&mut self, _: bool) {}
    fn read(&mut self) -> u16 {
        0
    }
    fn write(&mut self, _: u16) {}
}

/// RESTORE with the spin-up disabled (h = 1): the spin-up phase is over at once, so S5 is set with the motor on,
/// write protect and track 0.
#[test]
fn restore_with_h_sets_spin_up_complete() {
    let mut fdc = Wd1770::new(Empty, 8, true);
    fdc.write(0, 0xD0);
    fdc.tick(1000);
    fdc.write(0, 0x0B);
    fdc.tick(100_000);
    assert!(fdc.intrq());
    assert_eq!(fdc.read(0), 0xE4);
}
