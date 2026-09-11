//! Charts of the "Summary" tab: daily bars, horizontal per-core rankings, and the
//! bucket-popup chrome these charts share with the cumulative chart. The cumulative
//! chart itself lives in `cumulative`.

use std::ops::Range;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonPalette, MoonProgress, MoonScrollAxis, MoonScrollbarVisibility, MoonVirtualList, h_flex,
    moon_scrollbar_overlay_with_palette, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{CoreSeries, DayPoint, KindStat};

pub(super) const CHART_H: f32 = 170.0;
/// Weight the chart value labels are drawn at — they set none, so it is GPUI's normal.
/// `mono_caption_text_width` needs it as a number.
pub(super) const LABEL_WEIGHT: f32 = 400.0;

/// Width of the WIDEST of a set of already-formatted value labels, in pixels.
///
/// MEASURED, never a constant. A guessed width is wrong in both directions and both are
/// expensive: too narrow and the collision passes believe two labels are clear when the digits
/// overlap — the exact defect they exist to prevent — too wide and they drop labels that would
/// have fitted. The old constants were guessed against example strings (`"-1268 USDT"`), and
/// that example was not even a string these charts produce: `pnl_suffix` is `"%"` or nothing,
/// never a ticker.
///
/// The Analytics view draws in the MONO family (`analytics/render.rs`), so a glyph advance is
/// the same for every character and the widest label is simply the longest one — picking it by
/// `chars().count()` and measuring that one string is exact, not an approximation, and costs a
/// single measurement per frame rather than one per label.
///
/// Args:
///     texts: The formatted labels this chart is about to consider drawing.
///     cx: Analytics view context.
///
/// Returns:
///     Width of the longest label in pixels, or 0.0 when there are none.
pub(super) fn widest_label_w<'a>(
    texts: impl Iterator<Item = &'a str>,
    cx: &Context<AnalyticsView>,
) -> f32 {
    texts
        .max_by_key(|s| s.chars().count())
        .map(|s| design::mono_caption_text_width(cx, s, LABEL_WEIGHT))
        .unwrap_or(0.0)
}
/// Plot width both summary charts assume, in RAW pixels — deliberately neither font- nor
/// UI-scaled.
///
/// The card's real width is a layout result these functions never learn: it is only known
/// inside the paint closure, while the labels are absolutely-positioned divs outside it, and
/// threading a measured width in would widen both chart signatures through the summary card.
/// So the pass measures against the narrowest the card realistically gets — the Analytics
/// window's own minimum (860 wide, `analytics/mod.rs`) less the page padding (2x10), the
/// inter-card gap (8) and the card's own padding (2x12), over two equal cards:
/// `(860 - 20 - 8) / 2 - 24`. At the default 1240-wide window the real plot is ~584, so
/// assuming the minimum only ever drops a label that would in fact have fitted — the safe
/// direction; assuming wide puts overlap back.
///
/// **Raw, not scaled.** 860 is a fixed physical size and every padding it is reduced by goes
/// through `ui()`, which tracks the UI scale but NOT the Font slider — so a larger font never
/// widens this plot, and a larger UI scale only eats further INTO it. Passing this through
/// `font_w_px` would have grown the room the collision pass believes it has at exactly the
/// setting that makes the labels physically wider, which is the application's default (+3).
///
/// ONE constant for BOTH charts on purpose: they are the two `flex_1` siblings of the same
/// row, with the same page padding, the same gap and the same card chrome. Two copies could
/// drift apart from a layout that cannot.
pub(super) const PLOT_W_NOMINAL: f32 = 392.0;

/// Chart identity and bucket together prevent a late dismissal from closing another popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PopupKey {
    /// One daily profit bucket.
    Daily(usize),
    /// Running totals through one bucket.
    Cumulative(usize),
    /// One strategy type.
    Kind(usize),
}

/// Pointer ownership and generation fence for delayed popup dismissal.
#[derive(Default)]
pub(in crate::analytics) struct PopupHover {
    /// Bucket currently owning the popup.
    target: Option<PopupKey>,
    /// Whether the pointer reached the popup instead of merely leaving its source column.
    over_popup: bool,
    /// Invalidates outstanding leave timers when the pointer returns or changes buckets.
    revision: u64,
}

