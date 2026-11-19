//! What one pane DREW of its auto figures, kept beside the userdata it went into.
//!
//! ## Why the auto lines own a segment of the userdata
//!
//! The userdata buffers are rebuilt by the order sync, which needs the session — and the session is
//! not in hand on the frame path, where a price change is first seen. A level that price crossed has
//! to change side (and colour) and may leave the kept set; an event mark has to appear when price
//! approaches. Waiting for the next order revision for that would leave the picture wrong for as
//! long as the market is quiet.
//!
//! So the auto geometry is appended LAST, after every other layer, and the buffer lengths before it
//! are recorded ([`SegmentBase`]). A price-driven change then truncates the retained buffers to
//! that base, appends the new auto geometry and re-uploads — the orders, figures, news, trade arrows
//! and warning badges in front of it are left exactly as built, with their indices (the trade-hover
//! patch writes by index) untouched. A full rebuild re-records the base.
//!
//! ## What is kept per line
//!
//! [`AutoLineRec`]: the line's geometry and everything its caption and hover card print, the
//! figure-tool values it was drawn as (for hit-testing and for pinning a copy into the store), and
//! its caption text, built on revisions by [`refresh_auto_captions`] — the text pass only places it.

use std::hash::Hasher;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use moon_chart::auto_figures::draw::{
    ResolvedStyle, append_shapes, drawn_side, pin_figures, time_span,
};
use moon_chart::auto_figures::events::EventSet;
use moon_chart::auto_figures::{Found, FoundShape, LineTag, PairSide};
use moon_chart::figures::{FigureBand, FigureBuffers};
use moon_chart::layers::{MARKER_SHAPE_KNOT, MarkerInstance};
use moon_core::config::ChartTheme;
use moon_core::config::auto_figures::{CaptionPlace, CaptionShow, LineCaption, TfId};
use moon_core::figures::proj::PxPoint;
use moon_core::figures::{Figure, Proj};
use moon_core::session::CoreId;

use super::caption::{CaptionValues, RowTitle, caption_quantum, caption_text};
use super::select::LineKey;
use super::{AutoPaneFigures, resolve_rgb};

/// Half size of an event mark, logical px before pixels-per-point: a dot a little larger than the
/// line is thick, so it reads as "something happens here" without covering candles.
const EVENT_MARK_SIZE: f32 = 3.5;
/// Outline thickness of the event mark, logical px.
const EVENT_MARK_THICKNESS: f32 = 1.5;

/// Lengths of the four userdata buffers before the auto geometry was appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SegmentBase {
    pub zones: usize,
    pub hlines: usize,
    pub segs: usize,
    pub markers: usize,
}

/// One drawn auto line.
#[derive(Debug, Clone)]
pub(crate) struct AutoLineRec {
    pub key: LineKey,
    pub shape: FoundShape,
    /// For a two-line formation, the side it was kept by (`KeptLine::side`): the one its caption is
    /// placed on and prices; `None` for every other shape.
    pub side: Option<PairSide>,
    /// The drawn stretch, Unix ms, `±∞` for "to that plot edge" — where the caption may sit. For a
    /// two-line formation, the stretch its kept side is drawn over (`draw::drawn_side`): a
    /// converging side ends at its apex or the run's reference time, not at the plot edge.
    pub span: (f64, f64),
    pub touches: usize,
    pub tf: TfId,
    pub first_ms: f64,
    pub events: EventSet,
    /// `0xRRGGBB`, the line's own resolved colour.
    pub color: u32,
    pub caption: LineCaption,
    pub title: RowTitle,
    /// What the line is, where its detector draws several kinds (`Found::tag`).
    pub tag: Option<LineTag>,
    /// Which of [`AutoPaneDrawn::figures`] draw this line.
    figs: Range<usize>,
    /// The caption, built by [`refresh_auto_captions`]; `None` when it prints nothing.
    pub text: Option<Arc<str>>,
}

impl AutoLineRec {
    /// Whether the caption is drawn only while the pointer is over the line.
    pub(crate) fn hover_only(&self) -> bool {
        self.caption.show == CaptionShow::OnHover
    }

    /// Where the caption sits.
    pub(crate) fn place(&self) -> CaptionPlace {
        self.caption.place
    }
}

/// One pane's drawn auto figures.
#[derive(Debug, Default)]
pub(crate) struct AutoPaneDrawn {
    /// Where the auto segment starts in the retained userdata, set by a full rebuild that drew a
    /// pane with a live core; `None` otherwise, which also disables the partial redraw.
    pub segment: Option<SegmentBase>,
    /// The pane target the segment was built for. A pane re-stamped with another market keeps
    /// its old userdata until the next full rebuild, and must not redraw a segment for it.
    pub target: Option<(CoreId, String)>,
    /// The epoch the segment was built against — the one the rest of the buffer uses.
    pub epoch_ms: f64,
    /// The published set this pane last drew. Held, so comparing pointers is sound.
    pub set: Option<Rc<AutoPaneFigures>>,
    pub lines: Vec<AutoLineRec>,
    /// The figure-tool values the lines were drawn as.
    figures: Vec<Figure>,
    /// What the captions in `lines` were built from; `0` forces a rebuild.
    caption_sig: u64,
}

