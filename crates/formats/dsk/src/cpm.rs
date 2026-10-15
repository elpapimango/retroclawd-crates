//! The CP/M file system on a CPCEMU `.dsk`: what AMSDOS (Amstrad CPC) and +3DOS (ZX Spectrum +3) put on
//! a 3-inch disk. A [`Cpm`] wraps a [`Disk`] and reads, writes, deletes and renames files in it, checks
//! it, and formats a new one.
//!
//! **Geometry.** The standard single-sided formats are known by the ID of the first sector on track 0
//! (what the format commands write): `$C1` is the CPC *data* format (9 x 512 bytes, no reserved
//! tracks), `$41` the CPC *system* format (2 reserved tracks), `$01` the +3 format (9 sectors, 1 reserved
//! track) or the CPC IBM format (8 sectors, 1 reserved track). Block size is 1 KB and the directory is 2
//! blocks (64 entries). A +3 disk may carry a 16-byte *disk specification* in its first sector; when
//! it is there and sane it decides the geometry (so 80-track and other layouts are read). Logical
//! sector `n` of a track is the sector whose ID is `first_id + n` -- the physical interleave of a track
//! does not matter.
//!
//! **Directory.** 32-byte entries: user (`$E5` = free), 8 name + 3 extension characters whose top bits
//! are the read-only / system / archived attributes, extent number (`EX` + `S2` * 32), record count in
//! the extent, then 16 block numbers (8 bit while the disk has up to 256 blocks, 16 bit above). A file
//! is its extents in order; its size is `(last extent) * 128 + last record count` 128-byte records, so
//! the exact byte length of a file is not stored (AMSDOS and +3DOS put it in a 128-byte header inside
//! the file; see the `rcdisk` tool). Reading handles any block size; writing is limited to the
//! standard 1 KB / 8-bit layout.
//!
//! Only side 0 of a double-sided image is used.

pub mod header;

use crate::{Disk, sector_size};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

const ENTRY_SIZE: usize = 32;
const FREE: u8 = 0xE5;
const RECORD: usize = 128;
/// The disk specification: byte-sum rule that marks a +3 disk as bootable.
const BOOT_SUM: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// CPC data format: sector IDs `$C1..`, no reserved tracks, 178 KB free.
    CpcData,
    /// CPC system format: sector IDs `$41..`, 2 reserved tracks for the boot code.
    CpcSystem,
    /// CPC IBM format: 8 sectors, IDs from 1, 1 reserved track.
    CpcIbm,
    /// Spectrum +3 format: 9 sectors, IDs from 1, 1 reserved track.
    Plus3,
}

impl Layout {
    pub fn name(self) -> &'static str {
        match self {
            Layout::CpcData => "cpc-data",
            Layout::CpcSystem => "cpc-system",
            Layout::CpcIbm => "cpc-ibm",
            Layout::Plus3 => "plus3",
        }
    }

    pub fn parse(text: &str) -> Option<Layout> {
        match text.to_ascii_lowercase().as_str() {
            "cpc-data" | "data" => Some(Layout::CpcData),
            "cpc-system" | "system" => Some(Layout::CpcSystem),
            "cpc-ibm" | "ibm" => Some(Layout::CpcIbm),
            "plus3" | "+3" => Some(Layout::Plus3),
            _ => None,
        }
    }

    pub fn geometry(self) -> Geometry {
        let (spt, first_sector, reserved_tracks) = match self {
            Layout::CpcData => (9, 0xC1, 0),
            Layout::CpcSystem => (9, 0x41, 2),
            Layout::CpcIbm => (8, 0x01, 1),
            Layout::Plus3 => (9, 0x01, 1),
        };
        Geometry {
            tracks: 40,
            sectors_per_track: spt,
            sector_size_code: 2,
            first_sector,
            reserved_tracks,
            block_shift: 3,
            dir_blocks: 2,
        }
    }
}

/// How the disk is laid out for the file system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub tracks: u8,
    pub sectors_per_track: u8,
    /// `N` of the sector IDs (2 = 512 bytes).
    pub sector_size_code: u8,
    /// ID of the first sector of every track.
    pub first_sector: u8,
    pub reserved_tracks: u8,
    /// `BSH`: the block size is `128 << block_shift`.
    pub block_shift: u8,
    pub dir_blocks: u8,
}