impl PopupHover {
    /// Cancel queued actions when reloading changes what their bucket index identifies.
    pub(in crate::analytics) fn reset_for_reload(&mut self, reset_daily: bool) {
        if reset_daily || matches!(self.target, Some(PopupKey::Kind(_))) {
            self.target = None;
            self.over_popup = false;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Transfer ownership on entry and cancel any previous leave timer.
    fn enter(&mut self, key: PopupKey, over_popup: bool) {
        self.target = Some(key);
        self.over_popup = over_popup;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Begin a grace period unless this is an obsolete source or the popup still owns hover.
    fn leave(&mut self, key: PopupKey, from_popup: bool) -> Option<u64> {
        if self.target != Some(key) || (!from_popup && self.over_popup) {
            return None;
        }
        self.over_popup = false;
        self.revision = self.revision.wrapping_add(1);
        Some(self.revision)
    }

    /// Expire only the exact leave event; re-entry and bucket switches invalidate it.
    fn expire(&mut self, key: PopupKey, revision: u64) -> bool {
        if !self.is_current(key, revision) || self.over_popup {
            return false;
        }
        self.target = None;
        true
    }

    /// Accept a delayed action only while the same pointer event still owns the popup.
    fn is_current(&self, key: PopupKey, revision: u64) -> bool {
        self.target == Some(key) && self.revision == revision
    }
}

/// Keep the existing chart highlights synchronized with the popup's single pointer owner.
fn select_popup(this: &mut AnalyticsView, key: Option<PopupKey>) {
    this.hover_daily_bucket = match key {
        Some(PopupKey::Daily(i)) => Some(i),
        _ => None,
    };
    this.hover_cum_bucket = match key {
        Some(PopupKey::Cumulative(i)) => Some(i),
        _ => None,
    };
    this.hover_kind = match key {
        Some(PopupKey::Kind(i)) => Some(i),
        _ => None,
    };
}

/// Allow travel across the anchor gap without closing; a generation fence rejects stale timers.
pub(super) fn chart_hover(
    this: &mut AnalyticsView,
    key: PopupKey,
    hovered: bool,
    from_popup: bool,
    cx: &mut Context<AnalyticsView>,
) {
    let pending = if hovered {
        // Crossing dense neighbouring columns on the way to the popup must not make the card
        // chase the pointer. A brief dwell switches buckets; entering the card cancels that dwell.
        let switching = !from_popup
            && this
                .summary_popup_hover
                .target
                .is_some_and(|old| old != key)
            && (this.hover_daily_bucket.is_some()
                || this.hover_cum_bucket.is_some()
                || this.hover_kind.is_some());
        this.summary_popup_hover.enter(key, from_popup);
        if switching {
            Some((this.summary_popup_hover.revision, true))
        } else {
            select_popup(this, Some(key));
            cx.notify();
            None
        }
    } else {
        this.summary_popup_hover
            .leave(key, from_popup)
            .map(|revision| (revision, false))
    };
    if let Some((revision, reveal)) = pending {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor
                .timer(Duration::from_millis(if reveal { 150 } else { 300 }))
                .await;
            let _ = this.update(cx, |this, cx| {
                if reveal && this.summary_popup_hover.is_current(key, revision) {
                    select_popup(this, Some(key));
                    cx.notify();
                } else if !reveal && this.summary_popup_hover.expire(key, revision) {
                    select_popup(this, None);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// Which buckets keep a value label once the bars are denser than the labels are wide.
///
/// The LAST bucket is seeded before anything else, so the period's own end always carries its
/// number — including a zero one, which ordering by magnitude ranks dead last and which the
/// reader most wants to see, because "where did this period finish" is the question the right
/// edge of the chart answers. Every other candidate then has to clear that label rather than the
/// other way round, so the guarantee costs a neighbour rather than an overlap.
///
/// The rest is greedy by descending |value|: the biggest day is labelled next, and every later
/// candidate is kept only if its column sits at least one label-width from every column already
/// kept. That is what replaced the old all-or-nothing `days.len() <= 45` cutoff, which drew a
/// slab of colliding digits just under the limit and NOTHING at all just over it — so on «Все»
/// the chart said nothing about its own extremes.
///
/// Selecting by magnitude rather than by every N-th bucket is deliberate: the days worth naming
/// on a profit chart are the big ones, and an every-N-th rule names whichever days the stride
/// happens to land on.
///
/// Args:
///     vals: Per-bucket profit, ordered, one entry per bar.
///     plot_w: Width the bars are laid out across, in pixels.
///     label_w: Width of one label, in pixels.
///
/// Returns:
///     Indices into `vals` to label, ascending. Always contains the last index.
fn thinned_labels(vals: &[f64], plot_w: f32, label_w: f32) -> Vec<usize> {
    let n = vals.len();
    if n == 0 {
        return Vec::new();
    }
    let last = n - 1;
    // Bars are equal flex cells, so bucket `i` is centred at (i+0.5)/n of the width — the same
    // mapping the hover popup uses. The last one is the exception: its label is right-aligned to
    // the plot's edge instead of centred on its column (`daily_bars`), because a centred label
    // there would hang half its width outside the card. Its centre therefore sits half a label
    // in from that edge, and the separation test has to use THAT, or the neighbour it clears on
    // paper still collides on screen.
    let x = |i: usize| {
        if i == last {
            plot_w - label_w / 2.0
        } else {
            (i as f32 + 0.5) / n as f32 * plot_w
        }
    };
    let mut order: Vec<usize> = (0..n).collect();
    // Index as the tie-break, so two equal days never swap between frames.
    order.sort_by(|&a, &b| vals[b].abs().total_cmp(&vals[a].abs()).then(a.cmp(&b)));
    let mut kept: Vec<usize> = vec![last];
    for i in order {
        if i == last {
            continue;
        }
        if kept.iter().all(|&j| (x(i) - x(j)).abs() >= label_w) {
            kept.push(i);
        }
    }
    kept.sort_unstable();
    kept
}

/// FALLBACK color for a core's series (cycled from the palette) — used when
/// the server has no color in its settings (e.g. the core is already gone from
/// the config). The primary source is `ServerConfig.color` (see core_colors in
/// summary.rs).
pub(super) fn fallback_core_color(p: MoonPalette, i: usize) -> u32 {
    [
        p.blue,
        p.green,
        p.orange,
        p.amber,
        p.red,
        p.yellow,
        p.accent,
        p.text_soft,
    ][i % 8]
}

/// Minimum hue separation, as a fraction of the wheel, before two core colours read as one.
///
/// `picker_palette` steps a full twelfth (0.083) between hues, so this admits every one of its
/// own swatches while rejecting a pair a user picked a few degrees apart.
const MIN_HUE_SEP: f32 = 0.04;
/// Minimum lightness separation that rescues an otherwise too-close hue pair. A dark blue beside
/// a pale blue IS two lines a reader can follow; two mid blues are not.
const MIN_L_SEP: f32 = 0.14;
/// Below this saturation a colour reads as grey and its hue carries no information, so greys are
/// separated by lightness alone.
const GREY_S: f32 = 0.15;
/// Saturation gap that separates two colours on its own. HSL puts a neutral grey and a saturated
/// red at the SAME hue (zero), so a hue test alone calls them identical when they are the easiest
/// pair on the chart to tell apart.
const MIN_S_SEP: f32 = 0.35;

/// Whether two core colours are too close to tell apart on a one-pixel line.
///
/// Deliberately a COARSE rule, not a perceptual distance: it decides only whether to keep a
/// user's own configured colour or to substitute a palette swatch, and a threshold nobody can
/// state is worse than one that is occasionally generous.
///
/// Args:
///     a: One already-taken colour.
///     b: The candidate colour.
///
/// Returns:
///     `true` when a reader would see one line where there are two.
fn too_close(a: Hsla, b: Hsla) -> bool {
    if (b.l - a.l).abs() >= MIN_L_SEP {
        return false;
    }
    if (b.s - a.s).abs() >= MIN_S_SEP {
        return false;
    }
    if a.s < GREY_S && b.s < GREY_S {
        return true;
    }
    let d = (a.h - b.h).abs();
    d.min(1.0 - d) < MIN_HUE_SEP
}

/// Give every drawn core a colour a reader can actually tell from its neighbours.
///
/// The cumulative chart draws up to twelve thin per-core lines inside the total's area, and they
/// were all one blue: a core's colour comes from its own `ServerConfig.color`, and a user who
/// gives a whole exchange one colour gets one colour on the chart. So a configured colour is
/// KEPT — it is the identity the core selector and every popup dot already use — unless it
/// collides with one already taken, and only then is it replaced from `design::picker_palette`,
/// the very palette that colour was chosen from.
///
/// Walked in ascending `uid` order rather than in the caller's order, which is by PROFIT: a core
/// must not change colour because it had a better week.
///
/// Args:
///     configured: One entry per drawn core, in the caller's order — its uid and the RGB its
///         server config carries, or `None` when no config names it.
///     p: Active palette, for the last-resort cycle when the picker palette runs out.
///
/// Returns:
///     One colour per input entry in INPUT order. Colours stay distinguishable until the
///     separated picker swatches run out, then repeat deterministically by uid rank.
pub(super) fn distinct_core_colors(
    configured: &[(u64, Option<[u8; 3]>)],
    p: MoonPalette,
) -> Vec<Hsla> {
    let rgb = |c: [u8; 3]| {
        Hsla::from(Rgba {
            r: f32::from(c[0]) / 255.0,
            g: f32::from(c[1]) / 255.0,
            b: f32::from(c[2]) / 255.0,
            a: 1.0,
        })
    };
    let mut order: Vec<usize> = (0..configured.len()).collect();
    order.sort_by_key(|&i| configured[i].0);
    let mut out = vec![None; configured.len()];
    let mut taken: Vec<Hsla> = Vec::with_capacity(configured.len());
    for &i in &order {
        if let Some(c) = configured[i].1.map(rgb) {
            if !taken.iter().any(|&t| too_close(t, c)) {
                taken.push(c);
                out[i] = Some(c);
            }
        }
    }
    // Mid-shade first (index 2 of `SHADES`, the saturated one the picker leads with), then the
    // LIGHTEST and the DARKEST — never the neighbours. `SHADES` steps lightness by 0.12 between
    // adjacent entries, which is below `MIN_L_SEP`, so shades 1 and 3 would offer swatches
    // `too_close` rejects on sight and the usable pool would be twelve. 2 -> 0 -> 4 keeps every
    // step at 0.22 or more and gives thirty-six.
    let palette = design::picker_palette();
    let spare: Vec<Hsla> = [2usize, 0, 4]
        .into_iter()
        .flat_map(|shade| (0..12).map(move |hue| hue * 5 + shade))
        .filter_map(|ix| palette.get(ix).copied())
        .collect();
    let mut cursor = 0usize;
    for (rank, &i) in order.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let mut pick = None;
        while cursor < spare.len() {
            let candidate = spare[cursor];
            cursor += 1;
            if !taken.iter().any(|&t| too_close(t, candidate)) {
                pick = Some(candidate);
                break;
            }
        }
        let pick = pick.unwrap_or_else(|| {
            // Every separated swatch is spent — more than thirty-six cores need one. Colours
            // repeat from here, but by the core's RANK IN UID ORDER, so a core still keeps the
            // same colour between reloads; keying the repeat on the caller's profit-ordered
            // index would reshuffle the whole chart whenever the ranking moved.
            spare
                .get(rank % spare.len().max(1))
                .copied()
                .unwrap_or_else(|| moon(fallback_core_color(p, rank)))
        });
        taken.push(pick);
        out[i] = Some(pick);
    }
    out.into_iter().flatten().collect()
}

/// Label of one bucket: the HOUR when the grid is finer than a day (a single-day period),
/// the date otherwise. Without this every hourly bucket would be titled with the same date.
///
/// Args:
///     secs: Bucket start as UTC Unix seconds.
///     bucket: Civil bucket width in seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     `HH:MM` for hourly grids or `DD.MM` for wider grids.
pub(super) fn bucket_label(secs: i64, bucket: i64, zone: chrono_tz::Tz) -> String {
    if bucket < 86_400 {
        let s = moon_core::util::display_time::format_minute(secs, zone);
        // "YYYY-MM-DD HH:MM" → "HH:MM"
        if s.len() >= 16 {
            s[11..16].to_string()
        } else {
            s
        }
    } else {
        dm(secs, zone)
    }
}

/// A core's colour looked up by uid — the per-type popup knows uids, not series indices.
pub(super) fn core_color_by_uid(
    cores: &[CoreSeries],
    colors: &[Hsla],
    uid: u64,
    p: MoonPalette,
) -> Hsla {
    match cores.iter().position(|c| c.uid == uid) {
        Some(ci) => core_color(colors, ci, p),
        None => moon(fallback_core_color(p, uid as usize)),
    }
}

/// Format UTC Unix seconds as selected-zone `DD.MM` for axis labels.
///
/// Args:
///     secs: Absolute UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil date label, or the shared formatter's fallback text.
pub(super) fn dm(secs: i64, zone: chrono_tz::Tz) -> String {
    let s = moon_core::util::display_time::format_minute(secs, zone);
    if s.len() >= 10 {
        format!("{}.{}", &s[8..10], &s[5..7])
    } else {
        s
    }
}

/// Daily profit bars: green upward / orange downward from the zero line, with
/// the value labelled above a green bar / below a red one (while the bars are
/// few). Hovering a column pops up that day's per-core breakdown through the shared
/// `bucket_popup` — the same card the cumulative chart opens, in `Day` mode.
///
/// Args:
///     days: Ordered analytical buckets.
///     cores: Per-core series aligned to `days`.
///     colors: Resolved per-core theme colors.
///     hover: Hovered bucket index, if any.
///     bucket: Civil bucket width in seconds.
///     zone: Selected IANA display zone.
///     p: Active MoonUI palette.
///     cx: Analytics view context.
///
/// Returns:
///     Complete daily/hourly bar chart.
pub(super) fn daily_bars(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    hover: Option<usize>,
    bucket: i64,
    zone: chrono_tz::Tz,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if days.is_empty() {
        return div().h(px(CHART_H)).into_any_element();
    }
    let vmax = days
        .iter()
        .map(|d| d.profit)
        .fold(0.0f64, f64::max)
        .max(1e-6);
    let vmin = days
        .iter()
        .map(|d| d.profit)
        .fold(0.0f64, f64::min)
        .min(0.0);
    let span = (vmax - vmin).max(1e-6);
    let up_frac = (vmax / span) as f32; // share of the height above the zero line
    // Which bars keep a label: as many as fit side by side at the caption size, biggest
    // days first. Never a font shrink — the label is at `t_caption` or it is not drawn.
    // Format every candidate ONCE: the widest of them sizes the thinning pass, and the drawn
    // ones are taken straight from here rather than formatted a second time below.
    let texts: Vec<String> = days
        .iter()
        .map(|d| {
            format!(
                "{}{}",
                moon_core::util::fmt::compact(d.profit, 0),
                crate::analytics::pnl_suffix()
            )
        })
        .collect();
    // The label WIDTH is text and is measured at the caption size, so it already follows the
    // Font slider; the plot width is layout and must not (see `PLOT_W_NOMINAL`). Scaling both
    // together would let the room grow with the very setting that makes the labels wider.
    let label_w = widest_label_w(texts.iter().map(String::as_str), cx);
    let profits: Vec<f64> = days.iter().map(|d| d.profit).collect();
    let labelled: std::collections::BTreeSet<usize> =
        thinned_labels(&profits, PLOT_W_NOMINAL, label_w)
            .into_iter()
            .collect();
    // Space reserved for the labels: always on top (above the tallest green
    // bar), on the bottom only when there is a negative value (the label goes
    // BELOW a red bar). Bars scale into the remaining height, so the numbers
    // are never covered by a column. Derived from the caption size rather than a
    // fixed 13px, which was sized for the 8px text this chart used to draw and
    // stopped clearing the line box the moment the Font slider moved.
    // Text-derived height plus a small fixed clearance, and the two take DIFFERENT scales on
    // purpose: `t_caption` follows the Font slider, the clearance goes through `ui_px` like
    // every other piece of chrome (`cumulative.rs` sizes its own band the same way).
    let label_band = f32::from(design::t_caption(cx)) * 1.4 + f32::from(design::ui_px(cx, 2.0));
    // The band is unconditional now: `thinned_labels` always keeps the last bucket and that one
    // is drawn even on a quiet day, so there is no longer a period that reserves room for
    // labels it never draws.
    let pad_top = label_band;
    let pad_bottom = if vmin < 0.0 { label_band } else { 0.0 };
    let area_h = (CHART_H - pad_top - pad_bottom).max(10.0);
    let zero_from_bottom = pad_bottom + area_h * (1.0 - up_frac);
    let n = days.len();

    let mut row = h_flex()
        .w_full()
        .h(px(CHART_H))
        .items_end()
        .gap(px(if n > 120 { 0.0 } else { 1.0 }));
    for (bi, d) in days.iter().enumerate() {
        let frac = (d.profit.abs() / span) as f32;
        let bar_h = (frac * area_h).max(if d.trades > 0 { 1.5 } else { 0.0 });
        // Positives grow up from the zero line, negatives grow down.
        let bottom = if d.profit >= 0.0 {
            zero_from_bottom
        } else {
            (zero_from_bottom - bar_h).max(pad_bottom - label_band)
        };
        let mut col = div()
            .id(SharedString::from(format!("an-db-{bi}")))
            .flex_1()
            .relative()
            .h_full()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                chart_hover(this, PopupKey::Daily(bi), *hovered, false, cx);
            }))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(bottom))
                    .h(px(bar_h))
                    .rounded(px(1.0))
                    .bg(moon(if d.profit >= 0.0 { p.green } else { p.orange })),
            );
        if hover == Some(bi) {
            col = col.bg(moon_alpha(p.text_muted, 0.07));
        }
        // A quiet bucket normally draws no number — an empty day has nothing to report. The LAST
        // bucket is the exception and is drawn whatever it holds, zero included: the right edge
        // of the chart is where a reader looks for where the period ended, and a missing number
        // there reads as missing DATA rather than as a flat day.
        let is_last = bi + 1 == n;
        if labelled.contains(&bi) && (d.trades > 0 || is_last) {
            // Label: above a green bar / below a red one (the space is
            // reserved by pad_top/pad_bottom, so no bar covers the number).
            let label_bottom = if d.profit >= 0.0 {
                bottom + bar_h + 2.0
            } else {
                (bottom - label_band).max(0.0)
            };
            // The label is wider than its column and does not wrap — otherwise
            // "333" got clipped to "33". The overhang is half a label on each
            // side, so it matches the width the thinning pass kept the columns
            // apart by; neighbours can no longer touch.
            let over = px(label_w / 2.0);
            // Except at the right edge, where that same overhang would push half the number past
            // the plot and into the card's padding. The last label spends its whole overhang on
            // the LEFT and ends flush with its own column, which is the plot's edge;
            // `thinned_labels` separates its neighbours against that shifted centre.
            let band = if is_last {
                div().left(px(-label_w)).right_0()
            } else {
                div().left(-over).right(-over)
            };
            let text = div().w_full().flex().child(texts[bi].clone());
            let text = if is_last {
                text.justify_end()
            } else {
                text.justify_center()
            };
            col = col.child(
                band.absolute()
                    .bottom(px(label_bottom))
                    .text_size(design::t_caption(cx))
                    .whitespace_nowrap()
                    .text_color(moon(super::sign_color(p, d.profit)))
                    .child(text),
            );
        }
        row = row.child(col);
    }
    let popup = hover
        .filter(|bi| *bi < n && !cores.is_empty())
        // Bars are equal flex cells: bucket `bi` is the (bi+0.5)/n-th of the width.
        .map(|bi| {
            let frac = (bi as f32 + 0.5) / n as f32;
            bucket_popup(
                days,
                cores,
                colors,
                bi,
                frac,
                bucket,
                zone,
                PopupMode::Day,
                p,
                cx,
            )
        });
    let first = days.first().map(|d| d.start).unwrap_or(0);
    let last = days.last().map(|d| d.start).unwrap_or(0);
    div()
        .relative()
        .w_full()
        .child(v_flex().w_full().gap(px(4.0)).child(row).child(axis_row(
            p,
            bucket_label(first, bucket, zone),
            bucket_label(last, bucket, zone),
        )))
        .children(popup)
        .into_any_element()
}

/// Largest contributors retained in the popup; viewport scrolling separately bounds its height.
const POPUP_ROWS: usize = 16;

/// A core's colour: the server's own, or the cycled fallback when it has no config entry.
pub(super) fn core_color(colors: &[Hsla], ci: usize, p: MoonPalette) -> Hsla {
    colors
        .get(ci)
        .copied()
        .unwrap_or_else(|| moon(fallback_core_color(p, ci)))
}

/// One line of a popup: a coloured dot, a label, and `+500 (123)`.
pub(super) struct PopupRow {
    pub label: String,
    pub dot: Hsla,
    pub value: f64,
    pub trades: i64,
}

/// THE popup of the Summary tab. Every chart builds its own rows and hands them here, so
/// the card, the row cap, the "…N more" tail and the anchoring exist once: the per-bucket
/// core split, the running totals and the per-type core split are all this one card.
///
/// `frac` is where the hovered column sits across the chart's width — the CALLER's
/// business, since the bars are equal flex cells (`bi/n`) while the curve puts its points
/// at `bi/(n-1)`, and a shared guess would anchor the card away from what it describes.
/// `key` binds scroll state and delayed hover events to this exact chart and bucket.
pub(super) fn popup_card(
    title: String,
    total: f64,
    trades: i64,
    mut rows: Vec<PopupRow>,
    frac: f32,
    key: PopupKey,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    // The CAP keeps the biggest by ABSOLUTE value: sorting by signed value first and
    // truncating would drop the worst losers, which are the rows worth reading.
    let found = rows.len();
    if found > POPUP_ROWS {
        rows.sort_by(|a, b| b.value.abs().total_cmp(&a.value.abs()));
        rows.truncate(POPUP_ROWS);
    }
    // Then DISPLAY by value: earners on top, losers at the bottom.
    rows.sort_by(|a, b| b.value.total_cmp(&a.value));
    let hidden = found.saturating_sub(rows.len());
    let more = (hidden > 0).then(|| t!("analytics.popup_more", n = hidden).to_string());
    // Measured BEFORE the rows are consumed, so the card is sized by the very strings it is
    // about to draw rather than by a width guessed once and left behind.
    let content_w = popup_content_w(
        &title,
        &profit_trades(total, trades),
        &rows,
        more.as_deref(),
        cx,
    );
    let mut card = popup_shell(p, cx).child(popup_head(title, total, trades, p, cx));
    for r in rows {
        card = card.child(popup_core_row(r.label, r.dot, r.value, r.trades, p, cx));
    }
    // The cap is never silent: without this the rows visibly fail to add up to the header.
    if let Some(more) = more {
        card = card.child(div().text_color(moon(p.text_muted)).child(more));
    }
    PopupOverlay {
        card,
        width: popup_w(content_w, cx),
        frac,
        key,
        view: cx.entity().downgrade(),
        palette: p,
    }
    .into_any_element()
}

/// Widest line the popup is about to draw, in pixels, chrome between the columns included.
///
/// EVERY row is measured, unlike the bar labels of [`widest_label_w`], which take the longest
/// string as the widest one: that shortcut holds only while a glyph advance is constant, and a
/// core NAME is arbitrary user text that can mix scripts within one popup. Picking the wrong row
/// there would truncate the very name this measurement exists to reveal, and a hovered popup is
/// at most `POPUP_ROWS` server rows, so measuring all of them costs nothing worth saving.
///
/// Args:
///     title: Header text on the left — the bucket's date or the strategy type.
///     total: Already-formatted header total, the `Σ` value.
///     rows: The core lines this card will list, after the row cap.
///     more: The "…N more" tail, when the cap hid something.
///     cx: Analytics view context.
///
/// Returns:
///     Width the card's content needs, excluding its own padding and border.
fn popup_content_w(
    title: &str,
    total: &str,
    rows: &[PopupRow],
    more: Option<&str>,
    cx: &Context<AnalyticsView>,
) -> f32 {
    let w = |s: &str| design::mono_caption_text_width(cx, s, LABEL_WEIGHT);
    // `popup_head`: title, a 10px gap, `Σ`, a 4px gap, the total.
    let head = w(title)
        + f32::from(design::ui_px(cx, 10.0))
        + w("Σ")
        + f32::from(design::ui_px(cx, 4.0))
        + w(total);
    // `popup_core_row`: the 6px dot and two 5px gaps around the name.
    let row_chrome = f32::from(design::ui_px(cx, 6.0)) + f32::from(design::ui_px(cx, 5.0)) * 2.0;
    rows.iter()
        .map(|r| row_chrome + w(&r.label) + w(&profit_trades(r.value, r.trades)))
        .chain(more.map(w))
        .fold(head, f32::max)
}

/// Width of the popup card: measured from its own content, so a long server name reads in full.
///
/// The old fixed 190 turned every name past roughly twenty characters into an ellipsis, and this
/// card is the ONE surface that says WHICH core earned the day — a truncated name there answers
/// nothing. Keep the minimum width for short names, but let full names determine the maximum.
/// Only the real window bounds constrain the viewport; unusually long rows scroll horizontally.
/// [`PopupOverlay`] fits this width against the actual viewport, independently of bucket position.
///
/// Args:
///     content_w: Widest line the card will draw, from [`popup_content_w`].
///     cx: Analytics view context.
///
/// Returns:
///     Outer width for the popup holder.
fn popup_w(content_w: f32, cx: &Context<AnalyticsView>) -> Pixels {
    // `popup_shell`'s own 8px of padding each side, its one-pixel border each side, and a couple
    // of pixels of slack: `mono_caption_text_width` sums glyph advances without kerning, so an
    // exact fit can still clip the last character it was measured to hold.
    let chrome =
        f32::from(design::ui_px(cx, 8.0)) * 2.0 + 2.0 + f32::from(design::ui_px(cx, 2.0)) * 2.0;
    let min = f32::from(design::font_w_px(cx, 190.0));
    let track = f32::from(design::ui_px(cx, moon_ui::MOON_SCROLLBAR_TRACK));
    px(popup_outer_width(content_w, chrome, min, track))
}

/// Reserve scrollbar space outside the measured card, without an arbitrary chart-width cap.
fn popup_outer_width(content: f32, chrome: f32, minimum: f32, track: f32) -> f32 {
    (content + chrome).max(minimum) + track
}

/// Which numbers a bucket popup reports — the ONE thing the two time charts' popups differ in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PopupMode {
    /// Just that bucket: what the daily-bars chart draws.
    Day,
    /// Everything up to and including it: what the cumulative curve draws.
    Running,
}

/// Empty card of a bucket popup — shared chrome, so the cumulative chart's popup and the
/// daily one cannot drift apart in looks.
fn popup_shell(p: MoonPalette, cx: &Context<AnalyticsView>) -> Div {
    v_flex()
        .gap(px(2.0))
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 6.0))
        .rounded(design::ui_px(cx, 6.0))
        .bg(moon(p.panel_high))
        .border_1()
        .border_color(moon(p.border))
        .shadow_md()
        .text_size(design::t_caption(cx))
}

