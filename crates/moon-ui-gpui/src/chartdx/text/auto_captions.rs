//! The captions on auto lines: placed here, built elsewhere.
//!
//! The strings are made on revisions by `auto_figures_sync::pane::refresh_auto_captions` — a
//! rebuild of the lines, a price change that moved a printed digit, the age clock — so this pass,
//! which runs on every presented frame, does arithmetic only: where the line's drawn stretch is on
//! screen, the line's price there, which side of it the text goes, whether lines of several rows
//! share one price and so one caption ([`group_same_price`]), and whether a caption has to step
//! aside — for the order captions in the price column, and for the auto captions already drawn.
//!
//! The placement itself is pure ([`caption_spot`], [`group_same_price`], [`clear_of`]) so it is
//! tested without a text context; [`RenderState::draw_auto_captions`] only measures and draws what
//! they decide. The one string a frame can need beyond the prepared ones — the merged caption of a
//! shared price — is joined into a buffer the pass keeps, so a still picture allocates nothing.

use moon_chart::auto_figures::{FoundShape, PairSide};
use moon_core::config::auto_figures::CaptionPlace;

use super::caption::order_rows::OrderCaption;
use super::*;

#[cfg(test)]
mod tests;

/// One frame's mapping of a pane from time and price to logical pixels — the arithmetic the lines
/// are drawn by, so a caption lands on its line.
#[derive(Debug, Clone, Copy)]
pub(super) struct CaptionMap {
    /// The plot's `[left, top, right, bottom]`, logical pixels.
    pub plot: [f32; 4],
    /// Logical-to-physical factor of the text layer.
    pub sf: f32,
    /// The pane's epoch, Unix ms: what the view's times are relative to.
    pub epoch_ms: f64,
    /// The view's left edge, ms after the epoch.
    pub view_time0: f32,
    /// Logical pixels per millisecond.
    pub time_to_px: f32,
    /// The price at the plot's bottom edge.
    pub view_price0: f32,
    /// PHYSICAL pixels per unit of price (the view's own factor).
    pub price_to_px: f32,
}

impl CaptionMap {
    /// The logical x of the Unix time `t_ms`.
    fn x_of(&self, t_ms: f64) -> f32 {
        self.plot[0] + ((t_ms - self.epoch_ms) as f32 - self.view_time0) * self.time_to_px
    }

    /// The Unix time at logical x.
    fn t_at(&self, x: f32) -> f64 {
        self.epoch_ms + f64::from(self.view_time0) + f64::from((x - self.plot[0]) / self.time_to_px)
    }

    /// The logical y of `price`, snapped to a physical pixel like the lines.
    fn y_of(&self, price: f32) -> f32 {
        ((self.plot[3] * self.sf) - (price - self.view_price0) * self.price_to_px).round() / self.sf
    }
}

/// Where one caption goes, before its text is measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CaptionSpot {
    /// The text anchor's x, logical pixels.
    pub x: f32,
    /// Horizontal alignment of the text about `x`: `0` left, `0.5` centre, `1` right.
    pub ax: f32,
    /// The y of the line at the point the caption names, logical pixels.
    pub line_y: f32,
    /// The on-screen part of the drawn stretch, `[x_lo, x_hi]` in logical pixels: how far a
    /// caption sliding aside for the order captions may go and still sit on its own line.
    pub drawn: [f32; 2],
}

