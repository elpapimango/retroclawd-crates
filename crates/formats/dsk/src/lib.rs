//! Parses a CPCEMU `.dsk` disk image -- the format the ZX Spectrum +3 (and
//! the Amstrad CPC, which originated it) both use -- in either its
//! standard or "Extended DSK" variant. No dependency on `machine`, any CPU
//! crate, or `nec765` -- same "format crate only parses bytes into a plain
//! struct" precedent as `z80-snapshot`/`zx81-snapshot`/`zx-tape`. Byte
//! layout verified against cpctech.cpcwiki.de's own DSK/Extended-DSK
//! format references (see `CREDITS.md`), not recollection.
//!
//! Both variants parse into the exact same `Disk`/`Sector` shape -- the
//! difference is entirely about *where the bytes live in the file*
//! (`track_size`: one fixed value for every track vs. Extended DSK's own
//! per-track size table, since it supports variable-size tracks for
//! copy-protected/weak-sector software), never about what a consumer
//! (`nec765`, `zx_spectrum`) sees afterward -- so nothing downstream of
//! `parse` needed to change to gain Extended DSK support. Detected by
//! content (the disc-level signature), not file extension -- real
//! Extended DSK files are routinely still named `.dsk`, not `.edsk`.
//! Weak-sector copies are preserved (`Sector::copies`) and `nec765::Fdc`
//! rotates through them on successive reads; see `parse_track`'s own doc
//! comment for how a weak sector's extra copies are handled.
//!
//! Images are also *writable*: `write_sector_at`/`format_track` mutate the
//! in-memory model (setting `is_modified`), and `to_bytes` serialises it
//! back to a standard or Extended DSK file that `parse` reads back to an
//! equivalent disk -- the persistence half of the CPC/+3 FDC write path.

pub mod cpm;

const DISC_SIGNATURE: &[u8] = b"MV - CPCEMU Disk-File\r\nDisk-Info\r\n";
const EXTENDED_DISC_SIGNATURE: &[u8] = b"EXTENDED CPC DSK File\r\nDisk-Info\r\n";
const TRACK_SIGNATURE: &[u8] = b"Track-Info\r\n";
const BLOCK_SIZE: usize = 0x100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TooShort,
    NotAStandardDskFile,
    TruncatedTrack(usize),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TooShort => write!(f, "file shorter than the 256-byte Disc Information block"),
            Error::NotAStandardDskFile => write!(
                f,
                "not a CPCEMU .dsk file (missing the standard or Extended DSK signature)"
            ),
            Error::TruncatedTrack(index) => {
                write!(f, "track {index} truncated -- file ends mid-track")
            }
        }
    }
}

impl std::error::Error for Error {}

/// One sector's real ID fields plus its raw data. `size_code` is the
/// NEC765 N encoding (`128 << size_code` bytes) -- the file's own sector-
/// info byte, unconverted (see `nec765::SectorId`'s doc comment for why
/// the two crates share this exact encoding without knowing about each
/// other).
#[derive(Debug, Clone)]
pub struct Sector {
    pub cylinder: u8,
    pub head: u8,
    pub sector: u8,
    pub size_code: u8,
    /// Stored FDC status register 1/2 flags for this sector (sector-info
    /// list bytes 4/5). Extended DSK writers record e.g. a data-error or a
    /// deleted-data address mark here (`st2 & 0x40`); usually `0`.
    pub st1: u8,
    pub st2: u8,
    /// The first stored copy -- for a normal (non-weak) sector, the
    /// only one. Kept for backward compatibility with any consumer that
    /// only wants one copy.
    pub data: Vec<u8>,
    /// Every copy Extended DSK's own "actual data length" override
    /// stored for this sector, in file order (see `parse_track`'s own
    /// doc comment) -- a normal sector always has exactly one entry,
    /// identical to `data`. A real weak/copy-protected sector stores
    /// several different copies back-to-back for a real drive
    /// controller to return a different one on each successive read;
    /// `nec765::Fdc` is what actually cycles through these (see its own
    /// `weak_read_counter` doc comment).
    pub copies: Vec<Vec<u8>>,
}

/// One track. `formatted == false` means the file carried no Track
/// Information block for it (Extended DSK size-table entry `0`); such a
/// track has no sectors and is written back the same way.
#[derive(Debug, Clone, Default)]
struct Track {
    sectors: Vec<Sector>,
    formatted: bool,
    /// Track Information block header fields, preserved so `to_bytes`
    /// reproduces what `parse` read (track/side numbers, data rate,
    /// recording mode, sector-size N, GAP#3, filler byte).
    track_number: u8,
    side_number: u8,
    data_rate: u8,
    recording_mode: u8,
    size_code: u8,
    gap3: u8,
    filler: u8,
}

