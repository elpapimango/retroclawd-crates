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
