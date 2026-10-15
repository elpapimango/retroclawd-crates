use crate::Bus;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RegionKind {
    Rom,
    Ram,
}

#[derive(Clone, Debug)]
struct Region {
    base: u16,
    kind: RegionKind,
    data: Vec<u8>,
}

impl Region {
    fn contains(&self, addr: u16) -> Option<usize> {
        let base = self.base as usize;
        let end = base + self.data.len();
        let addr = addr as usize;
        (addr >= base && addr < end).then(|| addr - base)
    }
}

/// A memory map built from ROM/RAM regions. Reads outside every mapped
/// region return `unmapped_fill`; writes outside every region, or into a ROM
/// region, are silently dropped -- both match how real hardware behaves on
/// an unmapped bus or a write to masked-out ROM.
#[derive(Clone, Debug)]
pub struct MemoryBus {
    regions: Vec<Region>,
    unmapped_fill: u8,
}

impl MemoryBus {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            unmapped_fill: 0xFF,
        }
    }

    pub fn map_ram(&mut self, base: u16, len: usize) {
        self.regions.push(Region {
            base,
            kind: RegionKind::Ram,
            data: vec![0; len],
        });
    }

    pub fn map_rom(&mut self, base: u16, data: Vec<u8>) {
        self.regions.push(Region {
            base,
            kind: RegionKind::Rom,
            data,
        });
    }

    /// Writes `bytes` starting at `base` through the normal write path, so
    /// this only lands where a RAM region already covers the range.
    ///
    /// Returns how many bytes were actually stored -- a byte aimed at ROM or
    /// at unmapped space is dropped (as `write` would drop it) and not
    /// counted, so a caller can tell a fully-loaded program from a silently
    /// truncated one.
    pub fn load(&mut self, base: u16, bytes: &[u8]) -> usize {
        let mut addr = base;
        let mut written = 0;
        for &b in bytes {
            if let Some((region, off)) = self.region_for(addr)
                && region.kind == RegionKind::Ram
            {
                region.data[off] = b;
                written += 1;
            }
            addr = addr.wrapping_add(1);
        }
        written
    }

    fn region_for(&mut self, addr: u16) -> Option<(&mut Region, usize)> {
        self.regions
            .iter_mut()
            .find_map(|r| r.contains(addr).map(|off| (r, off)))
    }

    /// Read-only peek at one byte, for display/introspection -- unlike
    /// `Bus::read` this doesn't need `&mut self`.
    pub fn peek(&self, addr: u16) -> u8 {
        self.regions
            .iter()
            .find_map(|r| r.contains(addr).map(|off| r.data[off]))
            .unwrap_or(self.unmapped_fill)
    }
}

impl Default for MemoryBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for MemoryBus {
    fn read(&mut self, addr: u16) -> u8 {
        match self.region_for(addr) {
            Some((region, off)) => region.data[off],
            None => self.unmapped_fill,
        }
    }

    fn write(&mut self, addr: u16, val: u8) {
        if let Some((region, off)) = self.region_for(addr)
            && region.kind == RegionKind::Ram
        {
            region.data[off] = val;
        }
    }

    fn read_io(&mut self, _port: u16) -> u8 {
        0
    }

    fn write_io(&mut self, _port: u16, _val: u8) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_round_trips_reads_and_writes() {
        let mut bus = MemoryBus::new();
        bus.map_ram(0x4000, 0x100);

        bus.write(0x4010, 0x42);

        assert_eq!(bus.read(0x4010), 0x42);
    }

    #[test]
    fn rom_ignores_writes_but_keeps_initial_contents() {
        let mut bus = MemoryBus::new();
        bus.map_rom(0x0000, vec![0xAA, 0xBB]);

        bus.write(0x0000, 0xFF);

        assert_eq!(bus.read(0x0000), 0xAA);
        assert_eq!(bus.read(0x0001), 0xBB);
    }

    #[test]
    fn unmapped_address_reads_fill_value_and_ignores_writes() {
        let mut bus = MemoryBus::new();
        bus.map_ram(0x8000, 0x10);

        bus.write(0x0000, 0x99);

        assert_eq!(bus.read(0x0000), 0xFF);
    }

    #[test]
    fn load_writes_bytes_sequentially_into_ram() {
        let mut bus = MemoryBus::new();
        bus.map_ram(0x0000, 0x10000);

        let written = bus.load(0x0100, &[0x11, 0x22, 0x33]);

        assert_eq!(written, 3);
        assert_eq!(bus.read(0x0100), 0x11);
        assert_eq!(bus.read(0x0101), 0x22);
        assert_eq!(bus.read(0x0102), 0x33);
    }

    #[test]
    fn load_outside_any_region_is_dropped_and_not_counted() {
        let mut bus = MemoryBus::new();
        bus.map_ram(0x8000, 0x10);

        let written = bus.load(0x0000, &[0x11, 0x22]);

        assert_eq!(written, 0);
        assert_eq!(bus.read(0x0000), 0xFF);
        assert_eq!(bus.read(0x0001), 0xFF);
    }

    #[test]
    fn load_counts_only_the_bytes_that_land_in_ram() {
        let mut bus = MemoryBus::new();
        bus.map_ram(0x8000, 1); // a one-byte RAM window at 0x8000

        // First byte lands in RAM, the next two fall off the end of it.
        let written = bus.load(0x8000, &[0x11, 0x22, 0x33]);

        assert_eq!(written, 1);
        assert_eq!(bus.read(0x8000), 0x11);
        assert_eq!(bus.read(0x8001), 0xFF);
        assert_eq!(bus.read(0x8002), 0xFF);
    }

    #[test]
    fn load_into_rom_is_dropped_and_not_counted() {
        let mut bus = MemoryBus::new();
        bus.map_rom(0x0000, vec![0xAA, 0xBB]);

        let written = bus.load(0x0000, &[0x11, 0x22]);

        assert_eq!(written, 0);
        assert_eq!(bus.read(0x0000), 0xAA);
        assert_eq!(bus.read(0x0001), 0xBB);
    }

    #[test]
    fn peek_does_not_require_mutable_access() {
        let mut bus = MemoryBus::new();
        bus.map_ram(0x4000, 0x10);
        bus.write(0x4000, 0x77);

        let bus = bus; // shadow as immutable -- proves peek takes &self
        assert_eq!(bus.peek(0x4000), 0x77);
        assert_eq!(bus.peek(0x0000), 0xFF);
    }
}