impl AutoPaneDrawn {
    /// Forget everything drawn: the pane's core went away or it shows another market.
    pub(crate) fn clear(&mut self) {
        self.segment = None;
        self.target = None;
        self.set = None;
        self.lines.clear();
        self.figures.clear();
        self.caption_sig = 0;
    }

    /// Whether the segment was built for `core` / `market`, so the price pass may redraw it.
    pub(crate) fn redrawable_for(&self, core: Option<CoreId>, market: &str) -> bool {
        self.segment.is_some()
            && self
                .target
                .as_ref()
                .is_some_and(|(c, m)| Some(*c) == core && m == market)
    }

    /// Whether this pane draws exactly `set`.
    pub(crate) fn draws(&self, set: Option<&Rc<AutoPaneFigures>>) -> bool {
        match (&self.set, set) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// The drawn line nearest `pos` within `threshold` pixels, if any.
    pub(crate) fn line_at(&self, pos: PxPoint, proj: &dyn Proj, threshold: f32) -> Option<LineKey> {
        let mut best: Option<(LineKey, f32)> = None;
        for line in &self.lines {
            for fig in self.figures.get(line.figs.clone()).unwrap_or_default() {
                let d = fig.kind.shape().hit(pos, proj);
                if d <= threshold && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((line.key, d));
                }
            }
        }
        best.map(|(key, _)| key)
    }

    /// The drawn line named `key`.
    pub(crate) fn line(&self, key: LineKey) -> Option<&AutoLineRec> {
        self.lines.iter().find(|l| l.key == key)
    }

    /// The figure-tool values pinning line `key` copies into the store
    /// (`moon_chart::auto_figures::draw::pin_figures`): one per side for a two-line formation —
    /// two levels or two segments — and what the line is drawn as for every other shape.
    ///
    /// Rebuilt from the published figure the line was drawn from, with the row's style, the line's
    /// own colour and the run's reference time, so a pinned pair is the pair on screen. Empty when
    /// the line, or the figure it came from, is no longer in the published set.
    pub(crate) fn line_figures(&self, key: LineKey) -> Vec<Figure> {
        let Some(line) = self.line(key) else {
            return Vec::new();
        };
        let drawn = || {
            self.figures
                .get(line.figs.clone())
                .map(<[Figure]>::to_vec)
                .unwrap_or_default()
        };
        if !matches!(line.shape, FoundShape::Pair { .. }) {
            return drawn();
        }
        let Some((row, found)) = self.set.as_ref().and_then(|set| {
            set.rows
                .iter()
                .filter(|r| r.row_ix == key.row_ix)
                .find_map(|r| {
                    r.outcome
                        .found
                        .iter()
                        .find(|f| LineKey::of(r.row_ix, f) == key)
                        .map(|f| (r, f))
                })
        }) else {
            return Vec::new();
        };
        let rgb = [
            (line.color >> 16) as u8,
            (line.color >> 8) as u8,
            line.color as u8,
        ];
        let mut out = Vec::new();
        pin_figures(
            found,
            &ResolvedStyle::new(&row.style, rgb),
            row.ref_ms,
            &mut out,
        );
        out
    }
}

/// Pack an `[r, g, b]` triple into `0xRRGGBB`.
fn rgb_u32(rgb: [u8; 3]) -> u32 {
    (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2])
}

/// Where a line's event mark sits: at the close of the last closed bar of its timeframe — the
/// moment the bar events are about, and close to the live edge for the approach — on the line, or
/// for a two-line formation on the side it was kept by (`side`).
fn event_mark_at(
    found: &Found,
    side: Option<PairSide>,
    last_closed_ms: i64,
    tf: TfId,
) -> Option<(f64, f64)> {
    let t = (last_closed_ms + tf.ms()) as f64;
    Some((t, found.shape.side_price_at(t, side)?))
}