impl Geometry {
    pub fn sector_bytes(&self) -> usize {
        sector_size(self.sector_size_code)
    }
    pub fn block_bytes(&self) -> usize {
        RECORD << self.block_shift
    }
    pub fn sectors_per_block(&self) -> usize {
        self.block_bytes() / self.sector_bytes()
    }
    /// Blocks on the disk, directory included (`DSM + 1`).
    pub fn blocks(&self) -> usize {
        let data_sectors = (self.tracks as usize - self.reserved_tracks as usize)
            * self.sectors_per_track as usize;
        data_sectors / self.sectors_per_block().max(1)
    }
    pub fn dir_entries(&self) -> usize {
        self.dir_blocks as usize * self.block_bytes() / ENTRY_SIZE
    }
    /// Block numbers are 16 bits wide in an entry once the disk has more than 256 blocks.
    pub fn wide_pointers(&self) -> bool {
        self.blocks() > 256
    }
    pub fn pointers_per_entry(&self) -> usize {
        if self.wide_pointers() { 8 } else { 16 }
    }
    /// Bytes of file one directory entry can address.
    pub fn bytes_per_entry(&self) -> usize {
        self.pointers_per_entry() * self.block_bytes()
    }
    /// Whether this layout can be written (the standard one-extent-per-16-KB layout).
    fn writable(&self) -> bool {
        self.bytes_per_entry() == 16 * 1024
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The image does not look like a CP/M disk this module understands.
    NoGeometry(String),
    InvalidName(String),
    NotFound,
    Exists,
    DiskFull,
    DirectoryFull,
    Unsupported(String),
    Corrupt(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoGeometry(why) => write!(f, "not a recognised CP/M disk: {why}"),
            Error::InvalidName(why) => write!(f, "invalid file name: {why}"),
            Error::NotFound => f.write_str("file not found"),
            Error::Exists => f.write_str("file exists"),
            Error::DiskFull => f.write_str("disk full"),
            Error::DirectoryFull => f.write_str("directory full"),
            Error::Unsupported(why) => write!(f, "{why}"),
            Error::Corrupt(why) => write!(f, "damaged disk: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// A file as the directory lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub user: u8,
    /// `NAME.EXT`, trailing blanks removed, attribute bits cleared.
    pub name: String,
    /// The 8 + 3 name bytes with attribute bits cleared.
    pub raw_name: [u8; 11],
    pub records: u32,
    /// `records * 128`: the CP/M size. The true length is in the file's header, if it has one.
    pub bytes: u64,
    /// Blocks the file occupies.
    pub blocks: usize,
    pub extents: usize,
    pub read_only: bool,
    pub system: bool,
    pub archived: bool,
}

/// Something wrong with the disk found by [`Cpm::check`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub error: bool,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Cpm {
    disk: Disk,
    geo: Geometry,
}

/// A directory entry with its place.
#[derive(Clone, Debug)]
struct Entry {
    index: usize,
    raw: [u8; ENTRY_SIZE],
}

impl Entry {
    fn user(&self) -> u8 {
        self.raw[0]
    }
    fn is_file(&self) -> bool {
        self.raw[0] <= 15
    }
    fn name(&self) -> [u8; 11] {
        let mut n = [0u8; 11];
        for (d, s) in n.iter_mut().zip(&self.raw[1..12]) {
            *d = s & 0x7F;
        }
        n
    }
    fn extent(&self) -> usize {
        self.raw[12] as usize + 32 * (self.raw[14] as usize & 0x3F)
    }
    fn records(&self) -> usize {
        self.raw[15] as usize
    }
}

impl Cpm {
    /// Opens a disk, recognising its format.
    pub fn open(disk: Disk) -> Result<Cpm, Error> {
        let geo = detect(&disk)?;
        Ok(Cpm { disk, geo })
    }

    /// A new, empty, formatted disk.
    pub fn format(layout: Layout) -> Cpm {
        let geo = layout.geometry();
        let mut disk = Disk::new(1);
        for cylinder in 0..geo.tracks {
            let ids: Vec<[u8; 4]> = (0..geo.sectors_per_track)
                .map(|i| [cylinder, 0, geo.first_sector + i, geo.sector_size_code])
                .collect();
            disk.format_track(cylinder, 0, &ids, 0x4E, FREE);
        }
        let mut cpm = Cpm { disk, geo };
        if layout == Layout::Plus3 {
            cpm.write_plus3_spec();
        }
        cpm.disk.clear_modified();
        cpm
    }

    pub fn geometry(&self) -> &Geometry {
        &self.geo
    }

    pub fn disk(&self) -> &Disk {
        &self.disk
    }

    pub fn into_disk(self) -> Disk {
        self.disk
    }

    // ---- sectors and blocks ----

    fn lsn_to_id(&self, lsn: usize) -> (u8, u8) {
        let spt = self.geo.sectors_per_track as usize;
        ((lsn / spt) as u8, self.geo.first_sector + (lsn % spt) as u8)
    }

    fn sector(&self, lsn: usize) -> Result<&[u8], Error> {
        let (cyl, id) = self.lsn_to_id(lsn);
        let s = self.disk.read_sector(cyl, 0, id).ok_or_else(|| {
            Error::Corrupt(format!("track {cyl} has no sector with ID {id:#04x}"))
        })?;
        if s.data.len() != self.geo.sector_bytes() {
            return Err(Error::Corrupt(format!(
                "track {cyl} sector {id:#04x} holds {} bytes, not {}",
                s.data.len(),
                self.geo.sector_bytes()
            )));
        }
        Ok(&s.data)
    }

    fn write_sector(&mut self, lsn: usize, data: &[u8]) -> Result<(), Error> {
        let (cyl, id) = self.lsn_to_id(lsn);
        let count = self.disk.sector_count(cyl, 0);
        let index = (0..count)
            .find(|&i| {
                self.disk
                    .sector_at(cyl, 0, i)
                    .is_some_and(|s| s.sector == id)
            })
            .ok_or_else(|| {
                Error::Corrupt(format!("track {cyl} has no sector with ID {id:#04x}"))
            })?;
        self.disk.write_sector_at(cyl, 0, index, data, false);
        Ok(())
    }

    fn block_first_lsn(&self, block: usize) -> usize {
        self.geo.reserved_tracks as usize * self.geo.sectors_per_track as usize
            + block * self.geo.sectors_per_block()
    }

    fn read_block(&self, block: usize) -> Result<Vec<u8>, Error> {
        let first = self.block_first_lsn(block);
        let mut out = Vec::with_capacity(self.geo.block_bytes());
        for i in 0..self.geo.sectors_per_block() {
            out.extend_from_slice(self.sector(first + i)?);
        }
        Ok(out)
    }

    fn write_block(&mut self, block: usize, data: &[u8]) -> Result<(), Error> {
        let first = self.block_first_lsn(block);
        let size = self.geo.sector_bytes();
        for i in 0..self.geo.sectors_per_block() {
            let from = i * size;
            let chunk = data.get(from..(from + size).min(data.len())).unwrap_or(&[]);
            let mut padded = chunk.to_vec();
            padded.resize(size, FREE);
            self.write_sector(first + i, &padded)?;
        }
        Ok(())
    }

    // ---- directory ----

    fn entries(&self) -> Result<Vec<Entry>, Error> {
        let mut bytes = Vec::new();
        for b in 0..self.geo.dir_blocks as usize {
            bytes.extend(self.read_block(b)?);
        }
        Ok(bytes
            .as_chunks::<ENTRY_SIZE>()
            .0
            .iter()
            .take(self.geo.dir_entries())
            .enumerate()
            .map(|(index, c)| Entry { index, raw: *c })
            .collect())
    }

    fn write_entry(&mut self, index: usize, raw: &[u8; ENTRY_SIZE]) -> Result<(), Error> {
        let per_sector = self.geo.sector_bytes() / ENTRY_SIZE;
        let lsn = self.block_first_lsn(0) + index / per_sector;
        let mut sector = self.sector(lsn)?.to_vec();
        let at = (index % per_sector) * ENTRY_SIZE;
        sector[at..at + ENTRY_SIZE].copy_from_slice(raw);
        self.write_sector(lsn, &sector)
    }

    fn pointers(&self, entry: &Entry) -> Vec<usize> {
        let alloc = &entry.raw[16..32];
        if self.geo.wide_pointers() {
            alloc
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_le_bytes(*p) as usize)
                .filter(|&b| b != 0)
                .collect()
        } else {
            alloc
                .iter()
                .map(|&b| b as usize)
                .filter(|&b| b != 0)
                .collect()
        }
    }

    /// The files, in directory order of their first extent.
    pub fn files(&self) -> Result<Vec<FileInfo>, Error> {
        let mut order: Vec<(u8, [u8; 11])> = Vec::new();
        let mut groups: BTreeMap<(u8, [u8; 11]), Vec<Entry>> = BTreeMap::new();
        for e in self.entries()?.into_iter().filter(Entry::is_file) {
            let key = (e.user(), e.name());
            if !groups.contains_key(&key) {
                order.push(key);
            }
            groups.entry(key).or_default().push(e);
        }
        Ok(order
            .into_iter()
            .map(|key| {
                let mut extents = groups.remove(&key).expect("grouped");
                extents.sort_by_key(Entry::extent);
                let last = extents.last().expect("at least one");
                let records = (last.extent() * RECORD + last.records()) as u32;
                let blocks = extents.iter().map(|e| self.pointers(e).len()).sum();
                let attr = |byte: usize| extents[0].raw[byte] & 0x80 != 0;
                FileInfo {
                    user: key.0,
                    name: display_name(&key.1),
                    raw_name: key.1,
                    records,
                    bytes: records as u64 * RECORD as u64,
                    blocks,
                    extents: extents.len(),
                    read_only: attr(9),
                    system: attr(10),
                    archived: attr(11),
                }
            })
            .collect())
    }

    fn find(&self, user: u8, name: &[u8; 11]) -> Result<Vec<Entry>, Error> {
        let mut found: Vec<Entry> = self
            .entries()?
            .into_iter()
            .filter(|e| e.is_file() && e.user() == user && &e.name() == name)
            .collect();
        found.sort_by_key(Entry::extent);
        Ok(found)
    }

    /// The bytes of a file, `records * 128` of them.
    pub fn read(&self, user: u8, name: &str) -> Result<Vec<u8>, Error> {
        let raw = parse_name(name)?;
        let extents = self.find(user, &raw)?;
        let last = extents.last().ok_or(Error::NotFound)?;
        let records = last.extent() * RECORD + last.records();
        let mut out = Vec::with_capacity(records * RECORD);
        let mut seen = HashSet::new();
        for e in &extents {
            for b in self.pointers(e) {
                if b >= self.geo.blocks() {
                    return Err(Error::Corrupt(format!(
                        "{} uses block {b}, but the disk has {} blocks",
                        display_name(&raw),
                        self.geo.blocks()
                    )));
                }
                if !seen.insert(b) {
                    continue; // a block listed twice in one file
                }
                out.extend(self.read_block(b)?);
            }
        }
        out.truncate(records * RECORD);
        Ok(out)
    }

    fn used_blocks(&self) -> Result<HashSet<usize>, Error> {
        let mut used: HashSet<usize> = (0..self.geo.dir_blocks as usize).collect();
        for e in self.entries()?.iter().filter(|e| e.is_file()) {
            used.extend(self.pointers(e));
        }
        Ok(used)
    }

    pub fn free_blocks(&self) -> usize {
        let used = self.used_blocks().map_or(0, |u| u.len());
        self.geo.blocks().saturating_sub(used)
    }

    /// Stores a file. With `replace` an existing file of that name is deleted first.
    pub fn write(&mut self, user: u8, name: &str, data: &[u8], replace: bool) -> Result<(), Error> {
        if !self.geo.writable() {
            return Err(Error::Unsupported(
                "writing is limited to the standard 1 KB-block layout".to_string(),
            ));
        }
        if user > 15 {
            return Err(Error::InvalidName(format!("user area {user} (0 to 15)")));
        }
        let raw = parse_name(name)?;
        let backup = self.clone();
        let result = self.write_inner(user, &raw, data, replace);
        if result.is_err() {
            *self = backup;
        }
        result
    }

    fn write_inner(
        &mut self,
        user: u8,
        raw: &[u8; 11],
        data: &[u8],
        replace: bool,
    ) -> Result<(), Error> {
        if !self.find(user, raw)?.is_empty() {
            if !replace {
                return Err(Error::Exists);
            }
            self.delete_raw(user, raw)?;
        }
        let block_bytes = self.geo.block_bytes();
        let blocks_needed = data.len().div_ceil(block_bytes);
        let used = self.used_blocks()?;
        let free: Vec<usize> = (0..self.geo.blocks())
            .filter(|b| !used.contains(b))
            .collect();
        if free.len() < blocks_needed {
            return Err(Error::DiskFull);
        }
        let chosen = &free[..blocks_needed];
        let extents = blocks_needed.div_ceil(16).max(1);
        let slots: Vec<usize> = self
            .entries()?
            .into_iter()
            .filter(|e| e.user() == FREE)
            .map(|e| e.index)
            .take(extents)
            .collect();
        if slots.len() < extents {
            return Err(Error::DirectoryFull);
        }
        for (i, &block) in chosen.iter().enumerate() {
            let from = i * block_bytes;
            self.write_block(block, &data[from..(from + block_bytes).min(data.len())])?;
        }
        let records = data.len().div_ceil(RECORD);
        for (i, &slot) in slots.iter().enumerate() {
            let mut entry = [0u8; ENTRY_SIZE];
            entry[0] = user;
            entry[1..12].copy_from_slice(raw);
            entry[12] = (i & 31) as u8;
            entry[14] = (i >> 5) as u8;
            let left = records.saturating_sub(i * RECORD);
            entry[15] = left.min(RECORD) as u8;
            for (k, &b) in chosen.iter().skip(i * 16).take(16).enumerate() {
                entry[16 + k] = b as u8;
            }
            self.write_entry(slot, &entry)?;
        }
        Ok(())
    }

    fn delete_raw(&mut self, user: u8, raw: &[u8; 11]) -> Result<(), Error> {
        let extents = self.find(user, raw)?;
        if extents.is_empty() {
            return Err(Error::NotFound);
        }
        for e in extents {
            let mut r = e.raw;
            r[0] = FREE;
            self.write_entry(e.index, &r)?;
        }
        Ok(())
    }

    /// Deletes a file.
    pub fn delete(&mut self, user: u8, name: &str) -> Result<(), Error> {
        let raw = parse_name(name)?;
        self.delete_raw(user, &raw)
    }

    /// Renames a file (attributes are kept).
    pub fn rename(&mut self, user: u8, old: &str, new: &str) -> Result<(), Error> {
        let old = parse_name(old)?;
        let new = parse_name(new)?;
        if !self.find(user, &new)?.is_empty() {
            return Err(Error::Exists);
        }
        let extents = self.find(user, &old)?;
        if extents.is_empty() {
            return Err(Error::NotFound);
        }
        for e in extents {
            let mut r = e.raw;
            for (i, c) in new.iter().enumerate() {
                r[1 + i] = (r[1 + i] & 0x80) | c;
            }
            self.write_entry(e.index, &r)?;
        }
        Ok(())
    }

    /// Sets the read-only and system attributes of a file.
    pub fn set_attributes(
        &mut self,
        user: u8,
        name: &str,
        read_only: bool,
        system: bool,
    ) -> Result<(), Error> {
        let raw = parse_name(name)?;
        let extents = self.find(user, &raw)?;
        if extents.is_empty() {
            return Err(Error::NotFound);
        }
        for e in extents {
            let mut r = e.raw;
            r[9] = (r[9] & 0x7F) | if read_only { 0x80 } else { 0 };
            r[10] = (r[10] & 0x7F) | if system { 0x80 } else { 0 };
            self.write_entry(e.index, &r)?;
        }
        Ok(())
    }

    /// Looks for damage: blocks outside the disk or used twice, extents out of order or missing,
    /// record counts that need more blocks than listed, odd names.
    pub fn check(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        let mut say = |error: bool, message: String| problems.push(Problem { error, message });
        let entries = match self.entries() {
            Ok(e) => e,
            Err(e) => {
                say(true, e.to_string());
                return problems;
            }
        };
        let mut owner: BTreeMap<usize, String> = BTreeMap::new();
        for b in 0..self.geo.dir_blocks as usize {
            owner.insert(b, "the directory".to_string());
        }
        let mut groups: BTreeMap<(u8, [u8; 11]), Vec<&Entry>> = BTreeMap::new();
        for e in entries.iter().filter(|e| e.is_file()) {
            groups.entry((e.user(), e.name())).or_default().push(e);
        }
        for e in entries.iter().filter(|e| !e.is_file() && e.user() != FREE) {
            say(
                false,
                format!(
                    "directory entry {} has user number {:#04x} (not a file)",
                    e.index,
                    e.user()
                ),
            );
        }
        for ((user, raw), mut extents) in groups {
            let file = format!("{user}:{}", display_name(&raw));
            extents.sort_by_key(|e| e.extent());
            for (i, e) in extents.iter().enumerate() {
                if e.extent() != i && extents.len() > 1 {
                    say(
                        true,
                        format!(
                            "{file}: extent numbers are not consecutive (found {} at position {i})",
                            e.extent()
                        ),
                    );
                    break;
                }
            }
            let mut blocks = 0usize;
            for e in &extents {
                if e.records() > RECORD {
                    say(
                        true,
                        format!(
                            "{file}: extent {} claims {} records",
                            e.extent(),
                            e.records()
                        ),
                    );
                }
                for b in self.pointers(e) {
                    blocks += 1;
                    if b >= self.geo.blocks() {
                        say(
                            true,
                            format!(
                                "{file}: block {b} is outside the disk ({} blocks)",
                                self.geo.blocks()
                            ),
                        );
                    } else if let Some(other) = owner.insert(b, file.clone())
                        && other != file
                    {
                        say(
                            true,
                            format!("{file}: block {b} is also used by {other} (cross-linked)"),
                        );
                    }
                }
            }
            let last = extents.last().expect("non-empty");
            let records = last.extent() * RECORD + last.records();
            let needed = (records * RECORD).div_ceil(self.geo.block_bytes());
            if blocks < needed {
                say(
                    true,
                    format!(
                        "{file}: {records} records need {needed} blocks but only {blocks} are listed"
                    ),
                );
            }
            if raw
                .iter()
                .any(|&c| c < 0x20 || c == 0x7F || b"<>.,;:=?*[]".contains(&c))
            {
                say(
                    false,
                    format!("{file}: the name contains characters CP/M does not allow"),
                );
            }
        }
        problems
    }

    // ---- the +3 disk specification ----

    fn write_plus3_spec(&mut self) {
        let g = &self.geo;
        let mut sector = vec![FREE; g.sector_bytes()];
        sector[..16].copy_from_slice(&[
            0,                   // format: standard single sided
            0,                   // sides: single
            g.tracks,            // tracks per side
            g.sectors_per_track, // sectors per track
            g.sector_size_code,  // sector size code
            g.reserved_tracks,   // reserved tracks
            g.block_shift,       // block shift
            g.dir_blocks,        // directory blocks
            0x2A,                // read/write gap
            0x52,                // format gap
            0,
            0,
            0,
            0,
            0, // reserved
            0, // checksum, set below
        ]);
        // The byte sum of a bootable disk's first sector is 3; this disk has no boot code, so make
        // sure it is not.
        let sum = sector.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        if sum == BOOT_SUM {
            sector[15] = 1;
        }
        let _ = self.write_sector(0, &sector);
    }
}

// ---- names ----

/// `NAME.EXT` for display.
fn display_name(raw: &[u8; 11]) -> String {
    let text = |bytes: &[u8]| -> String {
        let s: String = bytes.iter().map(|&b| (b & 0x7F) as char).collect();
        s.trim_end().to_string()
    };
    let name = text(&raw[..8]);
    let ext = text(&raw[8..]);
    if ext.is_empty() {
        name
    } else {
        format!("{name}.{ext}")
    }
}

/// Splits `name.ext` into the 11 stored characters (upper case, blank padded).
pub fn parse_name(name: &str) -> Result<[u8; 11], Error> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, e),
        None => (name, ""),
    };
    if stem.is_empty() || stem.len() > 8 || ext.len() > 3 {
        return Err(Error::InvalidName(format!(
            "\"{name}\" does not fit 8.3 (1 to 8 characters, a dot, up to 3)"
        )));
    }
    let mut out = [b' '; 11];
    for (i, c) in stem.chars().enumerate() {
        out[i] = check_char(c, name)?;
    }
    for (i, c) in ext.chars().enumerate() {
        out[8 + i] = check_char(c, name)?;
    }
    Ok(out)
}

