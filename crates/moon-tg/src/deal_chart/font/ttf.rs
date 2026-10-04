//! A TrueType reader for the chart's two faces: the character map (format 4), the horizontal
//! metrics, and the quadratic outlines of `glyf`, composite glyphs included. Nothing else a font
//! carries — hinting, kerning, layout tables — matters to a picture this size.
//!
//! Every read is bounds-checked: a damaged face yields missing glyphs, never a panic.

/// A point of an outline, in font units, y up.
pub(super) type Point = (f32, f32);

/// One piece of an outline contour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Segment {
    Line(Point, Point),
    /// From, control, to.
    Quad(Point, Point, Point),
}

/// How deep composite glyphs may nest before the reader stops following them.
const MAX_NESTING: u32 = 6;

/// A parsed face: the offsets of the tables the chart reads.
pub(super) struct Face<'a> {
    data: &'a [u8],
    units_per_em: f32,
    long_loca: bool,
    num_glyphs: u16,
    num_h_metrics: u16,
    cmap: usize,
    hmtx: usize,
    loca: usize,
    glyf: usize,
}

fn u8_at(data: &[u8], at: usize) -> Option<u8> {
    data.get(at).copied()
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]))
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    u16_at(data, at).map(|v| v as i16)
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
        *data.get(at + 2)?,
        *data.get(at + 3)?,
    ]))
}

/// A 2.14 fixed-point number.
fn f2dot14_at(data: &[u8], at: usize) -> Option<f32> {
    i16_at(data, at).map(|v| f32::from(v) / 16384.0)
}

impl<'a> Face<'a> {
    /// Read the table directory and the header fields the chart needs. `None` when a table is
    /// missing or out of the file.
    pub(super) fn parse(data: &'a [u8]) -> Option<Self> {
        let tables = usize::from(u16_at(data, 4)?);
        let find = |tag: &[u8; 4]| -> Option<usize> {
            (0..tables).find_map(|i| {
                let entry = 12 + i * 16;
                if data.get(entry..entry + 4)? != tag {
                    return None;
                }
                let offset = u32_at(data, entry + 8)? as usize;
                let length = u32_at(data, entry + 12)? as usize;
                (offset.checked_add(length)? <= data.len()).then_some(offset)
            })
        };
        let head = find(b"head")?;
        let hhea = find(b"hhea")?;
        let maxp = find(b"maxp")?;
        let face = Self {
            data,
            units_per_em: f32::from(u16_at(data, head + 18)?.max(1)),
            long_loca: i16_at(data, head + 50)? == 1,
            num_glyphs: u16_at(data, maxp + 4)?,
            num_h_metrics: u16_at(data, hhea + 34)?.max(1),
            cmap: find(b"cmap")?,
            hmtx: find(b"hmtx")?,
            loca: find(b"loca")?,
            glyf: find(b"glyf")?,
        };
        Some(face)
    }

    pub(super) fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    /// The glyph `ch` maps to, `None` for a character the face does not have.
    pub(super) fn glyph(&self, ch: char) -> Option<u16> {
        let code = u16::try_from(u32::from(ch)).ok()?;
        let table = self.unicode_subtable()?;
        let glyph = self.format4_lookup(table, code)?;
        (glyph != 0 && glyph < self.num_glyphs).then_some(glyph)
    }

    /// The first Unicode BMP subtable in format 4: Windows Unicode, then Unicode platform.
    fn unicode_subtable(&self) -> Option<usize> {
        let d = self.data;
        let count = usize::from(u16_at(d, self.cmap + 2)?);
        let mut fallback = None;
        for i in 0..count {
            let record = self.cmap + 4 + i * 8;
            let (platform, encoding) = (u16_at(d, record)?, u16_at(d, record + 2)?);
            let table = self.cmap + u32_at(d, record + 4)? as usize;
            if u16_at(d, table)? != 4 {
                continue;
            }
            match (platform, encoding) {
                (3, 1) => return Some(table),
                (0, _) => fallback = fallback.or(Some(table)),
                _ => {}
            }
        }
        fallback
    }

