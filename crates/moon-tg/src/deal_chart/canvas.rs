//! The picture's pixels: 8-bit RGB, painted by blending a colour over what is there with a
//! coverage in 0..1. Rectangles are painted directly with fractional edges — the grid, the order
//! steps and the volume are all axis-aligned — and everything else goes through the rasterizer
//! ([`super::raster`]) as a mask.

use super::raster::{Mask, Path};

/// A colour, 8 bits per channel.
pub(super) type Rgb = [u8; 3];

/// `0xRRGGBB` as a colour.
pub(super) const fn rgb(hex: u32) -> Rgb {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

pub(super) struct Canvas {
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) px: Vec<u8>,
}

impl Canvas {
    /// A canvas filled with `bg`.
    pub(super) fn new(w: usize, h: usize, bg: Rgb) -> Self {
        Self {
            w,
            h,
            px: bg.repeat(w * h),
        }
    }

    /// Paint pixel (`x`, `y`) `alpha` of the way from what is there to `ink`. Off the canvas,
    /// nothing happens.
    fn blend(&mut self, x: i64, y: i64, ink: Rgb, alpha: f32) {
        if !(alpha > 0.0) || x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let alpha = alpha.min(1.0);
        let at = (y as usize * self.w + x as usize) * 3;
        for (channel, &target) in self.px[at..at + 3].iter_mut().zip(&ink) {
            let from = f32::from(*channel);
            *channel = (from + (f32::from(target) - from) * alpha).round() as u8;
        }
    }

    /// A rectangle between two corners; a pixel it covers in part is painted in part.
    pub(super) fn rect(
        &mut self,
        (x0, y0): (f32, f32),
        (x1, y1): (f32, f32),
        ink: Rgb,
        alpha: f32,
    ) {
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
            return;
        }
        let (left, right) = (x0.min(x1).max(0.0), x0.max(x1).min(self.w as f32));
        let (top, bottom) = (y0.min(y1).max(0.0), y0.max(y1).min(self.h as f32));
        if right <= left || bottom <= top {
            return;
        }
        for py in top.floor() as i64..bottom.ceil() as i64 {
            let cy = overlap(py, top, bottom);
            for px in left.floor() as i64..right.ceil() as i64 {
                self.blend(px, py, ink, alpha * cy * overlap(px, left, right));
            }
        }
    }

    /// A horizontal line `width` thick, centred on `y`.
    pub(super) fn hline(&mut self, x0: f32, x1: f32, y: f32, width: f32, ink: Rgb, alpha: f32) {
        self.rect((x0, y - width / 2.0), (x1, y + width / 2.0), ink, alpha);
    }

    /// A vertical line `width` thick, centred on `x`.
    pub(super) fn vline(&mut self, x: f32, y0: f32, y1: f32, width: f32, ink: Rgb, alpha: f32) {
        self.rect((x - width / 2.0, y0), (x + width / 2.0, y1), ink, alpha);
    }

    /// A filled circle.
    pub(super) fn disc(&mut self, (cx, cy): (f32, f32), r: f32, ink: Rgb, alpha: f32) {
        let sides = 32;
        let points: Vec<(f32, f32)> = (0..sides)
            .map(|i| {
                let a = i as f32 / sides as f32 * std::f32::consts::TAU;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        let mut path = Path::default();
        path.polygon(&points);
        if let Some(mask) = path.fill() {
            self.blend_mask(&mask, (0, 0), ink, alpha);
        }
    }

    /// `mask`, its own offset moved by `at`.
    pub(super) fn blend_mask(&mut self, mask: &Mask, at: (i64, i64), ink: Rgb, alpha: f32) {
        for row in 0..mask.h {
            let y = mask.top + at.1 + row as i64;
            for col in 0..mask.w {
                let cover = mask.cover[row * mask.w + col];
                if cover > 0.0 {
                    self.blend(mask.left + at.0 + col as i64, y, ink, alpha * cover);
                }
            }
        }
    }
}

/// A cross of two strokes `width` thick and `arm` long each way, centred on the origin: made once,
/// stamped at every print.
pub(super) fn cross_mask(arm: f32, width: f32) -> Option<Mask> {
    let half = width / 2.0 / std::f32::consts::SQRT_2;
    let mut path = Path::default();
    // Each diagonal as a thin rectangle; both have the same winding, so where they meet the
    // cover is the union, not twice the ink.
    path.polygon(&[
        (-arm - half, -arm + half),
        (-arm + half, -arm - half),
        (arm + half, arm - half),
        (arm - half, arm + half),
    ]);
    path.polygon(&[
        (arm - half, -arm - half),
        (arm + half, -arm + half),
        (-arm + half, arm + half),
        (-arm - half, arm - half),
    ]);
    path.fill()
}

/// How much of pixel `p` the stretch `[lo, hi)` covers.
fn overlap(p: i64, lo: f32, hi: f32) -> f32 {
    let p = p as f32;
    ((p + 1.0).min(hi) - p.max(lo)).clamp(0.0, 1.0)
}