/// `+500 (123)` — profit and the trade count behind it, the one shape every popup number
/// uses so a value can never be mistaken for a count.
fn profit_trades(v: f64, trades: i64) -> String {
    format!("{} ({trades})", super::fmt_signed(v))
}

/// Popup header: the bucket's date on the left, `Σ <total> (<trades>)` on the right. The
/// total lives in the HEADER because with dozens of cores the list's bottom is off screen.
fn popup_head(
    date: String,
    total: f64,
    trades: i64,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    h_flex()
        .justify_between()
        .gap(design::ui_px(cx, 10.0))
        .pb(px(1.0))
        .border_b_1()
        .border_color(moon_alpha(p.border, 0.6))
        .child(div().text_color(moon(p.text)).child(date))
        .child(
            h_flex()
                .gap(design::ui_px(cx, 4.0))
                .child(div().text_color(moon(p.text_muted)).child("Σ"))
                .child(
                    div()
                        .text_color(moon(super::sign_color(p, total)))
                        .child(profit_trades(total, trades)),
                ),
        )
}

/// One core's line in a bucket popup: colour dot, name, `+500 (123)`.
fn popup_core_row(
    name: String,
    dot: Hsla,
    v: f64,
    trades: i64,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    h_flex()
        .gap(design::ui_px(cx, 5.0))
        .items_center()
        .child(
            div()
                .flex_none()
                .w(design::ui_px(cx, 6.0))
                .h(design::ui_px(cx, 6.0))
                .rounded_full()
                .bg(dot),
        )
        .child(
            // Keep identities on one line; the window-bounded viewport scrolls oversized rows.
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .text_color(moon(p.text_soft))
                .child(name),
        )
        .child(
            div()
                .flex_none()
                .text_color(moon(super::sign_color(p, v)))
                .child(profit_trades(v, trades)),
        )
}

