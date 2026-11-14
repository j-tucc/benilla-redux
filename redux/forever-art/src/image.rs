//! RGBA images: read PNG, crop, shrink, pad to a size benilla's device textures accept, write TGA.

use std::io::Write;

/// The largest side benilla makes a device texture at: a power of two below 1024
/// (`benilla_formats::makes_device_texture`).
pub const MAX_SIDE: u32 = 512;
/// The smallest side it accepts.
pub const MIN_SIDE: u32 = 8;

/// Straight (not premultiplied) RGBA8, rows top-down.
#[derive(Clone, Debug, PartialEq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        self.pixels[i..i + 4].try_into().unwrap()
    }

    fn set(&mut self, x: u32, y: u32, p: [u8; 4]) {
        let i = ((y * self.width + x) * 4) as usize;
        self.pixels[i..i + 4].copy_from_slice(&p);
    }

    /// Decode any PNG to RGBA8.
    pub fn from_png(bytes: &[u8]) -> Result<Self, String> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let (w, h) = (info.width, info.height);
        let src = &buf[..info.buffer_size()];
        let channels = match info.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => return Err("indexed png not expanded".into()),
        };
        let mut out = Rgba::new(w, h);
        for (dst, s) in out
            .pixels
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(src.chunks_exact(channels))
        {
            dst.copy_from_slice(&match channels {
                1 => [s[0], s[0], s[0], 255],
                2 => [s[0], s[0], s[0], s[1]],
                3 => [s[0], s[1], s[2], 255],
                _ => [s[0], s[1], s[2], s[3]],
            });
        }
        Ok(out)
    }

    /// The `left..right` × `top..bottom` region, clamped to the image.
    pub fn crop(&self, left: u32, top: u32, right: u32, bottom: u32) -> Self {
        let (r, b) = (right.min(self.width), bottom.min(self.height));
        let (l, t) = (left.min(r), top.min(b));
        let mut out = Rgba::new(r - l, b - t);
        for y in 0..out.height {
            for x in 0..out.width {
                out.set(x, y, self.at(l + x, t + y));
            }
        }
        out
    }

    /// Shrink by an area average (alpha-weighted, so transparent pixels don't darken edges)
    /// until both sides fit `max`, keeping the aspect ratio. Never enlarges.
    pub fn fit(&self, max: u32) -> Self {
        let scale = (max as f64 / self.width.max(self.height) as f64).min(1.0);
        if scale >= 1.0 {
            return self.clone();
        }
        let w = ((self.width as f64 * scale).round() as u32).max(1);
        let h = ((self.height as f64 * scale).round() as u32).max(1);
        let mut out = Rgba::new(w, h);
        for y in 0..h {
            let (y0, y1) = span(y, h, self.height);
            for x in 0..w {
                let (x0, x1) = span(x, w, self.width);
                let mut acc = [0f64; 4];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let p = self.at(sx, sy);
                        let a = p[3] as f64;
                        for c in 0..3 {
                            acc[c] += p[c] as f64 * a;
                        }
                        acc[3] += a;
                    }
                }
                let n = ((x1 - x0) * (y1 - y0)) as f64;
                let p = if acc[3] > 0.0 {
                    let rgb = |c: usize| (acc[c] / acc[3]).round() as u8;
                    [rgb(0), rgb(1), rgb(2), (acc[3] / n).round() as u8]
                } else {
                    [0; 4]
                };
                out.set(x, y, p);
            }
        }
        out
    }

    /// Resample to exactly `w` × `h`, bilinear on premultiplied alpha (so transparent texels don't
    /// bleed their colour). For the small up- and down-scales bar art needs.
    pub fn resize(&self, w: u32, h: u32) -> Self {
        let mut out = Rgba::new(w, h);
        let pre = |p: [u8; 4]| {
            let a = p[3] as f64 / 255.0;
            [
                p[0] as f64 * a,
                p[1] as f64 * a,
                p[2] as f64 * a,
                p[3] as f64,
            ]
        };
        for y in 0..h {
            let fy = ((y as f64 + 0.5) * self.height as f64 / h as f64 - 0.5)
                .clamp(0.0, (self.height - 1) as f64);
            let (y0, ty) = (fy.floor() as u32, fy.fract());
            let y1 = (y0 + 1).min(self.height - 1);
            for x in 0..w {
                let fx = ((x as f64 + 0.5) * self.width as f64 / w as f64 - 0.5)
                    .clamp(0.0, (self.width - 1) as f64);
                let (x0, tx) = (fx.floor() as u32, fx.fract());
                let x1 = (x0 + 1).min(self.width - 1);
                let (a, b) = (pre(self.at(x0, y0)), pre(self.at(x1, y0)));
                let (c, d) = (pre(self.at(x0, y1)), pre(self.at(x1, y1)));
                let mut v = [0f64; 4];
                for i in 0..4 {
                    let top = a[i] + (b[i] - a[i]) * tx;
                    let bottom = c[i] + (d[i] - c[i]) * tx;
                    v[i] = top + (bottom - top) * ty;
                }
                let alpha = v[3] / 255.0;
                let un = |c: f64| {
                    if alpha > 0.0 {
                        (c / alpha).round().clamp(0.0, 255.0) as u8
                    } else {
                        0
                    }
                };
                out.set(x, y, [un(v[0]), un(v[1]), un(v[2]), v[3].round() as u8]);
            }
        }
        out
    }

    /// Place at the top-left of the smallest power-of-two canvas (at least [`MIN_SIDE`]) and fill
    /// the rest by repeating the edge pixels, so filtering at the region's border samples itself.
    pub fn pad_pot(&self) -> Self {
        let mut out = Rgba::new(pot(self.width), pot(self.height));
        for y in 0..out.height {
            for x in 0..out.width {
                out.set(x, y, self.at(x.min(self.width - 1), y.min(self.height - 1)));
            }
        }
        out
    }

    /// A run-length TGA (type 10), 32-bit BGRA, top-left origin, 8 alpha bits.
    pub fn to_tga(&self) -> Vec<u8> {
        let mut out = vec![0u8; 18];
        out[2] = 10;
        out[12..14].copy_from_slice(&(self.width as u16).to_le_bytes());
        out[14..16].copy_from_slice(&(self.height as u16).to_le_bytes());
        out[16] = 32;
        out[17] = 0x20 | 8;
        let bgra: Vec<[u8; 4]> = self
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        // Packets never cross a row, as the format asks.
        for row in bgra.chunks(self.width as usize) {
            let mut i = 0;
            while i < row.len() {
                let mut run = 1;
                while i + run < row.len() && run < 128 && row[i + run] == row[i] {
                    run += 1;
                }
                if run > 1 {
                    out.push(0x80 | (run as u8 - 1));
                    out.extend_from_slice(&row[i]);
                    i += run;
                } else {
                    // A raw packet runs until the next pixel starts a repeat.
                    let start = i;
                    i += 1;
                    while i < row.len()
                        && i - start < 128
                        && !(i + 1 < row.len() && row[i + 1] == row[i])
                    {
                        i += 1;
                    }
                    out.push((i - start - 1) as u8);
                    for p in &row[start..i] {
                        out.extend_from_slice(p);
                    }
                }
            }
        }
        out
    }

    pub fn write_tga(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::File::create(path)?.write_all(&self.to_tga())
    }
}

