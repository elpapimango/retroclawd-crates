/// Broadcast/market variant a machine was built for. Affects both CPU clock
/// speed (crystal differs per market) and refresh rate (a machine's
/// `frame_hz`) -- each machine impl maps this to its own concrete Hz
/// values; `Region` itself carries no numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Pal,
    Ntsc,
    Japan,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_are_distinct_and_copy() {
        let a = Region::Pal;
        let b = a; // proves Copy -- if this moved, using `a` below would fail to compile
        assert_eq!(a, Region::Pal);
        assert_ne!(b, Region::Ntsc);
        assert_ne!(Region::Ntsc, Region::Japan);
    }
}