/// Fit the scroll viewport inside window chrome, without squeezing it into a bucket's remainder.
fn popup_limits(width: f32, viewport_w: f32, viewport_h: f32, inset: f32) -> (f32, f32) {
    (
        width.min((viewport_w - 2.0 * inset).max(0.0)),
        (viewport_h - 2.0 * inset).max(0.0),
    )
}

/// Window-aware adapter for the shared chart popup; MoonUI supplies anchoring and scrollbars.
#[derive(IntoElement)]
struct PopupOverlay {
    /// Already formatted rows and header, retaining the active profit unit.
    card: Div,
    /// Measured outer width before viewport fitting.
    width: Pixels,
    /// Bucket anchor along the chart's actual laid-out width.
    frac: f32,
    /// Distinguishes scroll state and hover timers across charts and buckets.
    key: PopupKey,
    /// Weak owner avoids an element-to-view reference cycle.
    view: WeakEntity<AnalyticsView>,
    /// Palette for the Moon scrollbar track and thumb.
    palette: MoonPalette,
}

impl RenderOnce for PopupOverlay {
    /// Bound the entire card, preserve intrinsic row heights, and let MoonUI fit its window anchor.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let margin = design::ui_px(cx, 8.0);
        let inset = margin + window.client_inset().unwrap_or(px(0.0));
        let viewport = window.viewport_size();
        let (width, max_height) = popup_limits(
            f32::from(self.width),
            f32::from(viewport.width),
            f32::from(viewport.height),
            f32::from(inset),
        );
        let id = SharedString::from(format!("an-popup-{:?}", self.key));
        let state = window.use_keyed_state(id.clone(), cx, |window, _| {
            // The scrollbar reads bounds from the previous layout, so prime it once on opening.
            window.request_animation_frame();
            ScrollHandle::new()
        });
        let scroll = state.read(cx).clone();
        let track = design::ui_px(cx, moon_ui::MOON_SCROLLBAR_TRACK);
        let horizontal = self.width > px(width);
        let key = self.key;
        let view = self.view;
        let popup = div()
            .id(id.clone())
            .relative()
            .occlude()
            .w(px(width))
            .on_hover(move |hovered, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    chart_hover(this, key, *hovered, true, cx);
                });
            })
            .child(
                div()
                    .id(SharedString::from(format!("{id}:scroll")))
                    .w_full()
                    .max_h(px(max_height))
                    .overflow_y_scroll()
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    // Space for the pinned scrollbar is outside the measured card content.
                    .pr(track)
                    .when(horizontal, |this| this.pb(track))
                    .child(self.card.w(self.width - track).flex_none()),
            )
            .children(moon_scrollbar_overlay_with_palette(
                format!("{id}:bar"),
                &scroll,
                MoonScrollAxis::Both,
                MoonScrollbarVisibility::Always,
                self.palette,
                window,
                cx,
            ));
        let opens_left = self.frac > 0.62;
        deferred(
            div()
                .absolute()
                .left(relative(self.frac))
                .top(px(6.0))
                .child(
                    anchored()
                        .anchor(if opens_left {
                            Anchor::TopRight
                        } else {
                            Anchor::TopLeft
                        })
                        .offset(point(px(if opens_left { -12.0 } else { 12.0 }), px(0.0)))
                        .snap_to_window_with_margin(margin)
                        .child(popup),
                ),
        )
    }
}

