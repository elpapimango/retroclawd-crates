//! `DiskMedium` for a parsed `.dsk` image (`dsk::Disk`), behind the
//! optional `dsk` cargo feature -- so a host can hold an `Fdc<dsk::Disk>`
//! directly. The two crates stay independent by default (neither knows
//! about the other); this is the one place they meet, in one copy, instead
//! of one wrapper per host machine.
//!
//! `nec765::SectorId` and `dsk::Sector` use the same NEC765 N size-code
//! encoding, so no conversion beyond field renaming is needed.

use crate::{DiskMedium, SectorId, SectorInfo};

impl DiskMedium for dsk::Disk {
    fn sector_count(&self, cylinder: u8, head: u8) -> usize {
        dsk::Disk::sector_count(self, cylinder, head)
    }

    fn sector_info(&self, cylinder: u8, head: u8, index: usize) -> Option<SectorInfo> {
        let s = self.sector_at(cylinder, head, index)?;
        Some(SectorInfo {
            id: SectorId {
                cylinder: s.cylinder,
                head: s.head,
                sector: s.sector,
                size_code: s.size_code,
            },
            st1: s.st1,
            st2: s.st2,
        })
    }

    fn sector_data(&self, cylinder: u8, head: u8, index: usize, attempt: u32) -> Option<&[u8]> {
        let s = self.sector_at(cylinder, head, index)?;
        // Real weak/copy-protected sectors store several copies; rotate
        // through them the way a real drive returns a different one each
        // read. A normal sector has exactly one copy, so `attempt` has no
        // effect on it.
        Some(&s.copies[attempt as usize % s.copies.len()])
    }

    fn sides(&self) -> u8 {
        self.heads()
    }

    /// A `.dsk` image has no write-protect tab; the host decides whether
    /// modified images are persisted.
    fn write_protected(&self) -> bool {
        false
    }

    fn write_sector(
        &mut self,
        cylinder: u8,
        head: u8,
        index: usize,
        data: &[u8],
        deleted: bool,
    ) -> bool {
        self.write_sector_at(cylinder, head, index, data, deleted)
    }

    fn format_track(
        &mut self,
        cylinder: u8,
        head: u8,
        ids: &[SectorId],
        gap3: u8,
        filler: u8,
    ) -> bool {
        let ids: Vec<[u8; 4]> = ids
            .iter()
            .map(|id| [id.cylinder, id.head, id.sector, id.size_code])
            .collect();
        dsk::Disk::format_track(self, cylinder, head, &ids, gap3, filler)
    }
}

#[cfg(test)]
mod tests {
    use crate::{DiskMedium, Fdc};

    fn one_sector_dsk() -> dsk::Disk {
        const DISC: &[u8] = b"MV - CPCEMU Disk-File\r\nDisk-Info\r\n";
        const TRACK: &[u8] = b"Track-Info\r\n";
        let mut bytes = vec![0; 0x400];
        bytes[..DISC.len()].copy_from_slice(DISC);
        bytes[0x30] = 1;
        bytes[0x31] = 1;
        bytes[0x32..0x34].copy_from_slice(&0x0300u16.to_le_bytes());
        bytes[0x100..0x100 + TRACK.len()].copy_from_slice(TRACK);
        bytes[0x115] = 1;
        bytes[0x11A] = 1;
        bytes[0x11B] = 2;
        bytes[0x200..0x400].fill(0x5A);
        dsk::parse(&bytes).unwrap()
    }

    #[test]
    fn a_parsed_dsk_is_a_writable_medium() {
        let mut disk = one_sector_dsk();
        assert!(!disk.write_protected());
        assert_eq!(disk.sides(), 1);
        assert_eq!(DiskMedium::sector_count(&disk, 0, 0), 1);
        assert!(DiskMedium::write_sector(
            &mut disk, 0, 0, 0, &[7; 512], false
        ));
        assert_eq!(disk.sector_data(0, 0, 0, 0).unwrap()[0], 7);
        assert!(disk.is_modified());
    }

    #[test]
    fn the_fdc_writes_through_to_a_dsk_and_it_serialises() {
        let mut fdc: Fdc<dsk::Disk> = Fdc::new();
        fdc.insert_disk(one_sector_dsk());
        for b in [0x45, 0, 0, 0, 1, 2, 1, 0x2A, 0xFF] {
            fdc.write_data(b);
        }
        fdc.read_msr();
        for _ in 0..512 {
            fdc.write_data(0xC3);
        }
        let disk = fdc.disk().unwrap();
        assert!(disk.is_modified());
        let again = dsk::parse(&disk.to_bytes()).unwrap();
        assert_eq!(again.read_sector(0, 0, 1).unwrap().data, vec![0xC3; 512]);
    }
}