/// Append `set`'s geometry to `out` and record what was drawn in `drawn`.
///
/// Called by the full userdata rebuild and by the partial one alike, so both draw the same thing.
/// Captions are reset here and rebuilt by the caller's [`refresh_auto_captions`].
///
/// Counter: `autofigures_lines` moves by the lines appended.
pub(crate) fn append_auto_geometry(
    set: Option<&Rc<AutoPaneFigures>>,
    theme: &ChartTheme,
    ppp: f32,
    epoch_ms: f64,
    out: &mut FigureBuffers<'_>,
    drawn: &mut AutoPaneDrawn,
) {
    drawn.set = set.cloned();
    drawn.lines.clear();
    drawn.figures.clear();
    drawn.caption_sig = 0;
    let Some(set) = set else {
        return;
    };
    let mut bands: Vec<FigureBand> = Vec::new();
    let mut marks: Vec<MarkerInstance> = Vec::new();
    for row in &set.rows {
        for kept in &row.kept {
            let Some(found) = row.outcome.found.get(kept.ix) else {
                continue;
            };
            let rgb = resolve_rgb(row.style.color, kept.above, theme);
            let start = drawn.figures.len();
            // The zone band is the tolerance the detector reports with the figure.
            append_shapes(
                found,
                &ResolvedStyle::new(&row.style, rgb),
                row.ref_ms,
                &mut drawn.figures,
                &mut bands,
            );
            let tf = row.outcome.key.tf;
            if !kept.events.is_empty()
                && let Some((t, price)) =
                    event_mark_at(found, kept.side, row.outcome.key.last_closed_ms, tf)
            {
                let c = |v: u8| f32::from(v) / 255.0;
                marks.push(MarkerInstance::at_price(
                    (t - epoch_ms) as f32,
                    price as f32,
                    EVENT_MARK_SIZE * ppp,
                    EVENT_MARK_THICKNESS * ppp,
                    MARKER_SHAPE_KNOT,
                    [c(rgb[0]), c(rgb[1]), c(rgb[2]), 1.0],
                ));
            }
            drawn.lines.push(AutoLineRec {
                key: LineKey::of(row.row_ix, found),
                shape: found.shape,
                side: kept.side,
                span: line_span(found, kept.side, row),
                touches: found.touches.len(),
                tf,
                first_ms: found.first_ms,
                events: kept.events,
                color: rgb_u32(rgb),
                caption: row.caption.clone(),
                title: row.title.clone(),
                tag: found.tag,
                figs: start..drawn.figures.len(),
                text: None,
            });
        }
    }
    crate::diag::bump_by(&crate::diag::AUTOFIGURES_LINES, drawn.lines.len() as u64);
    if !drawn.figures.is_empty() || !bands.is_empty() {
        moon_chart::figures::build_passive_figure_geometry(&drawn.figures, &bands, epoch_ms, out);
    }
    // After the lines, so a mark sits on top of the line it marks.
    out.markers.extend_from_slice(&marks);
}

/// The stretch `found` is drawn over: for a two-line formation the one its `side` (the upper when
/// none was kept) is drawn over, through the same decision that drew it; for every other shape the
/// span its row's `extend` gives it.
fn line_span(found: &Found, side: Option<PairSide>, row: &super::AutoRowDrawn) -> (f64, f64) {
    let side = side.unwrap_or(PairSide::Upper);
    drawn_side(found, side, row.style.extend, row.ref_ms)
        .map_or_else(|| time_span(found, row.style.extend), |d| d.span())
}

/// The moment `line`'s printed price is read at: `now_ms`, clamped into the line's drawn stretch.
///
/// A line drawn to the plot edge is read now. A line that ENDS — a formation's side capped at its
/// apex, a segment between two touches — is read at its end once now is past it: the caption sits
/// on the drawn line and prints the price where the line is drawn, not a price extrapolated past
/// the end (where a triangle's sides claim prices the pattern never did). That is also the point
/// the right-end caption's height is read at, so the printed value and the caption's place agree.
fn price_ms(line: &AutoLineRec, now_ms: f64) -> f64 {
    let (lo, hi) = line.span;
    if now_ms > hi {
        hi
    } else if now_ms < lo {
        lo
    } else {
        now_ms
    }
}

/// Everything `line`'s caption prints, at `now_ms` against `price`.
fn caption_values(
    line: &AutoLineRec,
    price: Option<f64>,
    now_ms: f64,
    tick: Option<f64>,
) -> CaptionValues<'_> {
    CaptionValues {
        title: &line.title,
        tag: line.tag,
        line_now: line.shape.side_price_at(price_ms(line, now_ms), line.side),
        price,
        touches: line.touches,
        tf: line.tf,
        first_ms: line.first_ms,
        events: line.events,
        now_ms,
        tick,
    }
}

/// Rebuild the captions of `drawn` when what they print moved.
///
/// Cheap when nothing did: one hash over the printed quanta of every line, no string. Called on the
/// revisions that can move a printed value — a rebuild of the lines, a price change, the once-a-
/// second age clock — never per frame.
///
/// Returns whether any caption text changed.
pub(crate) fn refresh_auto_captions(
    drawn: &mut AutoPaneDrawn,
    price: Option<f64>,
    now_ms: f64,
    tick: Option<f64>,
) -> bool {
    if drawn.lines.is_empty() {
        return false;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(rust_i18n::locale().as_bytes());
    for line in &drawn.lines {
        caption_quantum(
            &line.caption.parts,
            &caption_values(line, price, now_ms, tick),
            &mut h,
        );
    }
    // Never 0, which is the "rebuild" sentinel.
    let sig = h.finish().max(1);
    if sig == drawn.caption_sig {
        return false;
    }
    drawn.caption_sig = sig;
    let mut changed = false;
    for i in 0..drawn.lines.len() {
        let line = &drawn.lines[i];
        let text = caption_text(
            &line.caption.parts,
            &caption_values(line, price, now_ms, tick),
        );
        let next = (!text.is_empty()).then(|| Arc::<str>::from(text));
        if drawn.lines[i].text != next {
            drawn.lines[i].text = next;
            changed = true;
        }
    }
    changed
}