fn check_char(c: char, whole: &str) -> Result<u8, Error> {
    if !c.is_ascii() || c.is_ascii_control() || c == ' ' || "<>.,;:=?*[]".contains(c) {
        return Err(Error::InvalidName(format!("\"{whole}\" contains \"{c}\"")));
    }
    Ok(c.to_ascii_uppercase() as u8)
}

// ---- recognising a disk ----

fn detect(disk: &Disk) -> Result<Geometry, Error> {
    let count = disk.sector_count(0, 0);
    if count == 0 {
        return Err(Error::NoGeometry("track 0 is not formatted".to_string()));
    }
    let ids: Vec<u8> = (0..count)
        .filter_map(|i| disk.sector_at(0, 0, i))
        .map(|s| s.sector)
        .collect();
    let first = *ids.iter().min().expect("non-empty");
    let size_code = disk.sector_at(0, 0, 0).map(|s| s.size_code).unwrap_or(2);
    // Sector IDs must be consecutive from the first.
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    if sorted
        .iter()
        .enumerate()
        .any(|(i, &id)| id as usize != first as usize + i)
    {
        return Err(Error::NoGeometry(format!(
            "the sector IDs of track 0 are not consecutive ({ids:02X?})"
        )));
    }
    let formatted_tracks = (0..disk.cylinders())
        .take_while(|&c| disk.sector_count(c as u8, 0) > 0)
        .count();
    let base = |layout: Layout| {
        let mut g = layout.geometry();
        g.tracks = (formatted_tracks.clamp(1, 255)).min(g.tracks as usize) as u8;
        g
    };
    if first == 0x01
        && let Some(g) = plus3_spec(disk, first, count, size_code)
    {
        return Ok(g);
    }
    match (first, count, size_code) {
        (0xC1, 9, 2) => Ok(base(Layout::CpcData)),
        (0x41, 9, 2) => Ok(base(Layout::CpcSystem)),
        (0x01, 9, 2) => Ok(base(Layout::Plus3)),
        (0x01, 8, 2) => Ok(base(Layout::CpcIbm)),
        _ => Err(Error::NoGeometry(format!(
            "track 0 has {count} sectors of {} bytes starting at ID {first:#04x}",
            sector_size(size_code)
        ))),
    }
}