/// Popup of bucket `bi`: the date, `Σ` of the whole bucket, and every core that traded by
/// then as `name +500 (123)`, biggest profit first.
///
/// `mode` is the only difference between the two time charts: `Day` reports the bucket
/// alone (what the bars draw), `Running` everything up to and including it (what the
/// cumulative curve draws). Rendering goes through the shared [`popup_card`].
///
/// Args:
///     days: Ordered authoritative bucket totals.
///     cores: Per-core series aligned to `days`.
///     colors: Resolved per-core theme colors.
///     bi: Selected bucket index.
///     frac: Horizontal bucket position from zero through one.
///     bucket: Civil bucket width in seconds.
///     zone: Selected IANA display zone.
///     mode: Per-bucket or running-total interpretation.
///     p: Active MoonUI palette.
///     cx: Analytics view context.
///
/// Returns:
///     Deferred popup anchored beside the selected bucket.
pub(super) fn bucket_popup(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    bi: usize,
    frac: f32,
    bucket: i64,
    zone: chrono_tz::Tz,
    mode: PopupMode,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    // A core is listed when it TRADED by this bucket, not when its value is non-zero:
    // in `Running` mode a core whose total happens to cross zero right here would vanish.
    let items: Vec<(usize, f64, i64)> = cores
        .iter()
        .enumerate()
        .filter_map(|(ci, c)| {
            let (v, t) = match mode {
                PopupMode::Day => (
                    c.per_bucket.get(bi).copied().unwrap_or(0.0),
                    c.per_bucket_trades.get(bi).copied().unwrap_or(0),
                ),
                PopupMode::Running => (
                    c.per_bucket.iter().take(bi + 1).sum(),
                    c.per_bucket_trades.iter().take(bi + 1).sum(),
                ),
            };
            (t > 0).then_some((ci, v, t))
        })
        .collect();
    // Header totals come from `days`, the authoritative series — not from summing the rows,
    // which the row cap would truncate.
    let (total, trades) = match mode {
        PopupMode::Day => days
            .get(bi)
            .map(|d| (d.profit, d.trades))
            .unwrap_or((0.0, 0)),
        PopupMode::Running => days
            .iter()
            .take(bi + 1)
            .fold((0.0, 0), |(s, t), d| (s + d.profit, t + d.trades)),
    };
    let rows: Vec<PopupRow> = items
        .into_iter()
        .map(|(ci, value, trades)| PopupRow {
            label: cores[ci].name.clone(),
            dot: core_color(colors, ci, p),
            value,
            trades,
        })
        .collect();
    let title = days
        .get(bi)
        .map(|d| bucket_label(d.start, bucket, zone))
        .unwrap_or_default();
    let key = match mode {
        PopupMode::Day => PopupKey::Daily(bi),
        PopupMode::Running => PopupKey::Cumulative(bi),
    };
    popup_card(title, total, trades, rows, frac, key, p, cx)
}