/// A parsed CPCEMU `.dsk` image (standard or Extended).
#[derive(Debug, Clone)]
pub struct Disk {
    tracks: Vec<Track>,
    heads: u8,
    extended: bool,
    /// Disc Information block bytes 0x22..0x30 (the "creator" string),
    /// preserved for lossless write-back.
    creator: [u8; 14],
    modified: bool,
}

/// Highest cylinder count `format_track` will grow an image to -- generous
/// for real 3"/3.5"/5.25" media (up to 84 physical tracks), and bounds a
/// hostile Format command's allocation.
const MAX_CYLINDERS: usize = 84;

/// `128 << n`, with `n` clamped so a malformed size code can't overflow the
/// shift (real N values above 8 have no defined meaning).
pub fn sector_size(size_code: u8) -> usize {
    128usize << size_code.min(8)
}

impl Disk {
    /// An empty standard-DSK image with `heads` sides and no formatted
    /// tracks: format them with `format_track` (a real drive's Format
    /// command does exactly that). `heads` is clamped to 1 or 2.
    pub fn new(heads: u8) -> Self {
        let mut creator = [0u8; 14];
        creator[..10].copy_from_slice(b"retroclawd");
        Self {
            tracks: Vec::new(),
            heads: heads.clamp(1, 2),
            extended: false,
            creator,
            modified: false,
        }
    }

    /// Number of sides (heads) the image stores per cylinder.
    pub fn heads(&self) -> u8 {
        self.heads
    }

    /// Number of cylinders present in the image.
    pub fn cylinders(&self) -> usize {
        if self.heads == 0 {
            0
        } else {
            self.tracks.len() / self.heads as usize
        }
    }

    /// Whether the image was parsed from / will serialise as Extended DSK.
    pub fn is_extended(&self) -> bool {
        self.extended
    }

    /// True once `write_sector_at`/`format_track` has changed the image
    /// since it was parsed (or since `clear_modified`).
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    pub fn clear_modified(&mut self) {
        self.modified = false;
    }

    /// Flags the image as modified without changing it -- for restoring a
    /// save state that captured an already-modified disk.
    pub fn mark_modified(&mut self) {
        self.modified = true;
    }

    /// The first sector this disk has for `(cylinder, head)`, in the
    /// file's own sector-info table order.
    pub fn first_sector(&self, cylinder: u8, head: u8) -> Option<&Sector> {
        self.track(cylinder, head)?.sectors.first()
    }

    /// One specific sector by its real ID fields.
    pub fn read_sector(&self, cylinder: u8, head: u8, sector: u8) -> Option<&Sector> {
        self.track(cylinder, head)?
            .sectors
            .iter()
            .find(|s| s.sector == sector)
    }

    /// How many sectors `(cylinder, head)` carries (`0` for an
    /// unformatted or absent track).
    pub fn sector_count(&self, cylinder: u8, head: u8) -> usize {
        self.track(cylinder, head).map_or(0, |t| t.sectors.len())
    }

    /// The `index`th sector of `(cylinder, head)` in rotational (file)
    /// order -- what a real drive sees passing under the head.
    pub fn sector_at(&self, cylinder: u8, head: u8, index: usize) -> Option<&Sector> {
        self.track(cylinder, head)?.sectors.get(index)
    }

    /// Overwrites the `index`th sector's data with `data` (truncated or
    /// zero-extended to the sector's size), collapsing any weak-sector
    /// copies to that single new copy. `deleted` records a deleted-data
    /// address mark (`st2` bit 6); a normal write clears it. ST1 is
    /// cleared either way, like a real successful write. Returns `false`
    /// if no such sector exists.
    pub fn write_sector_at(
        &mut self,
        cylinder: u8,
        head: u8,
        index: usize,
        data: &[u8],
        deleted: bool,
    ) -> bool {
        let Some(track) = self.track_mut(cylinder, head) else {
            return false;
        };
        let Some(sector) = track.sectors.get_mut(index) else {
            return false;
        };
        let size = sector_size(sector.size_code);
        let mut bytes = data.to_vec();
        bytes.resize(size, 0);
        sector.data = bytes.clone();
        sector.copies = vec![bytes];
        sector.st1 = 0;
        sector.st2 = if deleted { 0x40 } else { 0 };
        self.modified = true;
        true
    }

