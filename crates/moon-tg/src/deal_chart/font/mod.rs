//! The chart's text: Geist Mono — the terminal's own face (`assets/fonts/GeistMono-OFL.txt`) —
//! for everything it has, and a cut of Noto Sans SC behind it for coin names in hanzi or kana
//! (`assets/fonts/README.md`). Both are compiled in: a fresh server has no fonts at all.
//!
//! Glyphs are read by our own TrueType reader ([`ttf`]) and filled by the chart's rasterizer; one
//! picture keeps the masks it already made, so a digit is rasterised once per size.

mod ttf;

use std::collections::HashMap;
use std::sync::OnceLock;

use super::canvas::{Canvas, Rgb};
use super::raster::{Mask, Path};
use ttf::{Face, Segment};

const MONO: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Regular.ttf");
const CJK: &[u8] = include_bytes!("../../../assets/fonts/NotoSansSC-Subset.ttf");

/// Both faces, parsed once.
struct Faces {
    mono: Option<Face<'static>>,
    cjk: Option<Face<'static>>,
}

fn faces() -> &'static Faces {
    static FACES: OnceLock<Faces> = OnceLock::new();
    FACES.get_or_init(|| Faces {
        mono: Face::parse(MONO),
        cjk: Face::parse(CJK),
    })
}

/// Which face a glyph comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Which {
    Mono,
    Cjk,
}

impl Faces {
    fn face(&self, which: Which) -> Option<&Face<'static>> {
        match which {
            Which::Mono => self.mono.as_ref(),
            Which::Cjk => self.cjk.as_ref(),
        }
    }

    /// The face and glyph for `ch`: Geist Mono's, else the CJK face's, else Geist Mono's
    /// question mark.
    fn resolve(&self, ch: char) -> Option<(Which, u16)> {
        let mono = self.mono.as_ref();
        if let Some(glyph) = mono.and_then(|f| f.glyph(ch)) {
            return Some((Which::Mono, glyph));
        }
        if let Some(glyph) = self.cjk.as_ref().and_then(|f| f.glyph(ch)) {
            return Some((Which::Cjk, glyph));
        }
        mono.and_then(|f| f.glyph('?')).map(|g| (Which::Mono, g))
    }
}

/// The text of one picture, with the glyph masks it has made so far.
#[derive(Default)]
pub(super) struct Text {
    masks: HashMap<(Which, u16, u32), Option<Mask>>,
}

impl Text {
    /// How wide `s` comes out at `size` pixels to the em.
    pub(super) fn width(&self, s: &str, size: f32) -> f32 {
        let faces = faces();
        s.chars()
            .filter_map(|ch| faces.resolve(ch))
            .map(|(which, glyph)| advance(faces, which, glyph, size))
            .sum()
    }

    /// `s` cut to `max` pixels wide, ending in "..." when it had to be cut.
    pub(super) fn fit(&self, s: &str, size: f32, max: f32) -> String {
        if self.width(s, size) <= max {
            return s.to_owned();
        }
        let room = max - self.width("...", size);
        let mut out = String::new();
        for ch in s.chars() {
            let mut next = out.clone();
            next.push(ch);
            if self.width(&next, size) > room {
                break;
            }
            out = next;
        }
        out.push_str("...");
        out
    }

    /// The height of a capital at `size`: what "the top of the text" is when a label is placed
    /// by its box.
    pub(super) fn cap_height(&self, size: f32) -> f32 {
        let faces = faces();
        faces
            .mono
            .as_ref()
            .and_then(|f| Some(f.top(f.glyph('H')?)? / f.units_per_em() * size))
            .unwrap_or(size * 0.7)
    }

    /// Draw `s` with its left end at `x` and its baseline at `y`.
    pub(super) fn draw(
        &mut self,
        canvas: &mut Canvas,
        (x, y): (f32, f32),
        s: &str,
        size: f32,
        ink: Rgb,
        alpha: f32,
    ) {
        let faces = faces();
        let mut pen = x;
        for ch in s.chars() {
            let Some((which, glyph)) = faces.resolve(ch) else {
                continue;
            };
            // Placed on whole pixels: the mask is made once per size, the pen keeps its fraction.
            let (px, py) = (pen.round(), y.round());
            let key = (which, glyph, size.to_bits());
            let mask = self
                .masks
                .entry(key)
                .or_insert_with(|| glyph_mask(faces, which, glyph, size));
            if let Some(mask) = mask {
                canvas.blend_mask(mask, (px as i64, py as i64), ink, alpha);
            }
            pen += advance(faces, which, glyph, size);
        }
    }
}

fn advance(faces: &Faces, which: Which, glyph: u16, size: f32) -> f32 {
    faces
        .face(which)
        .map_or(0.0, |f| f.advance(glyph) / f.units_per_em() * size)
}

/// The coverage of one glyph at `size`, relative to its origin on the baseline.
fn glyph_mask(faces: &Faces, which: Which, glyph: u16, size: f32) -> Option<Mask> {
    let face = faces.face(which)?;
    let scale = size / face.units_per_em();
    // Font units are y up; the canvas is y down.
    let at = |(x, y): (f32, f32)| (x * scale, -y * scale);
    let mut path = Path::default();
    for segment in face.outline(glyph) {
        match segment {
            Segment::Line(a, b) => path.line(at(a), at(b)),
            Segment::Quad(a, c, b) => path.quad(at(a), at(c), at(b)),
        }
    }
    path.fill()
}

#[cfg(test)]
mod tests;