/// Profit per STRATEGY TYPE: one bar per type, green up / orange down, the type's name and
/// number under it. Hovering a bar opens the cores behind that type through the shared
/// popup. This replaces the daily bars on a single-day period, where "per day" is one
/// column and says nothing.
pub(super) fn kind_bars(
    kinds: &[KindStat],
    cores: &[CoreSeries],
    colors: &[Hsla],
    hover: Option<usize>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if kinds.is_empty() {
        return div().h(px(CHART_H)).into_any_element();
    }
    // A non-finite profit would turn the whole scale into NaN and feed px(NaN) into layout.
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    let vmax = kinds
        .iter()
        .map(|k| fin(k.profit))
        .fold(0.0f64, f64::max)
        .max(1e-6);
    let vmin = kinds
        .iter()
        .map(|k| fin(k.profit))
        .fold(0.0f64, f64::min)
        .min(0.0);
    let span = (vmax - vmin).max(1e-6);
    let up_frac = (vmax / span) as f32;
    // Value labels stay readable only while the bars are few — the same rule the daily bars
    // use. They are `whitespace_nowrap`, so past this they smear over each other.
    let labels_on = kinds.len() <= 10;
    let pad_top = if labels_on { 13.0f32 } else { 0.0 };
    let pad_bottom = if labels_on && vmin < 0.0 {
        13.0f32
    } else {
        0.0
    };
    let area_h = (CHART_H - pad_top - pad_bottom).max(10.0);
    let zero_from_bottom = pad_bottom + area_h * (1.0 - up_frac);
    let n = kinds.len();

    let mut row = h_flex()
        .w_full()
        .h(px(CHART_H))
        .items_end()
        .gap(design::ui_px(cx, 4.0));
    for (ki, k) in kinds.iter().enumerate() {
        let frac = (k.profit.abs() / span) as f32;
        let bar_h = (frac * area_h).max(if k.trades > 0 { 1.5 } else { 0.0 });
        let bottom = if k.profit >= 0.0 {
            zero_from_bottom
        } else {
            (zero_from_bottom - bar_h).max(0.0)
        };
        let mut col = div()
            .id(SharedString::from(format!("an-kb-{ki}")))
            .flex_1()
            .min_w_0()
            .relative()
            .h_full()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                chart_hover(this, PopupKey::Kind(ki), *hovered, false, cx);
            }))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(bottom))
                    .h(px(bar_h))
                    .rounded(px(1.0))
                    .bg(moon(if k.profit >= 0.0 { p.green } else { p.orange })),
            );
        if labels_on {
            // Value above a green bar / below a red one — pad_top/pad_bottom reserve the room.
            col = col.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(if k.profit >= 0.0 {
                        bottom + bar_h + 2.0
                    } else {
                        (bottom - 12.0).max(0.0)
                    }))
                    .text_size(design::t_caption(cx))
                    .whitespace_nowrap()
                    .text_color(moon(super::sign_color(p, k.profit)))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(profit_trades(k.profit, k.trades)),
                    ),
            );
        }
        if hover == Some(ki) {
            col = col.bg(moon_alpha(p.text_muted, 0.07));
        }
        row = row.child(col);
    }
    // Type names under the bars, on the same flex grid.
    let mut labels = h_flex().w_full().flex_none().gap(design::ui_px(cx, 4.0));
    for k in kinds {
        labels = labels.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_center()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_soft))
                .child(kind_label(&k.kind)),
        );
    }
    let popup = hover
        .and_then(|ki| kinds.get(ki).map(|k| (ki, k)))
        .map(|(ki, k)| {
            let rows: Vec<PopupRow> = k
                .cores
                .iter()
                .map(|c| PopupRow {
                    label: c.name.clone(),
                    dot: core_color_by_uid(cores, colors, c.uid, p),
                    value: c.profit,
                    trades: c.trades,
                })
                .collect();
            // Bars are equal flex cells: bar `ki` is the (ki+0.5)/n-th of the width.
            let frac = (ki as f32 + 0.5) / n as f32;
            popup_card(
                kind_label(&k.kind),
                k.profit,
                k.trades,
                rows,
                frac,
                PopupKey::Kind(ki),
                p,
                cx,
            )
        });
    div()
        .relative()
        .w_full()
        .child(v_flex().w_full().gap(px(4.0)).child(row).child(labels))
        .children(popup)
        .into_any_element()
}