    /// Replaces `(cylinder, head)` with a freshly formatted track: one
    /// sector per `ids` entry (`[C, H, R, N]`, in rotational order), each
    /// filled with `filler`. Grows the image with unformatted tracks if
    /// `cylinder` is past the current end (bounded by `MAX_CYLINDERS`).
    /// Returns `false` if `head` isn't a side this image stores or
    /// `cylinder` is out of range.
    pub fn format_track(
        &mut self,
        cylinder: u8,
        head: u8,
        ids: &[[u8; 4]],
        gap3: u8,
        filler: u8,
    ) -> bool {
        if head >= self.heads || cylinder as usize >= MAX_CYLINDERS {
            return false;
        }
        let needed = (cylinder as usize + 1) * self.heads as usize;
        while self.tracks.len() < needed {
            let n = self.tracks.len();
            self.tracks.push(Track {
                track_number: (n / self.heads as usize) as u8,
                side_number: (n % self.heads as usize) as u8,
                ..Track::default()
            });
        }
        let size_code = ids.first().map_or(2, |id| id[3]);
        let sectors = ids
            .iter()
            .map(|id| {
                let bytes = vec![filler; sector_size(id[3])];
                Sector {
                    cylinder: id[0],
                    head: id[1],
                    sector: id[2],
                    size_code: id[3],
                    st1: 0,
                    st2: 0,
                    data: bytes.clone(),
                    copies: vec![bytes],
                }
            })
            .collect();
        let index = cylinder as usize * self.heads as usize + head as usize;
        self.tracks[index] = Track {
            sectors,
            formatted: true,
            track_number: cylinder,
            side_number: head,
            data_rate: 0,
            recording_mode: 0,
            size_code,
            gap3,
            filler,
        };
        self.modified = true;
        true
    }

    fn track(&self, cylinder: u8, head: u8) -> Option<&Track> {
        if head >= self.heads {
            return None;
        }
        let index = cylinder as usize * self.heads as usize + head as usize;
        self.tracks.get(index)
    }

    fn track_mut(&mut self, cylinder: u8, head: u8) -> Option<&mut Track> {
        if head >= self.heads {
            return None;
        }
        let index = cylinder as usize * self.heads as usize + head as usize;
        self.tracks.get_mut(index)
    }

    /// Serialises the image back to a `.dsk` file. Extended DSK is used
    /// when the image was parsed as Extended, or when the model can't be
    /// represented as standard DSK (weak-sector copies, or sector data
    /// whose length differs from its N size code); otherwise standard DSK
    /// with every track padded to the largest track's size.
    pub fn to_bytes(&self) -> Vec<u8> {
        let needs_extended = self.extended
            || self.tracks.iter().any(|t| {
                t.sectors
                    .iter()
                    .any(|s| s.copies.len() != 1 || s.data.len() != sector_size(s.size_code))
            });

        let track_blocks: Vec<Vec<u8>> = self
            .tracks
            .iter()
            .map(|t| encode_track(t, needs_extended))
            .collect();
        let max_track_len = track_blocks
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(BLOCK_SIZE);

        let mut out = vec![0u8; BLOCK_SIZE];
        let sig = if needs_extended {
            EXTENDED_DISC_SIGNATURE
        } else {
            DISC_SIGNATURE
        };
        out[..sig.len()].copy_from_slice(sig);
        out[0x22..0x30].copy_from_slice(&self.creator);
        out[0x30] = self.cylinders().min(255) as u8;
        out[0x31] = self.heads;
        if needs_extended {
            for (i, block) in track_blocks.iter().enumerate() {
                let entry = if block.is_empty() {
                    0
                } else {
                    block.len().div_ceil(256).min(255) as u8
                };
                if let Some(slot) = out.get_mut(0x34 + i) {
                    *slot = entry;
                }
            }
            for block in &track_blocks {
                out.extend_from_slice(block);
                // Extended track sizes are recorded in 256-byte units.
                if !block.is_empty() {
                    out.resize(out.len().div_ceil(256) * 256, 0);
                }
            }
        } else {
            let track_size = max_track_len.div_ceil(256) * 256;
            out[0x32..0x34].copy_from_slice(&(track_size as u16).to_le_bytes());
            for block in &track_blocks {
                let start = out.len();
                out.extend_from_slice(block);
                out.resize(start + track_size, 0);
            }
        }
        out
    }
}

/// Encodes one track (Track Information block + sector data). An
/// unformatted track in Extended DSK is empty (absent from the file); in
/// standard DSK it is a header-only block with zero sectors.
fn encode_track(track: &Track, extended: bool) -> Vec<u8> {
    if !track.formatted && track.sectors.is_empty() && extended {
        return Vec::new();
    }
    let mut block = vec![0u8; BLOCK_SIZE];
    block[..TRACK_SIGNATURE.len()].copy_from_slice(TRACK_SIGNATURE);
    block[0x10] = track.track_number;
    block[0x11] = track.side_number;
    block[0x12] = track.data_rate;
    block[0x13] = track.recording_mode;
    block[0x14] = track.size_code;
    block[0x15] = track.sectors.len().min(29) as u8;
    block[0x16] = track.gap3;
    block[0x17] = track.filler;
    let mut data = Vec::new();
    for (i, s) in track.sectors.iter().take(29).enumerate() {
        let entry = 0x18 + i * 8;
        block[entry] = s.cylinder;
        block[entry + 1] = s.head;
        block[entry + 2] = s.sector;
        block[entry + 3] = s.size_code;
        block[entry + 4] = s.st1;
        block[entry + 5] = s.st2;
        let stored: usize = s.copies.iter().map(Vec::len).sum();
        if extended {
            block[entry + 6..entry + 8].copy_from_slice(&(stored as u16).to_le_bytes());
        }
        for copy in &s.copies {
            data.extend_from_slice(copy);
        }
    }
    block.extend_from_slice(&data);
    block
}

