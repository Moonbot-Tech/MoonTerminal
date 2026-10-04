//! Area coverage of a closed shape, for everything the chart fills: letters, strokes, crosses,
//! dots. The shape is a set of straight edges in pixel coordinates (y down), filled by the
//! non-zero winding rule. Each pixel row is sampled on [`SUBROWS`] horizontal lines; along each
//! line the covered stretch is exact, so a pixel gets the share of its width the shape covers,
//! averaged over the lines. That is enough antialiasing for text of 20–30 pixels and lines of
//! two or three.

/// Sampling lines per pixel row.
const SUBROWS: usize = 5;

/// Steps a quadratic curve is cut into per pixel of its length, at most [`MAX_STEPS`].
const STEPS_PER_PX: f32 = 0.5;
const MAX_STEPS: usize = 24;

/// A straight edge, `(x0, y0, x1, y1)` in pixels.
pub(super) type Edge = (f32, f32, f32, f32);

/// Coverage of one shape: `w` × `h` values from 0 to 1, its top-left pixel at (`left`, `top`).
#[derive(Clone, Debug, Default)]
pub(super) struct Mask {
    pub(super) left: i64,
    pub(super) top: i64,
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) cover: Vec<f32>,
}

/// Collects the edges of a shape.
#[derive(Default)]
pub(super) struct Path {
    edges: Vec<Edge>,
}

impl Path {
    pub(super) fn line(&mut self, from: (f32, f32), to: (f32, f32)) {
        if from.1 != to.1 {
            self.edges.push((from.0, from.1, to.0, to.1));
        }
    }

    /// A quadratic curve, cut into straight steps.
    pub(super) fn quad(&mut self, from: (f32, f32), control: (f32, f32), to: (f32, f32)) {
        let length = dist(from, control) + dist(control, to);
        let steps = ((length * STEPS_PER_PX).ceil() as usize).clamp(1, MAX_STEPS);
        let mut prev = from;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            let next = (
                u * u * from.0 + 2.0 * u * t * control.0 + t * t * to.0,
                u * u * from.1 + 2.0 * u * t * control.1 + t * t * to.1,
            );
            self.line(prev, next);
            prev = next;
        }
    }

    /// A closed polygon.
    pub(super) fn polygon(&mut self, points: &[(f32, f32)]) {
        for (i, &p) in points.iter().enumerate() {
            self.line(p, points[(i + 1) % points.len()]);
        }
    }

    /// The coverage of everything added, or `None` for an empty or non-finite shape.
    pub(super) fn fill(&self) -> Option<Mask> {
        if self.edges.is_empty() {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(ax, ay, bx, by) in &self.edges {
            x0 = x0.min(ax).min(bx);
            x1 = x1.max(ax).max(bx);
            y0 = y0.min(ay).min(by);
            y1 = y1.max(ay).max(by);
        }
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
            return None;
        }
        let (left, top) = (x0.floor() as i64, y0.floor() as i64);
        let w = (x1.ceil() as i64 - left).max(1) as usize;
        let h = (y1.ceil() as i64 - top).max(1) as usize;
        // A shape this large is a coordinate gone wild, not something the chart draws.
        if w.saturating_mul(h) > 4_000_000 {
            return None;
        }
        let mut cover = vec![0.0f32; w * h];
        let mut crossings: Vec<(f32, i32)> = Vec::new();
        let weight = 1.0 / SUBROWS as f32;
        for row in 0..h {
            for sub in 0..SUBROWS {
                let y = top as f32 + row as f32 + (sub as f32 + 0.5) * weight;
                crossings.clear();
                for &(ax, ay, bx, by) in &self.edges {
                    let (lo, hi, dir) = if ay < by { (ay, by, 1) } else { (by, ay, -1) };
                    if y < lo || y >= hi {
                        continue;
                    }
                    let x = ax + (y - ay) / (by - ay) * (bx - ax);
                    crossings.push((x - left as f32, dir));
                }
                crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
                let line = &mut cover[row * w..(row + 1) * w];
                let mut winding = 0;
                for pair in crossings.windows(2) {
                    winding += pair[0].1;
                    if winding != 0 {
                        add_span(line, pair[0].0, pair[1].0, weight);
                    }
                }
            }
        }
        for c in &mut cover {
            *c = c.min(1.0);
        }
        Some(Mask {
            left,
            top,
            w,
            h,
            cover,
        })
    }
}

/// Add `weight` × the share of each pixel the stretch `[a, b)` covers.
fn add_span(line: &mut [f32], a: f32, b: f32, weight: f32) {
    let (a, b) = (a.max(0.0), b.min(line.len() as f32));
    if b <= a {
        return;
    }
    let (first, last) = (a.floor() as usize, (b.ceil() as usize).min(line.len()));
    for (px, value) in line.iter_mut().enumerate().take(last).skip(first) {
        let covered = (b.min(px as f32 + 1.0) - a.max(px as f32)).max(0.0);
        *value += covered * weight;
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}