/// A strategy type's display name; an unknown type (no strategies DB) shows a dash rather
/// than an empty column nobody can identify.
fn kind_label(kind: &str) -> String {
    if kind.trim().is_empty() {
        "—".to_string()
    } else {
        kind.to_string()
    }
}

/// Maximum rows in either half of the default per-core overview.
const CORE_OVERVIEW_LIMIT: usize = 10;

/// Display snapshot for one core-ranking row owned by a virtual-list factory.
#[derive(Clone)]
struct CoreRankRow {
    /// Durable identity used to keep element IDs stable across repaints.
    uid: u64,
    /// User-facing server name.
    name: SharedString,
    /// Exact period total in the active Analytics profit unit.
    total: f64,
    /// Magnitude relative to the largest absolute core result, in percent.
    magnitude_pct: f32,
}

/// Compact facts shown under the per-core card title.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CoreRankStats {
    /// Number of cores with trades in the selected period.
    pub total: usize,
    /// Number whose unrounded total is above zero.
    pub profitable: usize,
    /// Number whose unrounded total is below zero.
    pub losing: usize,
    /// Best core divided by a positive net result; losses may make this exceed 100 percent.
    pub leader_share_pct: Option<f64>,
}

/// Split a descending list into non-overlapping leader and outsider ranges.
///
/// Args:
///     len: Number of ranked cores.
///     limit: Maximum rows allocated to either side.
///
/// Returns:
///     The leading range and the trailing range. When fewer than `2 * limit` rows exist, the
///     outsider range starts after the leaders instead of repeating a core in both columns.
fn overview_ranges(len: usize, limit: usize) -> (Range<usize>, Range<usize>) {
    let leaders_end = len.min(limit);
    let outsiders_start = len.saturating_sub(limit).max(leaders_end);
    (0..leaders_end, outsiders_start..len)
}

/// Summarize signs and concentration for the per-core ranking header.
///
/// Args:
///     cores: Profit-descending core series for the selected period.
///
/// Returns:
///     Counts by unrounded sign plus the leading core's share of a positive net result. The share
///     is absent for zero or losing periods because that ratio has no useful interpretation.
pub(super) fn core_rank_stats(cores: &[CoreSeries]) -> CoreRankStats {
    let net: f64 = cores.iter().map(|core| core.total).sum();
    let leader = cores.iter().map(|core| core.total).fold(0.0f64, f64::max);
    CoreRankStats {
        total: cores.len(),
        profitable: cores.iter().filter(|core| core.total > 0.0).count(),
        losing: cores.iter().filter(|core| core.total < 0.0).count(),
        leader_share_pct: (net > f64::EPSILON && leader > 0.0).then_some(leader / net * 100.0),
    }
}

/// Convert database series into owned, normalized rows for a `'static` virtual-list factory.
///
/// Args:
///     cores: Profit-descending core series for the selected period.
///
/// Returns:
///     Owned rows whose largest absolute result has a 100-percent bar.
fn core_rank_rows(cores: &[CoreSeries]) -> Vec<CoreRankRow> {
    let scale = cores
        .iter()
        .map(|core| core.total.abs())
        .fold(0.0f64, f64::max)
        .max(f64::EPSILON);
    cores
        .iter()
        .map(|core| CoreRankRow {
            uid: core.uid,
            name: core.name.clone().into(),
            total: core.total,
            magnitude_pct: (core.total.abs() / scale * 100.0) as f32,
        })
        .collect()
}