/// Where a caption of `place` goes on a line of `shape` drawn over `span` (Unix ms, `±∞` = to
/// that plot edge), reading the price of `side` for a two-line formation; `None` when the drawn
/// stretch is off screen or the line's price there is outside the plot.
///
/// The price is read at the point the caption names — the visible left or right end of the drawn
/// stretch, or its middle — so a caption at the right end of a triangle's side sits at the end of
/// the drawn segment (its apex or the run's reference time, `draw::drawn_side`), not at the plot
/// edge where the line would be extrapolated.
///
/// Args:
///     map: The frame's pane mapping.
///     shape: The line's geometry.
///     side: The formation side the line was kept by; `None` for every other shape.
///     span: The drawn stretch.
///     place: The caption's place.
///     label_x: The right edge of the price-label column, where `AtPriceAxis` captions end.
pub(super) fn caption_spot(
    map: &CaptionMap,
    shape: &FoundShape,
    side: Option<PairSide>,
    span: (f64, f64),
    place: CaptionPlace,
    label_x: f32,
) -> Option<CaptionSpot> {
    let [plot_left, plot_top, plot_right, plot_bottom] = map.plot;
    // The part of the drawn stretch that is on screen.
    let x_lo = if span.0.is_finite() {
        map.x_of(span.0).max(plot_left)
    } else {
        plot_left
    };
    let x_hi = if span.1.is_finite() {
        map.x_of(span.1).min(plot_right)
    } else {
        plot_right
    };
    // `!(>)` also turns a NaN end away.
    if !(x_hi > x_lo) {
        return None;
    }
    // (text anchor x, horizontal alignment, where on the line the price is read).
    let (x, ax, at_x) = match place {
        CaptionPlace::LeftEnd => (x_lo + READOUT_PAD_X, 0.0, x_lo),
        CaptionPlace::RightEnd => (x_hi - READOUT_PAD_X, 1.0, x_hi),
        // The label column the order labels use, read where the drawn line ends on screen.
        CaptionPlace::AtPriceAxis => (label_x, 1.0, x_hi),
        CaptionPlace::AboveLine | CaptionPlace::BelowLine => {
            let mid = (x_lo + x_hi) * 0.5;
            (mid, 0.5, mid)
        }
    };
    // A formation's caption sits on the side it was kept by, the one it prices.
    let price = shape.side_price_at(map.t_at(at_x), side)?;
    let line_y = map.y_of(price as f32);
    if !(plot_top..=plot_bottom).contains(&line_y) {
        return None;
    }
    Some(CaptionSpot {
        x,
        ax,
        line_y,
        drawn: [x_lo, x_hi],
    })
}

/// The y of `shape`'s line (its `side` for a formation) under logical x `x`, clamped into the
/// drawn stretch `drawn`; `None` when that price is unusable or falls outside the plot.
///
/// What keeps a caption that slid LEFT on its line: a sloped line's price changes along the slide,
/// so the caption's height is read again where it now ends instead of being carried over.
fn line_y_at(
    map: &CaptionMap,
    shape: &FoundShape,
    side: Option<PairSide>,
    x: f32,
    drawn: [f32; 2],
) -> Option<f32> {
    let at_x = x.clamp(drawn[0], drawn[1]);
    let price = shape.side_price_at(map.t_at(at_x), side)?;
    let y = map.y_of(price as f32);
    (map.plot[1]..=map.plot[3]).contains(&y).then_some(y)
}

/// Two captions of DIFFERENT rows whose lines sit at most this far apart on screen, logical
/// pixels, are one caption ([`group_same_price`]).
///
/// One and a half: a line is snapped to a whole physical pixel ([`CaptionMap::y_of`]), so two
/// lines at one price can still land a pixel apart, and two lines two pixels apart already read as
/// two lines — while their captions, a text line tall each, would sit on top of each other.
pub(super) const SAME_PRICE_PX: f32 = 1.5;

/// A padded caption box another caption has to clear: an order caption in the price column, or
/// an auto caption this frame already drew.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ClearBox {
    /// Padded horizontal extent, logical pixels.
    pub x: [f32; 2],
    /// Top of the text.
    pub top: f32,
    /// Height from `top`: the text line, with an order caption's plate insets where it has them.
    pub height: f32,
}

impl ClearBox {
    /// An order caption's box, `None` while it is hidden (its `top` is `None`).
    pub(super) fn of_order(o: &OrderCaption) -> Option<Self> {
        Some(Self {
            x: o.x,
            top: o.top?,
            height: o.height,
        })
    }

