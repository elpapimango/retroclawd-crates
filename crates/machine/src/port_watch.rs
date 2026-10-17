//! I/O-port watchpoints: the machine-side half of the debugger's port
//! breakpoints.
//!
//! A Z80-family bus owns a [`PortWatcher`] and feeds it every `IN`/`OUT` the
//! CPU performs (via [`crate::Bus::io_accessed`], which the Z80 core calls
//! from its single I/O funnel). The watcher only *latches* a hit; the host's
//! debugger decides when to look (after each instruction) and what stopping
//! means, so this crate stays free of any debugger concept.
//!
//! Debugger-initiated port writes (`Cpc::write_port` and friends) go straight
//! to the bus and never through the CPU funnel, so they can't trip a watch.

/// One watched port range: a port matches when `port & mask == self.port &
/// mask`, so `mask = 0xFFFF` is an exact port, and a partial mask matches the
/// whole family of mirrors a loosely-decoded machine answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortWatch {
    pub port: u16,
    pub mask: u16,
    pub on_read: bool,
    pub on_write: bool,
}

impl PortWatch {
    #[must_use]
    pub fn matches(&self, port: u16, write: bool) -> bool {
        let direction = if write { self.on_write } else { self.on_read };
        direction && port & self.mask == self.port & self.mask
    }
}

/// The access that tripped a watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortHit {
    pub port: u16,
    /// The byte read (`IN`) or written (`OUT`).
    pub value: u8,
    pub write: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PortWatcher {
    watches: Vec<PortWatch>,
    hit: Option<PortHit>,
}

impl PortWatcher {
    /// Replaces the watch list (and forgets any latched hit).
    pub fn set_watches(&mut self, watches: Vec<PortWatch>) {
        self.watches = watches;
        self.hit = None;
    }

    #[must_use]
    pub fn watches(&self) -> &[PortWatch] {
        &self.watches
    }

    /// Records one port access. With no watches set -- the normal case -- this
    /// is a single `is_empty` check. The first matching access since the last
    /// [`take_hit`](Self::take_hit) is the one kept.
    #[inline]
    pub fn note(&mut self, port: u16, value: u8, write: bool) {
        if self.watches.is_empty() || self.hit.is_some() {
            return;
        }
        if self.watches.iter().any(|w| w.matches(port, write)) {
            self.hit = Some(PortHit { port, value, write });
        }
    }

    /// Takes the latched hit, if any.
    pub fn take_hit(&mut self) -> Option<PortHit> {
        self.hit.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(port: u16, mask: u16, on_read: bool, on_write: bool) -> PortWatch {
        PortWatch {
            port,
            mask,
            on_read,
            on_write,
        }
    }

    #[test]
    fn nothing_latches_without_watches() {
        let mut w = PortWatcher::default();
        w.note(0x7FFD, 0x10, true);
        assert_eq!(w.take_hit(), None);
    }

    fn an_exact_port_matches_only_its_own_direction() {
        let mut w = PortWatcher::default();
        w.set_watches(vec![watch(0x00FE, 0xFFFF, false, true)]);
        w.note(0x00FE, 1, false); // IN: not watched
        w.note(0x00FF, 1, true); // other port
        assert_eq!(w.take_hit(), None);
        w.note(0x00FE, 7, true);
        assert_eq!(
            w.take_hit(),
            Some(PortHit {
                port: 0x00FE,
                value: 7,
                write: true
            })
        );
        assert_eq!(w.take_hit(), None, "taking a hit clears it");
    }

    fn a_partial_mask_matches_every_mirror() {
        let mut w = PortWatcher::default();
        // Any port with bit 0 clear, i.e. every ULA mirror.
        w.set_watches(vec![watch(0x0000, 0x0001, true, true)]);
        w.note(0x00FF, 0, false);
        assert_eq!(w.take_hit(), None);
        w.note(0xFFFE, 0xBF, false);
        assert_eq!(
            w.take_hit(),
            Some(PortHit {
                port: 0xFFFE,
                value: 0xBF,
                write: false
            })
        );
    }
    /// Merged (every check kept): `an_exact_port_matches_only_its_own_direction`, `a_partial_mask_matches_every_mirror`.
    #[test]
    fn port_matching() {
        an_exact_port_matches_only_its_own_direction();
        a_partial_mask_matches_every_mirror();
    }

    fn the_first_hit_is_kept_until_taken() {
        let mut w = PortWatcher::default();
        w.set_watches(vec![watch(0, 0, true, true)]);
        w.note(0x1234, 1, true);
        w.note(0x5678, 2, true);
        assert_eq!(w.take_hit().map(|h| h.port), Some(0x1234));
        w.note(0x5678, 2, true);
        assert_eq!(w.take_hit().map(|h| h.port), Some(0x5678));
    }

    fn replacing_the_watches_forgets_a_latched_hit() {
        let mut w = PortWatcher::default();
        w.set_watches(vec![watch(0, 0, true, true)]);
        w.note(1, 1, true);
        w.set_watches(Vec::new());
        assert_eq!(w.take_hit(), None);
    }
    /// Merged (every check kept): `the_first_hit_is_kept_until_taken`, `replacing_the_watches_forgets_a_latched_hit`.
    #[test]
    fn latched_hit() {
        the_first_hit_is_kept_until_taken();
        replacing_the_watches_forgets_a_latched_hit();
    }
}
