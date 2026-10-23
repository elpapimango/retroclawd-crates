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

/// A disk whose ID field carries a size code beyond the data sheet's table (`$FF`): the controller uses the low two
/// bits (the table lists 00-03), so a hostile image cannot make it panic.
struct OneSector {
    track: Vec<u16>,
    pos: usize,
}

impl OneSector {
    fn new(size_code: u8) -> Self {
        let crc = |bytes: &[u8]| bytes.iter().fold(0xFFFFu16, |c, &b| wd1770::crc_byte(c, b));
        let mut t: Vec<u16> = vec![0x4E; 40];
        t.extend([0u16; 12]);
        let id = [0xA1, 0xA1, 0xA1, 0xFE, 0, 0, 1, size_code];
        let c = crc(&id);
        t.extend([0x1A1; 3]);
        t.extend(id[3..].iter().map(|&b| b as u16));
        t.extend([c >> 8, c & 0xFF]);
        t.extend(vec![0x4E; 22]);
        t.extend([0u16; 12]);
        let mut data = vec![0xA1, 0xA1, 0xA1, 0xFB];
        data.extend(vec![0x5A; 1024]);
        let c = crc(&data);
        t.extend([0x1A1; 3]);
        t.extend(data[3..].iter().map(|&b| b as u16));
        t.extend([c >> 8, c & 0xFF]);
        t.resize(6250, 0x4E);
        Self { track: t, pos: 0 }
    }
}

impl Floppy for OneSector {
    fn index(&self) -> bool {
        self.pos < 64
    }
    fn index_count(&self) -> u32 {
        0
    }
    fn index_count_reset(&mut self) {}
    fn rotate(&mut self, bytes: u64) {
        self.pos = ((self.pos as u64 + bytes) % self.track.len() as u64) as usize;
    }
    fn track0(&self) -> bool {
        true
    }
    fn write_protect(&self) -> bool {
        true
    }
    fn seek_pulse(&mut self, _: bool) {}
    fn read(&mut self) -> u16 {
        let b = self.track[self.pos];
        self.rotate(1);
        b
    }
    fn write(&mut self, _: u16) {
        self.rotate(1);
    }
}

#[test]
fn a_size_code_beyond_the_table_uses_its_low_two_bits() {
    let mut fdc = Wd1770::new(OneSector::new(0xFF), 8, true);
    fdc.write(2, 1);
    fdc.write(0, 0x88); // READ SECTOR, motor on
    let mut bytes = 0;
    for _ in 0..20_000 {
        fdc.tick(256);
        if fdc.drq() {
            fdc.read(3);
            bytes += 1;
        }
        if fdc.intrq() {
            break;
        }
    }
    assert_eq!(bytes, 1024, "size code $FF read as 3 (1024 bytes)");
}

/// A command is pending from the write that gives it until it finishes, including the 24 microseconds before Busy is set.
#[test]
fn a_command_is_pending_from_its_write_until_it_finishes() {
    let mut fdc = Wd1770::new(Empty, 8, true);
    assert!(!fdc.command_pending());
    fdc.write(0, 0x0B); // restore, h = 1
    assert!(fdc.command_pending(), "pending at once");
    assert_eq!(fdc.peek(0) & 1, 0, "Busy is not set yet");
    fdc.tick(100_000);
    assert!(fdc.intrq());
    assert!(!fdc.command_pending(), "finished");
}

/// A drive that names its revolution makes the spin-up last six whole revolutions, whatever the phase of the index hole.
struct Turning {
    pos: u64,
}

impl Floppy for Turning {
    fn index(&self) -> bool {
        self.pos < 100
    }
    fn index_count(&self) -> u32 {
        0
    }
    fn index_count_reset(&mut self) {}
    fn rotate(&mut self, bytes: u64) {
        self.pos = (self.pos + bytes) % 6267;
    }
    fn revolution_bytes(&self) -> u64 {
        6267
    }
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

#[test]
fn spin_up_lasts_six_whole_revolutions_when_the_drive_names_its_revolution() {
    let revolution = 6267 * 256;
    for phase in [0u64, 1000, 4000] {
        let mut fdc = Wd1770::new(Turning { pos: phase }, 8, true);
        fdc.write(0, 0x00); // restore at 6 ms steps, h = 0: the motor must spin up first
        let mut cycles = 0u64;
        while !fdc.intrq() && cycles < 20 * revolution {
            fdc.tick(256);
            cycles += 256;
        }
        assert!(fdc.intrq());
        assert!(
            (6 * revolution..6 * revolution + 4000).contains(&cycles),
            "phase {phase}: done after {cycles} cycles"
        );
    }
}
