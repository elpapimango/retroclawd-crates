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

/// Force Interrupt does not stop the motor: MO goes low only after 9 idle revolutions ("Type I commands"), so a
/// SEEK without the h flag right after it runs at once instead of waiting 6 index pulses (none come without a disk).
#[test]
fn force_interrupt_keeps_the_motor_on() {
    let mut fdc = Wd1770::new(Empty, 8, true);
    fdc.write(0, 0x0B);
    fdc.tick(100_000);
    assert_eq!(fdc.read(0) & 0x80, 0x80, "motor on after RESTORE");
    fdc.write(0, 0xD0);
    fdc.tick(1000);
    assert_eq!(fdc.read(0) & 0x80, 0x80, "still on after Force Interrupt");
    fdc.write(3, 0);
    fdc.write(0, 0x13);
    fdc.tick(100_000);
    assert!(fdc.intrq(), "SEEK finished without a spin-up");
}

/// "If the Force Interrupt Command is received when there is a current command under execution, the Busy Status
/// Bit is reset, and the rest of the status bits are unchanged."
#[test]
fn force_interrupt_during_a_command_keeps_the_rest_of_the_status() {
    let mut fdc = Wd1770::new(Empty, 8, true);
    fdc.write(0, 0x0B);
    fdc.tick(100_000);
    fdc.write(0, 0x80); // READ SECTOR: waits for an ID field that never comes
    fdc.tick(10_000);
    let before = fdc.read(0);
    assert_eq!(before & 0x01, 0x01, "busy");
    fdc.write(0, 0xD0);
    fdc.tick(1000);
    assert_eq!(fdc.read(0), before & !0x01);
}