    /// The box of a caption drawn `w` wide and `h` tall with its anchor at `x`, aligned `ax`
    /// about it (`0` left, `1` right), its text top at `top` — padded the way an order caption's
    /// is.
    pub(super) fn drawn(x: f32, ax: f32, w: f32, top: f32, h: f32) -> Self {
        Self {
            x: [
                x - ax * w - READOUT_PAD_X,
                x + (1.0 - ax) * w + READOUT_PAD_X,
            ],
            top,
            height: h,
        }
    }
}

/// One line's caption in this frame, before it is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CaptionCand {
    /// Index of the line in the pane's drawn lines.
    pub li: usize,
    /// The row the line belongs to.
    pub row: usize,
    pub place: CaptionPlace,
    pub spot: CaptionSpot,
    /// Whether the line's own caption shows now: always, or `OnHover` with the pointer on it.
    pub shown: bool,
    /// Index into the candidates of the caption this one is printed in: its own for a caption of
    /// its own (and for the first of a shared price), the first one's for every other line at that
    /// price ([`group_same_price`]).
    pub lead: usize,
}

/// Decide which candidates share one caption: a candidate joins the earliest caption before it
/// whose line sits within [`SAME_PRICE_PX`] of its own, at the same place, and which holds no line
/// of its row yet; otherwise it leads a caption of its own.
///
/// Lines of ONE row at one price are left apart: a row's two lines are two answers of its own
/// detector, and the caption steps aside for the other instead ([`clear_of`]). The candidates come
/// in draw order — the rows' order — so the first row of a shared price leads it, gives it its
/// colour and its name first, and the same picture always groups the same way.
pub(super) fn group_same_price(cands: &mut [CaptionCand]) {
    for i in 0..cands.len() {
        cands[i].lead = i;
        let (row, place, y) = (cands[i].row, cands[i].place, cands[i].spot.line_y);
        let joined = (0..i).find(|&j| {
            let lead = &cands[j];
            lead.lead == j
                && lead.place == place
                && (lead.spot.line_y - y).abs() <= SAME_PRICE_PX
                && !cands[..i].iter().any(|c| c.lead == j && c.row == row)
        });
        if let Some(j) = joined {
            cands[i].lead = j;
        }
    }
}

/// Where a caption ends once it is clear of `boxes`: its right edge `x` when nothing is in the
/// way, else slid LEFT along its line until its padded box meets none of them; `None` when, after
/// sliding, the text would start left of `min_left`. A caption that needed no slide keeps its place
/// whatever `min_left` says: a box elsewhere in its row is no reason to drop a caption that was
/// drawn there before.
///
/// The boxes are this frame's order captions and the auto captions drawn before this one. Why
/// sliding left rather than the alternatives: moving the text up or down puts it beside a
/// DIFFERENT price, where it names the wrong line; shortening it to the price still covers the
/// other caption's numbers. Along its own line — the caller reads its height again where it ends,
/// so on a sloped line it follows the slope — the caption still reads as that line's. The order
/// captions keep the column: they carry the user's live orders; an auto caption drawn earlier keeps
/// its place: it belongs to an earlier row.
///
/// Deterministic, so it cannot flicker on a still picture: the answer depends only on this frame's
/// boxes, and each step moves the box strictly left of a box it met, so it ends within
/// `boxes.len()` steps.
///
/// Args:
///     x: The caption's right edge, logical pixels.
///     w: The caption's text width.
///     top: The top of its text.
///     h: Its text line height.
///     boxes: What it must not cover.
///     min_left: The leftmost x its text may start at once it slid: where its line's drawn
///         stretch starts on screen, so it never hangs left of its own line.
pub(super) fn clear_of(
    x: f32,
    w: f32,
    top: f32,
    h: f32,
    boxes: &[ClearBox],
    min_left: f32,
) -> Option<f32> {
    // The same padded box an order caption is laid out by.
    let (y0, y1) = (top - READOUT_PAD_Y, top + h + READOUT_PAD_Y);
    let mut right = x;
    let mut slid = false;
    for _ in 0..=boxes.len() {
        let (x0, x1) = (right - w - READOUT_PAD_X, right + READOUT_PAD_X);
        let block = boxes
            .iter()
            .filter(|b| b.x[0] < x1 && x0 < b.x[1] && b.top < y1 && y0 < b.top + b.height)
            .map(|b| b.x[0])
            .reduce(f32::min);
        let Some(block) = block else {
            return (!slid || right - w >= min_left).then_some(right);
        };
        right = block - READOUT_PAD_X;
        slid = true;
    }
    None
}