    /// Format 4: segments of character codes, each mapped by a delta or through a glyph array.
    fn format4_lookup(&self, table: usize, code: u16) -> Option<u16> {
        let d = self.data;
        let segments = usize::from(u16_at(d, table + 6)? / 2);
        let ends = table + 14;
        let starts = ends + segments * 2 + 2;
        let deltas = starts + segments * 2;
        let ranges = deltas + segments * 2;
        // The first segment whose end is at or past the code: the ends are sorted.
        let (mut lo, mut hi) = (0usize, segments);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if u16_at(d, ends + mid * 2)? < code {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == segments {
            return None;
        }
        let start = u16_at(d, starts + lo * 2)?;
        if code < start {
            return None;
        }
        let delta = u16_at(d, deltas + lo * 2)?;
        let range_at = ranges + lo * 2;
        let range = usize::from(u16_at(d, range_at)?);
        if range == 0 {
            return Some(code.wrapping_add(delta));
        }
        let at = range_at + range + usize::from(code - start) * 2;
        let glyph = u16_at(d, at)?;
        (glyph != 0).then(|| glyph.wrapping_add(delta))
    }

    /// How far the pen moves after `glyph`, in font units.
    pub(super) fn advance(&self, glyph: u16) -> f32 {
        let index = glyph.min(self.num_h_metrics - 1);
        u16_at(self.data, self.hmtx + usize::from(index) * 4).map_or(0.0, f32::from)
    }

    /// The top of `glyph`'s box, in font units: the cap height when asked of a digit.
    pub(super) fn top(&self, glyph: u16) -> Option<f32> {
        let (start, end) = self.glyph_range(glyph)?;
        (end > start)
            .then(|| i16_at(self.data, start + 8))
            .flatten()
            .map(f32::from)
    }

    /// Where `glyph`'s outline lies inside `glyf`.
    fn glyph_range(&self, glyph: u16) -> Option<(usize, usize)> {
        let i = usize::from(glyph);
        let (start, end) = if self.long_loca {
            (
                u32_at(self.data, self.loca + i * 4)? as usize,
                u32_at(self.data, self.loca + i * 4 + 4)? as usize,
            )
        } else {
            (
                usize::from(u16_at(self.data, self.loca + i * 2)?) * 2,
                usize::from(u16_at(self.data, self.loca + i * 2 + 2)?) * 2,
            )
        };
        let (start, end) = (self.glyf + start, self.glyf + end);
        (start <= end && end <= self.data.len()).then_some((start, end))
    }

    /// `glyph`'s outline in font units. Empty for a blank glyph (a space) or a damaged one.
    pub(super) fn outline(&self, glyph: u16) -> Vec<Segment> {
        let mut out = Vec::new();
        self.append_outline(glyph, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut out);
        out
    }

    /// Append `glyph` under the affine `m` (`[a, b, c, d, dx, dy]`: x' = a·x + c·y + dx,
    /// y' = b·x + d·y + dy).
    fn append_outline(&self, glyph: u16, m: [f32; 6], depth: u32, out: &mut Vec<Segment>) {
        let Some((start, end)) = self.glyph_range(glyph) else {
            return;
        };
        if end <= start {
            return;
        }
        let Some(contours) = i16_at(self.data, start) else {
            return;
        };
        if contours >= 0 {
            if let Some(points) = self.simple_points(start, contours as usize) {
                for contour in points {
                    contour_segments(&contour, m, out);
                }
            }
        } else if depth < MAX_NESTING {
            self.append_components(start + 10, m, depth, out);
        }
    }

    /// The contours of a simple glyph: each point with whether it lies on the curve.
    fn simple_points(&self, start: usize, contours: usize) -> Option<Vec<Vec<(Point, bool)>>> {
        let d = self.data;
        let mut ends = Vec::with_capacity(contours);
        for i in 0..contours {
            ends.push(usize::from(u16_at(d, start + 10 + i * 2)?));
        }
        let count = ends.last().map_or(0, |last| last + 1);
        let instructions = start + 10 + contours * 2;
        let mut at = instructions + 2 + usize::from(u16_at(d, instructions)?);
        let mut flags = Vec::with_capacity(count);
        while flags.len() < count {
            let flag = u8_at(d, at)?;
            at += 1;
            let mut repeat = 1;
            if flag & 0x08 != 0 {
                repeat += usize::from(u8_at(d, at)?);
                at += 1;
            }
            for _ in 0..repeat.min(count - flags.len()) {
                flags.push(flag);
            }
        }
        let mut xs = Vec::with_capacity(count);
        let mut x = 0i32;
        for &flag in &flags {
            x += coordinate(d, &mut at, flag, 0x02, 0x10)?;
            xs.push(x);
        }
        let mut points = Vec::with_capacity(count);
        let mut y = 0i32;
        for (i, &flag) in flags.iter().enumerate() {
            y += coordinate(d, &mut at, flag, 0x04, 0x20)?;
            points.push(((xs[i] as f32, y as f32), flag & 0x01 != 0));
        }
        let mut out = Vec::with_capacity(contours);
        let mut from = 0;
        for end in ends {
            if end < from || end >= points.len() {
                return None;
            }
            out.push(points[from..=end].to_vec());
            from = end + 1;
        }
        Some(out)
    }

    /// A composite glyph: other glyphs placed by offsets and an optional scale.
    fn append_components(&self, mut at: usize, m: [f32; 6], depth: u32, out: &mut Vec<Segment>) {
        let d = self.data;
        loop {
            let (Some(flags), Some(glyph)) = (u16_at(d, at), u16_at(d, at + 2)) else {
                return;
            };
            at += 4;
            let words = flags & 0x0001 != 0;
            let (dx, dy) = if words {
                let pair = (i16_at(d, at), i16_at(d, at + 2));
                at += 4;
                match pair {
                    (Some(x), Some(y)) => (f32::from(x), f32::from(y)),
                    _ => return,
                }
            } else {
                let pair = (u8_at(d, at), u8_at(d, at + 1));
                at += 2;
                match pair {
                    (Some(x), Some(y)) => (f32::from(x as i8), f32::from(y as i8)),
                    _ => return,
                }
            };
            // Point-matched placement (args are point numbers) is not followed: placed at 0.
            let (dx, dy) = if flags & 0x0002 != 0 {
                (dx, dy)
            } else {
                (0.0, 0.0)
            };
            let mut part = [1.0, 0.0, 0.0, 1.0, dx, dy];
            if flags & 0x0008 != 0 {
                let Some(s) = f2dot14_at(d, at) else { return };
                at += 2;
                part[0] = s;
                part[3] = s;
            } else if flags & 0x0040 != 0 {
                let (Some(sx), Some(sy)) = (f2dot14_at(d, at), f2dot14_at(d, at + 2)) else {
                    return;
                };
                at += 4;
                part[0] = sx;
                part[3] = sy;
            } else if flags & 0x0080 != 0 {
                let values: Option<Vec<f32>> = (0..4).map(|k| f2dot14_at(d, at + k * 2)).collect();
                let Some(v) = values else { return };
                at += 8;
                part[..4].copy_from_slice(&v);
            }
            self.append_outline(glyph, compose(m, part), depth + 1, out);
            if flags & 0x0020 == 0 {
                return;
            }
        }
    }
}

/// One coordinate delta: a byte with its sign in the flag, a repeat of the previous value, or a
/// signed word.
fn coordinate(d: &[u8], at: &mut usize, flag: u8, short: u8, same: u8) -> Option<i32> {
    if flag & short != 0 {
        let v = i32::from(u8_at(d, *at)?);
        *at += 1;
        Some(if flag & same != 0 { v } else { -v })
    } else if flag & same != 0 {
        Some(0)
    } else {
        let v = i32::from(i16_at(d, *at)?);
        *at += 2;
        Some(v)
    }
}

/// `outer` applied after `inner`.
fn compose(outer: [f32; 6], inner: [f32; 6]) -> [f32; 6] {
    let [a, b, c, d, e, f] = outer;
    let [p, q, r, s, t, u] = inner;
    [
        a * p + c * q,
        b * p + d * q,
        a * r + c * s,
        b * r + d * s,
        a * t + c * u + e,
        b * t + d * u + f,
    ]
}

fn apply(m: [f32; 6], (x, y): Point) -> Point {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// One closed contour as segments. Two off-curve points in a row imply an on-curve point midway;
/// a contour may start off the curve, so the walk starts at an on-curve point, real or implied.
fn contour_segments(points: &[(Point, bool)], m: [f32; 6], out: &mut Vec<Segment>) {
    let n = points.len();
    if n < 2 {
        return;
    }
    let mid = |a: Point, b: Point| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let first_on = points.iter().position(|p| p.1);
    let start = match first_on {
        Some(i) => points[i].0,
        None => mid(points[0].0, points[1].0),
    };
    let begin = first_on.unwrap_or(0);
    let mut pen = start;
    let mut control: Option<Point> = None;
    for k in 1..=n {
        let (point, on) = points[(begin + k) % n];
        match (on, control) {
            (true, None) => {
                out.push(Segment::Line(apply(m, pen), apply(m, point)));
                pen = point;
            }
            (true, Some(c)) => {
                out.push(Segment::Quad(apply(m, pen), apply(m, c), apply(m, point)));
                pen = point;
                control = None;
            }
            (false, None) => control = Some(point),
            (false, Some(c)) => {
                let between = mid(c, point);
                out.push(Segment::Quad(apply(m, pen), apply(m, c), apply(m, between)));
                pen = between;
                control = Some(point);
            }
        }
    }
    // Back to where the walk began.
    match control {
        Some(c) => out.push(Segment::Quad(apply(m, pen), apply(m, c), apply(m, start))),
        None if pen != start => out.push(Segment::Line(apply(m, pen), apply(m, start))),
        None => {}
    }
}
