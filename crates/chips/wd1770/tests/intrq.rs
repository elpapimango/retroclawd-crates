//! INTRQ: "INTRQ is reset by either reading the status register or by loading the command register with a new
//! command" (a WD177X data sheet transcription, <https://www.worldofsam.org/node/1097>, "INTRQ"). The Atari ST's TOS
//! writes a command and looks at the interrupt line at once, so the line must fall with the write, not after the
//! controller's own start-up delay.

use wd1770::{Floppy, Wd1770};

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

/// Type I, II and III commands alike (the ST's TOS reads the status after a seek but writes the next command without
/// reading it when it only wanted the interrupt, then looks at the line).
#[test]
fn a_new_command_resets_intrq_at_once() {
    for command in [0x1B, 0x80, 0xC0] {
        // SEEK (h = 1), READ SECTOR, READ ADDRESS
        let mut fdc = Wd1770::new(Empty, 8, true);
        fdc.write(0, 0x0B); // RESTORE, no spin-up
        fdc.tick(100_000);
        assert!(fdc.intrq(), "the command completed");
        fdc.write(3, 5);
        fdc.write(0, command); // written without reading the status
        assert!(
            !fdc.intrq(),
            "INTRQ falls with the write of command {command:#04x}"
        );
    }
}

#[test]
fn reading_the_status_still_resets_intrq() {
    let mut fdc = Wd1770::new(Empty, 8, true);
    fdc.write(0, 0x0B);
    fdc.tick(100_000);
    assert!(fdc.intrq());
    fdc.read(0);
    assert!(!fdc.intrq());
}