/// The source rows (or columns) destination index `i` of `dst` covers in `src`.
fn span(i: u32, dst: u32, src: u32) -> (u32, u32) {
    let a = (i as u64 * src as u64 / dst as u64) as u32;
    let b = (((i + 1) as u64 * src as u64).div_ceil(dst as u64) as u32).clamp(a + 1, src);
    (a, b)
}

/// The smallest power of two at least `n` and at least [`MIN_SIDE`].
pub fn pot(n: u32) -> u32 {
    n.max(MIN_SIDE).next_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Rgba {
        let mut img = Rgba::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set(
                    x,
                    y,
                    [
                        (x * 7) as u8,
                        (y * 5) as u8,
                        ((x ^ y) * 3) as u8,
                        if (x + y) % 3 == 0 { 0 } else { 255 },
                    ],
                );
            }
        }
        img
    }

    #[test]
    fn pot_sizes() {
        assert_eq!(
            (pot(1), pot(8), pot(9), pot(198), pot(512)),
            (8, 8, 16, 256, 512)
        );
    }

    #[test]
    fn pad_repeats_edges() {
        let img = checker(5, 3).pad_pot();
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!(img.at(7, 7), checker(5, 3).at(4, 2));
        assert_eq!(img.at(2, 6), checker(5, 3).at(2, 2));
    }

    #[test]
    fn fit_shrinks_to_max_keeping_aspect() {
        let img = checker(2048, 1024).fit(512);
        assert_eq!((img.width, img.height), (512, 256));
        assert_eq!(checker(100, 40).fit(512), checker(100, 40));
    }

    #[test]
    fn resize_hits_exact_size_and_keeps_solid_colour() {
        let solid = Rgba {
            width: 124,
            height: 20,
            pixels: [40, 200, 60, 255].repeat(124 * 20),
        };
        let r = solid.resize(128, 32);
        assert_eq!((r.width, r.height), (128, 32));
        assert!(r.pixels.chunks(4).all(|p| p == [40, 200, 60, 255]));
        // Transparent neighbours don't darken an opaque texel's colour.
        let mut half = Rgba::new(2, 1);
        half.set(0, 0, [200, 100, 50, 255]);
        let r = half.resize(4, 1);
        assert_eq!(&r.pixels[4..7], &[200, 100, 50]);
    }

    #[test]
    fn crop_takes_region() {
        let img = checker(20, 20);
        let c = img.crop(3, 4, 10, 6);
        assert_eq!((c.width, c.height), (7, 2));
        assert_eq!(c.at(0, 0), img.at(3, 4));
        assert_eq!(c.at(6, 1), img.at(9, 5));
    }

    /// The written TGA decodes in benilla's own loader to the same pixels, at a size its device
    /// textures accept.
    #[test]
    fn tga_round_trips_through_benilla() {
        for (w, h) in [(8, 8), (256, 128), (16, 512)] {
            let img = checker(w, h);
            let tga = img.to_tga();
            assert_eq!(benilla_formats::tga_texture_size(&tga), Some((w, h)));
            assert!(benilla_formats::makes_device_texture(w, h));
            let (dw, dh, px) = benilla_formats::tga_to_rgba(&tga).unwrap();
            assert_eq!((dw, dh), (w, h));
            assert_eq!(px, img.pixels);
        }
        // Long runs and a solid image compress.
        let solid = Rgba {
            width: 256,
            height: 256,
            pixels: [9, 8, 7, 255].repeat(256 * 256),
        };
        let tga = solid.to_tga();
        assert!(tga.len() < 18 + 256 * 3 * 5);
        assert_eq!(benilla_formats::tga_to_rgba(&tga).unwrap().2, solid.pixels);
    }
}
