/// Composites `inner` (an `inner_w`x`inner_h` RGB buffer, row-major,
/// top-to-bottom) into a larger buffer padded by `border_w`/`border_h`
/// pixels on each side (so the total size is
/// `(inner_w + border_w*2) x (inner_h + border_h*2)`), filled with
/// `border_rgb`. Returns `(total_w, total_h, rgb)`, the same shape
/// `Machine::render_frame` returns.
///
/// CPU/bus-agnostic pixel compositing -- lives here (not in a
/// machine-specific crate) because `Machine::render_frame`'s RGB-tuple
/// contract is already defined in this crate, and both `zx81` and
/// `zx-spectrum` need this same border-around-a-picture composition (real
/// hardware on both: the "active picture" is smaller than the full
/// TV-visible area, with the difference filled by a border color -- fixed
/// white on ZX81, software-controlled via `OUT (0xFE)` on ZX Spectrum).
pub fn with_border(
    inner_w: u32,
    inner_h: u32,
    inner: &[u8],
    border_w: u32,
    border_h: u32,
    border_rgb: [u8; 3],
) -> (u32, u32, Vec<u8>) {
    if border_w == 0 && border_h == 0 {
        return (inner_w, inner_h, inner.to_vec());
    }

    let total_w = inner_w + border_w * 2;
    let total_h = inner_h + border_h * 2;
    let total_row_bytes = total_w as usize * 3;

    let mut border_row = vec![0u8; total_row_bytes];
    for pixel in border_row.as_chunks_mut::<3>().0 {
        pixel[0] = border_rgb[0];
        pixel[1] = border_rgb[1];
        pixel[2] = border_rgb[2];
    }
    let mut out = vec![0u8; total_row_bytes * total_h as usize];
    for row in out.chunks_exact_mut(total_row_bytes) {
        row.copy_from_slice(&border_row);
    }

    let row_bytes = (inner_w * 3) as usize;
    for row in 0..inner_h {
        let dst_start = (((row + border_h) * total_w + border_w) * 3) as usize;
        let src_start = (row * inner_w * 3) as usize;
        out[dst_start..dst_start + row_bytes]
            .copy_from_slice(&inner[src_start..src_start + row_bytes]);
    }

    (total_w, total_h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_border_returns_the_inner_buffer_unchanged() {
        let inner = vec![1, 2, 3, 4, 5, 6]; // 2x1 RGB
        let (w, h, rgb) = with_border(2, 1, &inner, 0, 0, [9, 9, 9]);

        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb, inner);
    }

    fn pads_inner_buffer_with_border_color_on_all_sides() {
        // 1x1 white inner pixel, 1px black border on every side -> 3x3.
        let inner = vec![255, 255, 255];
        let (w, h, rgb) = with_border(1, 1, &inner, 1, 1, [0, 0, 0]);

        assert_eq!((w, h), (3, 3));
        assert_eq!(rgb.len(), 3 * 3 * 3);

        // Center pixel (1,1) is the inner pixel; every other pixel is border.
        for y in 0..3u32 {
            for x in 0..3u32 {
                let offset = ((y * 3 + x) * 3) as usize;
                let expected = if x == 1 && y == 1 {
                    [255, 255, 255]
                } else {
                    [0, 0, 0]
                };
                assert_eq!(&rgb[offset..offset + 3], expected);
            }
        }
    }

    fn asymmetric_border_width_and_height_are_independent() {
        let inner = vec![7, 7, 7]; // 1x1
        let (w, h, _) = with_border(1, 1, &inner, 4, 2, [0, 0, 0]);

        assert_eq!(w, 1 + 4 * 2);
        assert_eq!(h, 1 + 2 * 2);
    }
    /// Merged (every check kept): `zero_border_returns_the_inner_buffer_unchanged`, `pads_inner_buffer_with_border_color_on_all_sides`, `asymmetric_border_width_and_height_are_independent`.
    #[test]
    fn border_padding() {
        zero_border_returns_the_inner_buffer_unchanged();
        pads_inner_buffer_with_border_color_on_all_sides();
        asymmetric_border_width_and_height_are_independent();
    }
}