/// Parses one Track Information block (starting at `offset`, which must
/// point at the `Track-Info\r\n` signature) plus its sector data, shared
/// verbatim between the standard and Extended DSK variants -- their
/// Track Information block and Sector Information List layouts are
/// byte-identical; only how each track's own overall size is determined
/// differs (see `parse`).
///
/// Extended DSK's Sector Information List entry adds an "actual data
/// length" override (bytes 6-7, little-endian) for weak/copy-protected
/// sectors whose stored data differs from what `size_code` alone implies
/// -- most commonly *larger*, storing multiple back-to-back copies for a
/// real drive controller to return a different one each read. When present
/// and non-zero, the stored bytes are split into `size_code`-sized copies
/// (`Sector::copies`; `Sector::data` is the first) and the file cursor
/// advances by the override's full length; `0` means "no override" (the
/// standard format never sets this field).
fn parse_track(bytes: &[u8], offset: usize, index: usize) -> Result<Track, Error> {
    let block = bytes
        .get(offset..offset + BLOCK_SIZE)
        .ok_or(Error::TruncatedTrack(index))?;
    if &block[..TRACK_SIGNATURE.len()] != TRACK_SIGNATURE {
        return Err(Error::TruncatedTrack(index));
    }
    let sector_count = block[0x15] as usize;
    if 0x18 + sector_count * 8 > BLOCK_SIZE {
        return Err(Error::TruncatedTrack(index));
    }

    let mut sectors = Vec::with_capacity(sector_count);
    let mut data_offset = offset + BLOCK_SIZE;
    for s in 0..sector_count {
        let entry = &block[0x18 + s * 8..0x18 + s * 8 + 8];
        let size_code = entry[3];
        let size = sector_size(size_code);
        let actual_length = u16::from_le_bytes([entry[6], entry[7]]) as usize;
        let stored_length = if actual_length > 0 {
            actual_length
        } else {
            size
        };

        let stored = bytes
            .get(data_offset..data_offset + stored_length)
            .ok_or(Error::TruncatedTrack(index))?;
        let mut copies: Vec<Vec<u8>> = stored
            .chunks(size)
            .map(|chunk| {
                let mut copy = chunk.to_vec();
                copy.resize(size, 0); // pad a malformed short trailing chunk
                copy
            })
            .collect();
        if copies.is_empty() {
            copies.push(vec![0; size]);
        }
        let data = copies[0].clone();
        sectors.push(Sector {
            cylinder: entry[0],
            head: entry[1],
            sector: entry[2],
            size_code,
            st1: entry[4],
            st2: entry[5],
            data,
            copies,
        });
        data_offset += stored_length;
    }

    Ok(Track {
        sectors,
        formatted: true,
        track_number: block[0x10],
        side_number: block[0x11],
        data_rate: block[0x12],
        recording_mode: block[0x13],
        size_code: block[0x14],
        gap3: block[0x16],
        filler: block[0x17],
    })
}