/// Render one horizontal core-ranking row with a MoonUI progress bar.
///
/// Args:
///     id_prefix: Stable namespace separating overview and all-mode element IDs.
///     row: Owned display snapshot for the core.
///     p: Active Moon palette.
///     name_w: Maximum width reserved for a readable server name.
///     value_w: Width reserved for the signed total.
///     text_size: Active body text size.
///
/// Returns:
///     One fixed-height row suitable for `MoonVirtualList`.
fn core_rank_row(
    id_prefix: &'static str,
    row: CoreRankRow,
    p: MoonPalette,
    name_w: Pixels,
    value_w: Pixels,
    text_size: Pixels,
) -> AnyElement {
    h_flex()
        .size_full()
        .min_w_0()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .child(
            div()
                .flex_1()
                .max_w(name_w)
                .min_w_0()
                .truncate()
                .text_size(text_size)
                .text_color(moon(p.text_soft))
                .child(row.name),
        )
        .child(
            div().flex_1().min_w_0().child(
                MoonProgress::new(format!("{id_prefix}-bar-{}", row.uid))
                    .value(row.magnitude_pct)
                    .color(super::sign_color(p, row.total))
                    .height(7.0)
                    .render(),
            ),
        )
        .child(
            div()
                .flex_none()
                .w(value_w)
                .text_right()
                .whitespace_nowrap()
                .text_size(text_size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(super::sign_color(p, row.total)))
                .child(super::fmt_signed(row.total)),
        )
        .into_any_element()
}

/// Render the default two-column leaders/outsiders overview in a virtual list.
///
/// Args:
///     rows: Owned profit-descending core rows.
///     p: Active Moon palette.
///     cx: Analytics context used to resolve scaled row geometry.
///
/// Returns:
///     A two-column ranking whose shared vertical scrollbar remains usable at the card's minimum
///     height and whose two sides never repeat the same core.
fn core_rank_overview(
    rows: Vec<CoreRankRow>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let (leaders, outsiders) = overview_ranges(rows.len(), CORE_OVERVIEW_LIMIT);
    let leaders = rows[leaders].to_vec();
    let outsiders: Vec<_> = rows[outsiders].iter().rev().cloned().collect();
    let count = leaders.len().max(outsiders.len());
    let row_h = f32::from(design::fit_h_px(cx, 28.0, 14.0, 7.0));
    let name_w = design::font_w_px(cx, 180.0);
    let value_w = design::font_w_px(cx, 82.0);
    let text_size = design::t_body(cx);
    let gap = design::ui_px(cx, 10.0);
    let scrollbar_gutter = design::ui_px(cx, 8.0);
    let list = MoonVirtualList::new("an-core-rank-overview", count, row_h, move |ix, _, _| {
        let left = leaders
            .get(ix)
            .cloned()
            .map(|row| core_rank_row("an-core-leader", row, p, name_w, value_w, text_size))
            .unwrap_or_else(|| div().into_any_element());
        let right = outsiders
            .get(ix)
            .cloned()
            .map(|row| core_rank_row("an-core-outsider", row, p, name_w, value_w, text_size))
            .unwrap_or_else(|| div().into_any_element());
        h_flex()
            .size_full()
            .min_w_0()
            .gap(gap)
            .pr(scrollbar_gutter)
            .child(div().flex_1().min_w_0().child(left))
            .child(div().flex_1().min_w_0().child(right))
    })
    .surface(false)
    .border(false)
    .radius(0.0)
    .scrollbar_visibility(MoonScrollbarVisibility::Always);
    v_flex()
        .size_full()
        .min_h_0()
        .gap(design::ui_px(cx, 2.0))
        .child(
            h_flex()
                .w_full()
                .gap(gap)
                .text_size(design::t_caption(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.text_muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(design::ui_px(cx, 6.0))
                        .child(t!("analytics.cores_leaders").to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(design::ui_px(cx, 6.0))
                        .child(t!("analytics.cores_outsiders").to_string()),
                ),
        )
        .child(div().flex_1().min_h_0().child(list))
        .into_any_element()
}

/// Render every core in one profit-descending virtual list.
///
/// Args:
///     rows: Owned profit-descending core rows.
///     p: Active Moon palette.
///     cx: Analytics context used to resolve scaled row geometry.
///
/// Returns:
///     The complete ranking with an always-visible MoonUI scrollbar.
fn core_rank_all(
    rows: Vec<CoreRankRow>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let count = rows.len();
    let row_h = f32::from(design::fit_h_px(cx, 28.0, 14.0, 7.0));
    let name_w = design::font_w_px(cx, 280.0);
    let value_w = design::font_w_px(cx, 92.0);
    let text_size = design::t_body(cx);
    let scrollbar_gutter = design::ui_px(cx, 8.0);
    MoonVirtualList::new("an-core-rank-all", count, row_h, move |ix, _, _| {
        div().size_full().pr(scrollbar_gutter).child(
            rows.get(ix)
                .cloned()
                .map(|row| core_rank_row("an-core-all", row, p, name_w, value_w, text_size))
                .unwrap_or_else(|| div().into_any_element()),
        )
    })
    .surface(false)
    .border(false)
    .radius(0.0)
    .scrollbar_visibility(MoonScrollbarVisibility::Always)
    .into_any_element()
}

/// Render period totals per core as a readable horizontal ranking.
///
/// Args:
///     cores: Profit-descending core series for the selected period.
///     show_all: Whether to show the complete one-column ranking instead of the two-column
///         leaders/outsiders overview.
///     p: Active Moon palette.
///     cx: Analytics context used to resolve scaled geometry.
///
/// Returns:
///     A virtualized ranking that fills the elastic bottom card.
pub(super) fn core_totals_rank(
    cores: &[CoreSeries],
    show_all: bool,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if cores.is_empty() {
        return div().flex_1().into_any_element();
    }
    let rows = core_rank_rows(cores);
    if show_all {
        core_rank_all(rows, p, cx)
    } else {
        core_rank_overview(rows, p, cx)
    }
}

#[cfg(test)]
mod tests;

/// X-axis labels of the daily-bars chart: the first and last date.
fn axis_row(p: MoonPalette, left: String, right: String) -> AnyElement {
    h_flex()
        .w_full()
        .justify_between()
        .child(muted_caption(p, left))
        .child(muted_caption(p, right))
        .into_any_element()
}

pub(super) fn muted_caption(p: MoonPalette, text: String) -> Div {
    div().text_color(moon(p.text_muted)).child(text)
}
