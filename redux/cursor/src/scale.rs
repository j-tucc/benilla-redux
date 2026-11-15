//! The filtered resize behind every scaled cursor: Catmull-Rom bicubic on premultiplied RGBA8, so
//! an upscaled 32×32 BLP stays sharp at its edges without the dark fringe a straight-alpha filter
//! leaves round the arrow's outline.

/// Catmull-Rom's weight for a tap `x` texels from the sample point.
fn weight(x: f32) -> f32 {
    let x = x.abs();
    if x < 1.0 {
        1.5 * x * x * x - 2.5 * x * x + 1.0
    } else if x < 2.0 {
        -0.5 * x * x * x + 2.5 * x * x - 4.0 * x + 2.0
    } else {
        0.0
    }
}

/// For each output column (or row), the four source taps and their weights, the taps clamped to
/// the edge.
fn taps(src: u32, dst: u32) -> Vec<([usize; 4], [f32; 4])> {
    let ratio = src as f32 / dst as f32;
    (0..dst)
        .map(|o| {
            // The output texel's centre in source texel space.
            let x = (o as f32 + 0.5) * ratio - 0.5;
            let base = x.floor();
            let frac = x - base;
            let mut idx = [0usize; 4];
            let mut w = [0f32; 4];
            for k in 0..4 {
                let t = base as i64 + k as i64 - 1;
                idx[k] = t.clamp(0, i64::from(src) - 1) as usize;
                w[k] = weight(frac - (k as f32 - 1.0));
            }
            let sum: f32 = w.iter().sum();
            (idx, w.map(|v| v / sum))
        })
        .collect()
}

/// Resize top-to-bottom RGBA8 `(w, h)` to `(dw, dh)`. Same size returns the input unchanged.
pub fn resize_rgba(w: u32, h: u32, rgba: &[u8], dw: u32, dh: u32) -> Vec<u8> {
    assert_eq!(rgba.len(), (w * h * 4) as usize, "rgba is not {w}x{h}");
    if (w, h) == (dw, dh) {
        return rgba.to_vec();
    }
    // Premultiply into floats.
    let pre: Vec<[f32; 4]> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let a = f32::from(p[3]) / 255.0;
            [
                f32::from(p[0]) * a,
                f32::from(p[1]) * a,
                f32::from(p[2]) * a,
                f32::from(p[3]),
            ]
        })
        .collect();
    let xs = taps(w, dw);
    let ys = taps(h, dh);
    // Horizontal pass: h rows of dw.
    let mut mid = vec![[0f32; 4]; (dw * h) as usize];
    for y in 0..h as usize {
        let row = &pre[y * w as usize..(y + 1) * w as usize];
        for (x, (idx, wt)) in xs.iter().enumerate() {
            let mut acc = [0f32; 4];
            for k in 0..4 {
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += row[idx[k]][c] * wt[k];
                }
            }
            mid[y * dw as usize + x] = acc;
        }
    }
    // Vertical pass, then un-premultiply; the bicubic's overshoot is clamped, colour under alpha.
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for (y, (idx, wt)) in ys.iter().enumerate() {
        for x in 0..dw as usize {
            let mut acc = [0f32; 4];
            for k in 0..4 {
                let p = mid[idx[k] * dw as usize + x];
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += p[c] * wt[k];
                }
            }
            let alpha = acc[3].clamp(0.0, 255.0);
            let o = (y * dw as usize + x) * 4;
            if alpha < 0.5 {
                continue; // fully transparent: leave black, alpha 0
            }
            let a = alpha / 255.0;
            for c in 0..3 {
                out[o + c] = (acc[c].clamp(0.0, alpha) / a).round().clamp(0.0, 255.0) as u8;
            }
            out[o + 3] = alpha.round() as u8;
        }
    }
    out
}

/// A hotspot `(x, y)` on a `(w, h)` image, moved onto the same point of its `(dw, dh)` resize and
/// kept inside it. A hotspot names a texel's top-left corner (the arrow's tip is `(0, 0)`), so it
/// scales corner to corner: the tip stays on the tip.
pub fn scale_hotspot(hotspot: (u16, u16), (w, h): (u32, u32), (dw, dh): (u32, u32)) -> (u16, u16) {
    let s = |v: u16, from: u32, to: u32| -> u16 {
        let scaled = u64::from(v) * u64::from(to) / u64::from(from.max(1));
        scaled.min(u64::from(to.max(1) - 1)) as u16
    };
    (s(hotspot.0, w, dw), s(hotspot.1, h, dh))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_size_is_untouched() {
        let px: Vec<u8> = (0..4 * 4 * 4).map(|i| i as u8).collect();
        assert_eq!(resize_rgba(4, 4, &px, 4, 4), px);
    }

    #[test]
    fn a_flat_colour_stays_flat_at_every_size() {
        let px: Vec<u8> = [200u8, 100, 50, 255].repeat(32 * 32);
        for size in [48, 64, 96, 128] {
            let out = resize_rgba(32, 32, &px, size, size);
            assert_eq!(out.len(), (size * size * 4) as usize);
            assert!(out
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [200, 100, 50, 255]));
        }
    }

    #[test]
    fn transparent_texels_do_not_darken_their_neighbours() {
        // Left half opaque white, right half fully transparent *black*: a straight-alpha filter
        // pulls the edge texels grey; premultiplied, every visible texel stays white.
        let mut px = Vec::new();
        for _y in 0..8 {
            for x in 0..8 {
                px.extend_from_slice(if x < 4 {
                    &[255, 255, 255, 255]
                } else {
                    &[0, 0, 0, 0]
                });
            }
        }
        let out = resize_rgba(8, 8, &px, 16, 16);
        for p in out.as_chunks::<4>().0.iter().filter(|p| p[3] > 8) {
            assert!(
                p[0] >= 250 && p[1] >= 250 && p[2] >= 250,
                "fringed texel {p:?}"
            );
        }
    }

    #[test]
    fn the_hotspot_scales_with_the_image() {
        assert_eq!(scale_hotspot((0, 0), (32, 32), (64, 64)), (0, 0));
        assert_eq!(scale_hotspot((16, 16), (32, 32), (64, 64)), (32, 32));
        assert_eq!(scale_hotspot((5, 9), (32, 32), (48, 48)), (7, 13));
        assert_eq!(scale_hotspot((31, 31), (32, 32), (128, 128)), (124, 124));
        assert_eq!(scale_hotspot((10, 4), (32, 32), (32, 32)), (10, 4));
    }
}