/// Whether any of `boxes` shares the rows from `top` to `top + h` — the cheap test that decides
/// whether a caption is worth measuring for [`clear_of`].
fn row_blocked(top: f32, h: f32, boxes: &[ClearBox]) -> bool {
    let (y0, y1) = (top - READOUT_PAD_Y, top + h + READOUT_PAD_Y);
    boxes.iter().any(|b| b.top < y1 && y0 < b.top + b.height)
}

/// The text pass's reused buffers for the auto captions, kept between frames so a still picture
/// allocates nothing once they have grown.
#[derive(Debug, Default)]
pub(in crate::chartdx) struct AutoCaptionScratch {
    cands: Vec<CaptionCand>,
    boxes: Vec<ClearBox>,
    /// The merged caption of a shared price, joined here rather than into a new `String`.
    text: String,
}

impl RenderState {
    /// Draw pane `idx`'s auto-line captions.
    ///
    /// An `OnHover` caption is drawn only for the line under the pointer (`auto_hover`). A line
    /// whose drawn stretch is off screen, or whose price at the anchor is outside the plot, prints
    /// nothing. Lines of different rows at one price print ONE caption ([`group_same_price`]): the
    /// names of all of them, then the first one's other parts (`Double top · Day high · 5.3000`),
    /// in the first one's colour — shown while any of them would show its own. Every caption steps
    /// aside ([`clear_of`]) for this frame's order captions — the pane's `order_caption_scratch`,
    /// laid out earlier in the same pass whenever line labels are on — and for the auto captions
    /// drawn before it.
    ///
    /// Args:
    ///     ctx: Text context of the frame.
    ///     idx: Pane index.
    ///     sf: Logical-to-physical factor of the text layer.
    ///     plot: The plot's `[left, top, right, bottom]` in logical pixels.
    ///     label_x: The right edge of the price-label column, where `AtPriceAxis` captions end.
    ///     placed: The frame's label layout.
    pub(super) fn draw_auto_captions(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
        plot: [f32; 4],
        label_x: f32,
        placed: &mut Vec<PlacedLabel>,
    ) -> anyhow::Result<()> {
        let Some(pr) = self.panes.get(idx) else {
            return Ok(());
        };
        if pr.auto.lines.is_empty() {
            return Ok(());
        }
        let view = pr.view;
        let map = CaptionMap {
            plot,
            sf,
            epoch_ms: pr.epoch_ms,
            view_time0: view.view_time0,
            time_to_px: (view.time_to_px / sf).max(moon_chart::view::MIN_PX_PER_MS),
            view_price0: view.view_price0,
            price_to_px: view.price_to_px,
        };
        let hovered = self
            .auto_hover
            .filter(|(pane, _)| *pane == idx)
            .map(|(_, key)| key);
        // Taken for the pass and put back after it, so their allocations are reused next frame.
        // The order captions are this frame's only while line labels are on; the scratch is stale
        // otherwise, and nothing is in the column.
        let orders = std::mem::take(&mut self.panes[idx].order_caption_scratch);
        let mut sc = std::mem::take(&mut self.auto_caption_scratch);
        sc.cands.clear();
        sc.boxes.clear();
        if self.line_labels {
            sc.boxes
                .extend(orders.iter().filter_map(ClearBox::of_order));
        }
        for (li, line) in self.panes[idx].auto.lines.iter().enumerate() {
            if line.text.is_none() {
                continue;
            }
            let Some(spot) = caption_spot(
                &map,
                &line.shape,
                line.side,
                line.span,
                line.place(),
                label_x,
            ) else {
                continue;
            };
            sc.cands.push(CaptionCand {
                li,
                row: line.key.row_ix,
                place: line.place(),
                spot,
                shown: !line.hover_only() || hovered == Some(line.key),
                lead: li,
            });
        }
        group_same_price(&mut sc.cands);
        let line_h = label_line_px(self.label_font_delta);
        let mut result = Ok(());
        for ci in 0..sc.cands.len() {
            let cand = sc.cands[ci];
            // A line printed in an earlier caption, or a caption none of whose lines shows now.
            if cand.lead != ci || !sc.cands[ci..].iter().any(|m| m.lead == ci && m.shown) {
                continue;
            }
            // Copied out: the text is an `Arc<str>` (a refcount bump), and drawing needs `&mut self`.
            let (own, shape, side, rgb, alpha) = {
                let line = &self.panes[idx].auto.lines[cand.li];
                (
                    line.text.clone(),
                    line.shape,
                    line.side,
                    line.color,
                    line.opacity,
                )
            };
            let Some(own) = own else {
                continue;
            };
            let merged = {
                let lines = &self.panes[idx].auto.lines;
                let members = sc.cands[ci..].iter().filter(|m| m.lead == ci);
                let second = members.clone().nth(1).is_some();
                second
                    && crate::chartdx::auto_figures_sync::caption::merged_caption(
                        &mut sc.text,
                        members.filter_map(|m| lines[m.li].name.as_deref()),
                        lines[cand.li].rest.as_deref(),
                    )
            };
            let text: &str = if merged { &sc.text } else { &own };
            let spot = cand.spot;
            let offset = |line_y: f32| match cand.place {
                CaptionPlace::BelowLine => (line_y + LABEL_LINE_GAP, 0.0),
                _ => (line_y - LABEL_LINE_GAP, 1.0),
            };
            let (mut dy, ay) = offset(spot.line_y);
            let mut x = spot.x;
            let top = dy - ay * line_h;
            // Measured only when another caption shares the row: text shaping is the expensive
            // call on this path, and most captions have nothing beside them.
            if row_blocked(top, line_h, &sc.boxes) {
                let w = self.measure_label_text(ctx, text).width.as_f32();
                // The anchor's distance from the text's right edge, by its alignment.
                let tail = (1.0 - spot.ax) * w;
                let right = x + tail;
                let cleared = clear_of(right, w, top, line_h, &sc.boxes, spot.drawn[0]);
                // Slid along its line: its height is read again where its anchor now is.
                let slid_y = match cleared {
                    Some(clear) if clear != right => {
                        line_y_at(&map, &shape, side, clear - tail, spot.drawn).map(|y| (clear, y))
                    }
                    other => other.map(|clear| (clear, spot.line_y)),
                };
                let Some((clear, line_y)) = slid_y else {
                    // No room left on its own line, or its price there is off the plot. The run
                    // slot the measurement shaped this text into is left to it, so the next
                    // caption does not reshape that slot on every frame.
                    self.text_run_cursor += 1;
                    continue;
                };
                x = clear - tail;
                dy = offset(line_y).0;
            }
            match draw_label_text_run(
                &mut self.text_runs,
                &mut self.text_run_cursor,
                ctx,
                self.label_font_delta,
                text,
                x,
                dy,
                spot.ax,
                ay,
                color(rgb).opacity(alpha.clamp(0.0, 1.0)),
            ) {
                Ok(m) => {
                    let (w, h) = (m.width.as_f32(), m.line_height.as_f32());
                    sc.boxes
                        .push(ClearBox::drawn(x, spot.ax, w, dy - ay * line_h, line_h));
                    placed.push(PlacedLabel {
                        x,
                        y: dy,
                        ax: spot.ax,
                        ay,
                        w,
                        h,
                        solid: false,
                    });
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.auto_caption_scratch = sc;
        self.panes[idx].order_caption_scratch = orders;
        result
    }
}