pub fn parse(bytes: &[u8]) -> Result<Disk, Error> {
    if bytes.len() < BLOCK_SIZE {
        return Err(Error::TooShort);
    }

    let track_count = bytes[0x30] as usize;
    let head_count = bytes[0x31];
    let total_tracks = track_count * head_count as usize;

    // Each present track's own size in the file (including its own
    // 0x100-byte Track Information block) -- either one fixed value
    // shared by every track (standard DSK) or a per-track table
    // (Extended DSK, `0` meaning "unformatted, not present in the file at
    // all" -- see the module doc comment).
    let extended;
    let track_sizes: Vec<usize> = if bytes.len() >= DISC_SIGNATURE.len()
        && &bytes[..DISC_SIGNATURE.len()] == DISC_SIGNATURE
    {
        extended = false;
        // Includes the Track Information block itself, per the format
        // reference -- the next track starts exactly `track_size` bytes
        // after this one, not `BLOCK_SIZE` + a separately-tracked data
        // length.
        let track_size = u16::from_le_bytes([bytes[0x32], bytes[0x33]]) as usize;
        vec![track_size; total_tracks]
    } else if bytes.len() >= EXTENDED_DISC_SIGNATURE.len()
        && &bytes[..EXTENDED_DISC_SIGNATURE.len()] == EXTENDED_DISC_SIGNATURE
    {
        extended = true;
        (0..total_tracks)
            .map(|i| bytes.get(0x34 + i).copied().unwrap_or(0) as usize * 256)
            .collect()
    } else {
        return Err(Error::NotAStandardDskFile);
    };

    let mut tracks = Vec::with_capacity(total_tracks);
    let mut offset = BLOCK_SIZE;
    for (index, &size) in track_sizes.iter().enumerate() {
        if size == 0 {
            // Extended DSK only: unformatted track, no Track Information
            // block present in the file at all -- contributes nothing to
            // `offset`.
            tracks.push(Track {
                track_number: (index / head_count.max(1) as usize) as u8,
                side_number: (index % head_count.max(1) as usize) as u8,
                ..Track::default()
            });
            continue;
        }
        tracks.push(parse_track(bytes, offset, index)?);
        offset += size;
    }

    let mut creator = [0u8; 14];
    creator.copy_from_slice(&bytes[0x22..0x30]);
    Ok(Disk {
        tracks,
        heads: head_count,
        extended,
        creator,
        modified: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal_dsk() -> Vec<u8> {
        let mut bytes = vec![0u8; BLOCK_SIZE]; // Disc Information block
        bytes[..DISC_SIGNATURE.len()].copy_from_slice(DISC_SIGNATURE);
        bytes[0x30] = 1; // 1 track
        bytes[0x31] = 1; // 1 side
        let track_size = (BLOCK_SIZE + 512) as u16; // header + one 512-byte sector
        bytes[0x32..0x34].copy_from_slice(&track_size.to_le_bytes());

        let mut track = vec![0u8; BLOCK_SIZE];
        track[..TRACK_SIGNATURE.len()].copy_from_slice(TRACK_SIGNATURE);
        track[0x10] = 0; // cylinder
        track[0x11] = 0; // head
        track[0x14] = 2; // sector size N=2 (512 bytes)
        track[0x15] = 1; // 1 sector
        track[0x18] = 0; // sector info entry: C
        track[0x19] = 0; // H
        track[0x1A] = 1; // R
        track[0x1B] = 2; // N
        bytes.extend(track);

        let mut sector_data = vec![0xAAu8; 512];
        sector_data[0] = 0x11;
        bytes.extend(sector_data);

        bytes
    }

    #[test]
    fn parses_a_minimal_single_sector_disk() {
        let disk = parse(&build_minimal_dsk()).unwrap();

        let sector = disk.read_sector(0, 0, 1).unwrap();
        assert_eq!(sector.size_code, 2);
        assert_eq!(sector.data.len(), 512);
        assert_eq!(sector.data[0], 0x11);
        assert_eq!(sector.data[1], 0xAA);
    }

    #[test]
    fn first_sector_matches_read_sector_for_a_single_sector_track() {
        let disk = parse(&build_minimal_dsk()).unwrap();
        let first = disk.first_sector(0, 0).unwrap();
        assert_eq!(first.sector, 1);
    }

    #[test]
    fn missing_sector_or_track_is_none_not_a_panic() {
        let disk = parse(&build_minimal_dsk()).unwrap();
        assert!(disk.read_sector(0, 0, 2).is_none()); // no sector 2
        assert!(disk.read_sector(9, 0, 1).is_none()); // no track 9
    }

    fn rejects_a_file_with_the_wrong_signature() {
        let bytes = vec![0u8; BLOCK_SIZE];
        assert_eq!(parse(&bytes).unwrap_err(), Error::NotAStandardDskFile);
    }

    fn rejects_a_truncated_file() {
        let mut bytes = build_minimal_dsk();
        bytes.truncate(bytes.len() - 100); // cut off the end of the sector data
        assert!(matches!(parse(&bytes), Err(Error::TruncatedTrack(0))));
    }
    /// Merged (every check kept): `rejects_a_file_with_the_wrong_signature`, `rejects_a_truncated_file`.
    #[test]
    fn rejects_bad_files() {
        rejects_a_file_with_the_wrong_signature();
        rejects_a_truncated_file();
    }

    /// One present track (256-byte sector) followed by one unformatted
    /// track (size-table entry 0) -- exercises the per-track size table,
    /// the variable overall track size, and the "unformatted, not present
    /// in the file at all" case in one minimal image.
    fn build_minimal_extended_dsk() -> Vec<u8> {
        let mut bytes = vec![0u8; BLOCK_SIZE]; // Disc Information block
        bytes[..EXTENDED_DISC_SIGNATURE.len()].copy_from_slice(EXTENDED_DISC_SIGNATURE);
        bytes[0x30] = 2; // 2 tracks
        bytes[0x31] = 1; // 1 side
        let track0_size = BLOCK_SIZE + 256; // header + one 256-byte sector
        bytes[0x34] = (track0_size / 256) as u8; // track 0's size-table entry
        bytes[0x35] = 0; // track 1: unformatted

        let mut track = vec![0u8; BLOCK_SIZE];
        track[..TRACK_SIGNATURE.len()].copy_from_slice(TRACK_SIGNATURE);
        track[0x10] = 0; // cylinder
        track[0x11] = 0; // head
        track[0x14] = 1; // sector size N=1 (256 bytes)
        track[0x15] = 1; // 1 sector
        track[0x18] = 0; // C
        track[0x19] = 0; // H
        track[0x1A] = 1; // R
        track[0x1B] = 1; // N
        // bytes 6-7 (actual data length override) left as 0 -- "no override".
        bytes.extend(track);

        let mut sector_data = vec![0xBBu8; 256];
        sector_data[0] = 0x22;
        bytes.extend(sector_data);

        bytes
    }

    #[test]
    fn parses_an_extended_dsk_file() {
        let disk = parse(&build_minimal_extended_dsk()).unwrap();

        let sector = disk.read_sector(0, 0, 1).unwrap();
        assert_eq!(sector.size_code, 1);
        assert_eq!(sector.data.len(), 256);
        assert_eq!(sector.data[0], 0x22);
    }

    #[test]
    fn extended_dsk_unformatted_track_has_no_sectors() {
        let disk = parse(&build_minimal_extended_dsk()).unwrap();

        assert!(disk.read_sector(1, 0, 1).is_none());
        assert!(disk.first_sector(1, 0).is_none());
    }

    #[test]
    fn extended_dsk_actual_length_override_preserves_every_real_copy() {
        // A weak/copy-protected sector storing 2 back-to-back 256-byte
        // copies (actual length 512) -- `Sector::data` still reports the
        // first copy (what a single real read returns by default), but
        // `Sector::copies` now preserves both, and the file cursor still
        // advances the full 512 bytes so nothing after it gets misaligned.
        let mut bytes = vec![0u8; BLOCK_SIZE];
        bytes[..EXTENDED_DISC_SIGNATURE.len()].copy_from_slice(EXTENDED_DISC_SIGNATURE);
        bytes[0x30] = 1;
        bytes[0x31] = 1;
        let track_size = BLOCK_SIZE + 512;
        bytes[0x34] = (track_size / 256) as u8;

        let mut track = vec![0u8; BLOCK_SIZE];
        track[..TRACK_SIGNATURE.len()].copy_from_slice(TRACK_SIGNATURE);
        track[0x14] = 1; // N=1 (256 bytes nominal)
        track[0x15] = 1;
        track[0x18] = 0;
        track[0x19] = 0;
        track[0x1A] = 1;
        track[0x1B] = 1;
        track[0x1E..0x20].copy_from_slice(&512u16.to_le_bytes()); // actual length override
        bytes.extend(track);

        let mut copy1 = vec![0x11u8; 256];
        let mut copy2 = vec![0x22u8; 256];
        copy1[0] = 0xAA;
        copy2[0] = 0xBB;
        bytes.extend(copy1);
        bytes.extend(copy2);

        let disk = parse(&bytes).unwrap();
        let sector = disk.read_sector(0, 0, 1).unwrap();

        assert_eq!(sector.data.len(), 256, "data reports the first real copy");
        assert_eq!(sector.data[0], 0xAA);
        assert_eq!(sector.copies.len(), 2, "both real copies are preserved");
        assert_eq!(sector.copies[0][0], 0xAA);
        assert_eq!(sector.copies[1][0], 0xBB);
    }

    #[test]
    fn multi_sector_track_reads_each_sectors_own_distinct_data() {
        // Real regression: parsing a track with more than one sector
        // must advance the file cursor past each sector's own stored
        // bytes before reading the next one, or every sector after the
        // first silently reads back the first sector's own data.
        let mut bytes = vec![0u8; BLOCK_SIZE];
        bytes[..DISC_SIGNATURE.len()].copy_from_slice(DISC_SIGNATURE);
        bytes[0x30] = 1; // one track
        bytes[0x31] = 1; // one side
        let track_size = (BLOCK_SIZE + 512 + 512) as u16; // header + two 512-byte sectors
        bytes[0x32..0x34].copy_from_slice(&track_size.to_le_bytes());

        let mut track = vec![0u8; BLOCK_SIZE];
        track[..TRACK_SIGNATURE.len()].copy_from_slice(TRACK_SIGNATURE);
        track[0x15] = 2; // two sectors
        track[0x18] = 0; // sector 1: C
        track[0x19] = 0; // H
        track[0x1A] = 1; // R
        track[0x1B] = 2; // N = 512 bytes
        track[0x20] = 0; // sector 2: C
        track[0x21] = 0; // H
        track[0x22] = 2; // R
        track[0x23] = 2; // N = 512 bytes
        bytes.extend(track);

        let mut sector1 = vec![0x5Au8; 512];
        let mut sector2 = vec![0xA5u8; 512];
        sector1[0] = 0x11;
        sector2[0] = 0x22;
        bytes.extend(sector1);
        bytes.extend(sector2);

        let disk = parse(&bytes).unwrap();
        assert_eq!(disk.read_sector(0, 0, 1).unwrap().data[0], 0x11);
        assert_eq!(disk.read_sector(0, 0, 2).unwrap().data[0], 0x22);
    }

    fn sector_bytes(disk: &Disk, c: u8, h: u8, r: u8) -> Vec<u8> {
        disk.read_sector(c, h, r).unwrap().data.clone()
    }

    fn standard_dsk_round_trips_through_to_bytes() {
        let original = parse(&build_minimal_dsk()).unwrap();
        let bytes = original.to_bytes();
        assert_eq!(&bytes[..DISC_SIGNATURE.len()], DISC_SIGNATURE);
        let again = parse(&bytes).unwrap();
        assert!(!again.is_extended());
        assert_eq!(
            sector_bytes(&again, 0, 0, 1),
            sector_bytes(&original, 0, 0, 1)
        );
        // A second round trip is byte-stable.
        assert_eq!(again.to_bytes(), bytes);
    }

    fn extended_dsk_round_trips_including_unformatted_track() {
        let original = parse(&build_minimal_extended_dsk()).unwrap();
        let bytes = original.to_bytes();
        assert_eq!(
            &bytes[..EXTENDED_DISC_SIGNATURE.len()],
            EXTENDED_DISC_SIGNATURE
        );
        let again = parse(&bytes).unwrap();
        assert!(again.is_extended());
        assert_eq!(sector_bytes(&again, 0, 0, 1)[0], 0x22);
        assert!(
            again.first_sector(1, 0).is_none(),
            "track 1 stays unformatted"
        );
        assert_eq!(again.to_bytes(), bytes);
    }

    fn weak_sector_copies_survive_a_round_trip_as_extended_dsk() {
        let mut bytes = vec![0u8; BLOCK_SIZE];
        bytes[..DISC_SIGNATURE.len()].copy_from_slice(DISC_SIGNATURE);
        bytes[0x30] = 1;
        bytes[0x31] = 1;
        let mut track = vec![0u8; BLOCK_SIZE];
        track[..TRACK_SIGNATURE.len()].copy_from_slice(TRACK_SIGNATURE);
        track[0x14] = 1;
        track[0x15] = 1;
        track[0x1A] = 1;
        track[0x1B] = 1;
        track[0x1E..0x20].copy_from_slice(&512u16.to_le_bytes());
        bytes.extend(track);
        bytes.extend(vec![0xAAu8; 256]);
        bytes.extend(vec![0xBBu8; 256]);
        bytes[0x32..0x34].copy_from_slice(&((BLOCK_SIZE + 512) as u16).to_le_bytes());

        let disk = parse(&bytes).unwrap();
        assert_eq!(disk.read_sector(0, 0, 1).unwrap().copies.len(), 2);
        let again = parse(&disk.to_bytes()).unwrap();
        let s = again.read_sector(0, 0, 1).unwrap();
        assert_eq!(s.copies.len(), 2, "copies preserved");
        assert_eq!(s.copies[1][0], 0xBB);
    }

    fn sector_st1_st2_flags_are_preserved() {
        let mut bytes = build_minimal_dsk();
        bytes[0x100 + 0x18 + 4] = 0x20; // ST1 data error
        bytes[0x100 + 0x18 + 5] = 0x40; // ST2 deleted data mark
        let disk = parse(&bytes).unwrap();
        let s = disk.read_sector(0, 0, 1).unwrap();
        assert_eq!((s.st1, s.st2), (0x20, 0x40));
        let again = parse(&disk.to_bytes()).unwrap();
        let s = again.read_sector(0, 0, 1).unwrap();
        assert_eq!((s.st1, s.st2), (0x20, 0x40));
    }
    /// Merged (every check kept): `standard_dsk_round_trips_through_to_bytes`, `extended_dsk_round_trips_including_unformatted_track`, `weak_sector_copies_survive_a_round_trip_as_extended_dsk`, `sector_st1_st2_flags_are_preserved`.
    #[test]
    fn dsk_round_trips() {
        standard_dsk_round_trips_through_to_bytes();
        extended_dsk_round_trips_including_unformatted_track();
        weak_sector_copies_survive_a_round_trip_as_extended_dsk();
        sector_st1_st2_flags_are_preserved();
    }

    fn write_sector_at_changes_data_sets_modified_and_survives_serialisation() {
        let mut disk = parse(&build_minimal_dsk()).unwrap();
        assert!(!disk.is_modified());
        assert!(disk.write_sector_at(0, 0, 0, &[0x99; 512], false));
        assert!(disk.is_modified());
        assert_eq!(sector_bytes(&disk, 0, 0, 1)[0], 0x99);
        let again = parse(&disk.to_bytes()).unwrap();
        assert_eq!(sector_bytes(&again, 0, 0, 1), vec![0x99; 512]);
        assert!(!again.is_modified(), "a freshly parsed image is clean");
    }

    fn write_sector_at_pads_short_data_and_can_mark_deleted() {
        let mut disk = parse(&build_minimal_dsk()).unwrap();
        assert!(disk.write_sector_at(0, 0, 0, &[1, 2, 3], true));
        let s = disk.read_sector(0, 0, 1).unwrap();
        assert_eq!(&s.data[..4], &[1, 2, 3, 0]);
        assert_eq!(s.data.len(), 512);
        assert_eq!(s.st2 & 0x40, 0x40);
    }

    fn write_sector_at_rejects_a_missing_sector_or_track() {
        let mut disk = parse(&build_minimal_dsk()).unwrap();
        assert!(!disk.write_sector_at(0, 0, 5, &[0; 512], false));
        assert!(!disk.write_sector_at(9, 0, 0, &[0; 512], false));
        assert!(!disk.is_modified());
    }
    /// Merged (every check kept): `write_sector_at_changes_data_sets_modified_and_survives_serialisation`, `write_sector_at_pads_short_data_and_can_mark_deleted`, `write_sector_at_rejects_a_missing_sector_or_track`.
    #[test]
    fn write_sector_at_cases() {
        write_sector_at_changes_data_sets_modified_and_survives_serialisation();
        write_sector_at_pads_short_data_and_can_mark_deleted();
        write_sector_at_rejects_a_missing_sector_or_track();
    }

    fn format_track_replaces_a_track_and_serialises() {
        let mut disk = parse(&build_minimal_dsk()).unwrap();
        let ids = [[0, 0, 0xC1, 2], [0, 0, 0xC2, 2], [0, 0, 0xC3, 2]];
        assert!(disk.format_track(0, 0, &ids, 0x4E, 0xE5));
        assert!(disk.is_modified());
        assert_eq!(disk.sector_count(0, 0), 3);
        assert_eq!(sector_bytes(&disk, 0, 0, 0xC2), vec![0xE5; 512]);
        assert!(disk.read_sector(0, 0, 1).is_none(), "old sector gone");
        let again = parse(&disk.to_bytes()).unwrap();
        assert_eq!(again.sector_count(0, 0), 3);
        assert_eq!(sector_bytes(&again, 0, 0, 0xC3), vec![0xE5; 512]);
    }

    fn format_track_grows_the_image_to_a_new_cylinder() {
        let mut disk = parse(&build_minimal_dsk()).unwrap();
        assert_eq!(disk.cylinders(), 1);
        assert!(disk.format_track(3, 0, &[[3, 0, 1, 1]], 0x4E, 0x00));
        assert_eq!(disk.cylinders(), 4);
        assert_eq!(disk.sector_count(2, 0), 0, "gap cylinders stay unformatted");
        let again = parse(&disk.to_bytes()).unwrap();
        assert_eq!(again.cylinders(), 4);
        assert_eq!(sector_bytes(&again, 3, 0, 1), vec![0; 256]);
    }

    fn format_track_rejects_an_unstored_head_or_absurd_cylinder() {
        let mut disk = parse(&build_minimal_dsk()).unwrap(); // one head
        assert!(!disk.format_track(0, 1, &[[0, 1, 1, 2]], 0x4E, 0));
        assert!(!disk.format_track(200, 0, &[[200, 0, 1, 2]], 0x4E, 0));
        assert!(!disk.is_modified());
    }
    /// Merged (every check kept): `format_track_replaces_a_track_and_serialises`, `format_track_grows_the_image_to_a_new_cylinder`, `format_track_rejects_an_unstored_head_or_absurd_cylinder`.
    #[test]
    fn format_track_cases() {
        format_track_replaces_a_track_and_serialises();
        format_track_grows_the_image_to_a_new_cylinder();
        format_track_rejects_an_unstored_head_or_absurd_cylinder();
    }

    #[test]
    fn oversized_size_codes_do_not_overflow_the_shift() {
        // A malformed N of 200 must not panic (`128 << 200` would).
        assert_eq!(sector_size(200), 128 << 8);
    }

    #[test]
    fn a_new_disk_is_empty_and_formats_up_like_a_real_one() {
        let mut disk = Disk::new(1);
        assert_eq!(disk.cylinders(), 0);
        assert!(!disk.is_modified());
        for c in 0..3u8 {
            let ids: Vec<[u8; 4]> = (1..=9).map(|r| [c, 0, 0xC0 + r, 2]).collect();
            assert!(disk.format_track(c, 0, &ids, 0x4E, 0xE5));
        }
        assert_eq!(disk.cylinders(), 3);
        assert_eq!(disk.sector_count(2, 0), 9);
        let again = parse(&disk.to_bytes()).unwrap();
        assert_eq!(again.cylinders(), 3);
        assert_eq!(again.sector_count(1, 0), 9);
        assert_eq!(again.read_sector(1, 0, 0xC5).unwrap().data, vec![0xE5; 512]);
    }
}