fn plus3_spec(disk: &Disk, first: u8, count: usize, size_code: u8) -> Option<Geometry> {
    let s = disk.read_sector(0, 0, first)?;
    let d = &s.data;
    if d.len() < 16 {
        return None;
    }
    let (kind, tracks, spt, n, off, bsh, dir) = (d[0], d[2], d[3], d[4], d[5], d[6], d[7]);
    let sane = kind <= 3
        && (1..=4).contains(&(d[1] & 3).max(1))
        && (35..=84).contains(&tracks)
        && spt as usize == count
        && n == size_code
        && off <= 4
        && (3..=5).contains(&bsh)
        && (1..=8).contains(&dir);
    sane.then_some(Geometry {
        tracks,
        sectors_per_track: spt,
        sector_size_code: n,
        first_sector: first,
        reserved_tracks: off,
        block_shift: bsh,
        dir_blocks: dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    #[test]
    fn blank_disks_have_the_documented_capacity() {
        // 40 tracks x 9 sectors x 512 bytes = 180 blocks of 1 KB; the data format has no reserved tracks.
        let data_disk = Cpm::format(Layout::CpcData);
        assert_eq!(data_disk.geometry().blocks(), 180);
        assert_eq!(data_disk.free_blocks(), 178); // 2 directory blocks
        assert_eq!(data_disk.geometry().dir_entries(), 64);
        // The system format loses 2 tracks (9 blocks) to the boot area, the +3 and IBM formats one track.
        assert_eq!(Cpm::format(Layout::CpcSystem).free_blocks(), 171 - 2);
        assert_eq!(Cpm::format(Layout::Plus3).geometry().blocks(), 175);
        assert_eq!(Cpm::format(Layout::Plus3).free_blocks(), 175 - 2);
        assert_eq!(Cpm::format(Layout::CpcIbm).geometry().blocks(), 156);
        assert!(Cpm::format(Layout::CpcData).files().unwrap().is_empty());
    }

    #[test]
    fn files_round_trip_in_every_layout() {
        for layout in [
            Layout::CpcData,
            Layout::CpcSystem,
            Layout::CpcIbm,
            Layout::Plus3,
        ] {
            let mut cpm = Cpm::format(layout);
            let free = cpm.free_blocks();
            let small = data(300, 1);
            let big = data(40_000, 2); // three extents
            let empty: Vec<u8> = Vec::new();
            cpm.write(0, "small.txt", &small, false).unwrap();
            cpm.write(0, "BIG.BIN", &big, false).unwrap();
            cpm.write(3, "other.dat", &data(5, 3), false).unwrap();
            cpm.write(0, "empty", &empty, false).unwrap();

            let files = cpm.files().unwrap();
            let names: Vec<_> = files.iter().map(|f| (f.user, f.name.as_str())).collect();
            assert_eq!(
                names,
                [
                    (0, "SMALL.TXT"),
                    (0, "BIG.BIN"),
                    (3, "OTHER.DAT"),
                    (0, "EMPTY")
                ]
            );
            assert_eq!(files[1].extents, 3);
            assert_eq!(files[1].blocks, 40_000_usize.div_ceil(1024));
            assert_eq!(files[0].bytes, 384); // 3 records
            assert_eq!(files[3].bytes, 0);

            // Reading gives the data padded to whole records.
            let back = cpm.read(0, "small.txt").unwrap();
            assert_eq!(&back[..300], &small[..]);
            assert_eq!(back.len(), 384);
            let back = cpm.read(0, "BIG.BIN").unwrap();
            assert_eq!(&back[..40_000], &big[..]);
            assert_eq!(back.len(), 40_064);
            assert_eq!(cpm.read(0, "empty").unwrap(), empty);
            assert_eq!(cpm.read(3, "other.dat").unwrap().len(), 128);
            assert_eq!(cpm.read(1, "small.txt"), Err(Error::NotFound));

            assert_eq!(cpm.free_blocks(), free - 1 - 40 - 1);
            assert!(cpm.check().is_empty(), "{layout:?}: {:?}", cpm.check());

            // And it survives being written out as a .dsk and read back.
            let bytes = cpm.disk().to_bytes();
            let reopened = Cpm::open(crate::parse(&bytes).unwrap()).unwrap();
            assert_eq!(reopened.geometry(), cpm.geometry());
            assert_eq!(reopened.files().unwrap(), files);
            assert_eq!(
                reopened.read(0, "BIG.BIN").unwrap(),
                cpm.read(0, "BIG.BIN").unwrap()
            );
        }
    }

    #[test]
    fn delete_rename_replace_and_attributes() {
        let mut cpm = Cpm::format(Layout::CpcData);
        cpm.write(0, "a.bin", &data(2000, 1), false).unwrap();
        cpm.write(0, "b.bin", &data(10, 2), false).unwrap();
        assert_eq!(cpm.write(0, "a.bin", &[1], false), Err(Error::Exists));
        cpm.write(0, "a.bin", &data(50, 9), true).unwrap();
        assert_eq!(cpm.read(0, "a.bin").unwrap()[..50], data(50, 9)[..]);
        cpm.rename(0, "b.bin", "c.dat").unwrap();
        assert_eq!(cpm.rename(0, "c.dat", "a.bin"), Err(Error::Exists));
        assert_eq!(cpm.rename(0, "zzz", "yyy"), Err(Error::NotFound));
        cpm.set_attributes(0, "c.dat", true, true).unwrap();
        let f = cpm.files().unwrap();
        assert!(
            f.iter()
                .any(|f| f.name == "C.DAT" && f.read_only && f.system)
        );
        let before = cpm.free_blocks();
        cpm.delete(0, "a.bin").unwrap();
        assert_eq!(cpm.free_blocks(), before + 1);
        assert_eq!(cpm.delete(0, "a.bin"), Err(Error::NotFound));
        assert!(cpm.check().is_empty(), "{:?}", cpm.check());
    }

    #[test]
    fn full_disk_and_full_directory_leave_the_disk_unchanged() {
        let mut cpm = Cpm::format(Layout::CpcData);
        let snapshot = cpm.disk().to_bytes();
        assert_eq!(
            cpm.write(0, "huge.bin", &data(200 * 1024, 1), false),
            Err(Error::DiskFull)
        );
        assert_eq!(cpm.disk().to_bytes(), snapshot);
        for i in 0..64 {
            cpm.write(0, &format!("f{i}.dat"), &[i as u8], false)
                .unwrap();
        }
        assert_eq!(
            cpm.write(0, "more.dat", &[1], false),
            Err(Error::DirectoryFull)
        );
        assert_eq!(cpm.files().unwrap().len(), 64);
        assert!(cpm.check().is_empty());
    }

    #[test]
    fn names_are_eight_dot_three() {
        assert_eq!(&parse_name("file.bas").unwrap(), b"FILE    BAS");
        assert_eq!(&parse_name("noext").unwrap(), b"NOEXT      ");
        assert!(parse_name("toolongname.bin").is_err());
        assert!(parse_name("a.long").is_err());
        assert!(parse_name("").is_err());
        assert!(parse_name("we?rd.bin").is_err());
        assert!(parse_name("sp ace.bin").is_err());
        assert!(parse_name(".bin").is_err());
    }

    #[test]
    fn check_finds_a_cross_linked_block() {
        let mut cpm = Cpm::format(Layout::CpcData);
        cpm.write(0, "a.dat", &data(10, 1), false).unwrap();
        cpm.write(0, "b.dat", &data(10, 2), false).unwrap();
        // Point b at a's block.
        let entries = cpm.entries().unwrap();
        let a = entries
            .iter()
            .find(|e| e.is_file() && e.name() == *b"A       DAT")
            .unwrap();
        let b = entries
            .iter()
            .find(|e| e.is_file() && e.name() == *b"B       DAT")
            .unwrap();
        let mut raw = b.raw;
        raw[16] = a.raw[16];
        cpm.write_entry(b.index, &raw).unwrap();
        let problems = cpm.check();
        assert!(
            problems
                .iter()
                .any(|p| p.error && p.message.contains("cross-linked")),
            "{problems:?}"
        );
    }

    #[test]
    fn unknown_disks_are_refused() {
        let mut disk = Disk::new(1);
        let ids: Vec<[u8; 4]> = (0..5).map(|i| [0, 0, 0x20 + i, 2]).collect();
        disk.format_track(0, 0, &ids, 0x4E, 0xE5);
        assert!(matches!(Cpm::open(disk), Err(Error::NoGeometry(_))));
        assert!(matches!(Cpm::open(Disk::new(1)), Err(Error::NoGeometry(_))));
    }

    #[test]
    fn interleaved_sector_order_on_the_track_does_not_matter() {
        // Format every track with the CPC's usual interleave (IDs in rotational order).
        let order = [0xC1u8, 0xC6, 0xC2, 0xC7, 0xC3, 0xC8, 0xC4, 0xC9, 0xC5];
        let mut disk = Disk::new(1);
        for cyl in 0..40u8 {
            let ids: Vec<[u8; 4]> = order.iter().map(|&id| [cyl, 0, id, 2]).collect();
            disk.format_track(cyl, 0, &ids, 0x4E, 0xE5);
        }
        let mut cpm = Cpm::open(disk).unwrap();
        cpm.write(0, "x.bin", &data(9000, 4), false).unwrap();
        assert_eq!(cpm.read(0, "x.bin").unwrap()[..9000], data(9000, 4)[..]);
        assert!(cpm.check().is_empty());
    }

    #[test]
    fn a_plus3_disk_specification_selects_the_geometry() {
        // 80 tracks, 9 sectors, 1 reserved track, 2 KB blocks, 4 directory blocks.
        let mut disk = Disk::new(1);
        for cyl in 0..80u8 {
            let ids: Vec<[u8; 4]> = (1..=9).map(|id| [cyl, 0, id, 2]).collect();
            disk.format_track(cyl, 0, &ids, 0x4E, 0xE5);
        }
        let mut sector = vec![0xE5u8; 512];
        sector[..16].copy_from_slice(&[3, 1, 80, 9, 2, 1, 4, 4, 0x2A, 0x52, 0, 0, 0, 0, 0, 0]);
        let idx = 0;
        disk.write_sector_at(0, 0, idx, &sector, false);
        let cpm = Cpm::open(disk).unwrap();
        let g = cpm.geometry();
        assert_eq!(
            (g.tracks, g.block_shift, g.dir_blocks, g.reserved_tracks),
            (80, 4, 4, 1)
        );
        assert_eq!(g.block_bytes(), 2048);
        assert_eq!(g.dir_entries(), 256);
        // Reading works on such a disk; writing is only for the standard layout.
        assert!(cpm.files().unwrap().is_empty());
        let mut cpm = cpm;
        assert!(matches!(
            cpm.write(0, "a.b", &[1], false),
            Err(Error::Unsupported(_))
        ));
    }
}
