//! Detection cards based on the design specification.
//!
//! Each card has a neutral palette background and border rather than a core-color fill, and can
//! add a server-color rail and gradient fade on the left. Fields occupy configured
//! [`DetectSizeCfg::slots`]: mini uses four slots in two rows, medium uses six across side columns
//! or chart-overlay corners, and large uses nine in bands above and below the chart or overlays.
//! `detects_view.toml` configures dimensions, chart type, and rail per size. This module builds
//! visuals only; [`super`] owns card ordering and attaches left- and right-click actions.

use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonText, h_flex, rgba_from, v_flex,
};

use moon_core::config::{
    BadgesConfig, DETECT_SIZE_LARGE, DETECT_SIZE_MEDIUM, DETECT_SIZE_MINI, DetectChart,
    DetectField, DetectSizeCfg, DetectSlot, DetectViewCfg, detect_slot_count,
};

use super::DetectItem;
use crate::design;

/// Fraction of a medium card's width allocated to the chart between the text columns.
const MEDIUM_CHART_FRAC: f32 = 0.40;
/// Height reserved for each field band above and below a large card's chart, in logical pixels.
const BAND_H: f32 = 16.0;

/// Return the rounded chart-zone dimensions in scaled logical pixels.
///
/// The rendered chart box keeps this height while its vector content stretches across the zone.
fn zone_dims(size: u8, s: &DetectSizeCfg, cx: &App) -> (f32, f32) {
    let (w, h) = (f32::from(s.w), f32::from(s.h));
    match size {
        // Large: use the full inner width and subtract field bands and padding from the height.
        DETECT_SIZE_LARGE => (
            design::ui_value(cx, w - 16.0).round(),
            design::ui_value(cx, (h - 12.0 - 2.0 * BAND_H - 6.0).max(12.0)).round(),
        ),
        // Medium: use a fixed width fraction and nearly the full card height.
        _ => (
            medium_zone_w(s, cx),
            design::ui_value(cx, (h - 10.0).max(12.0)).round(),
        ),
    }
}

/// Build a card at the configured active size without attaching click handlers.
#[allow(clippy::too_many_arguments)]
pub(super) fn card(
    it: &DetectItem,
    secs: u32,
    cfg: &DetectViewCfg,
    theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    card_sized(
        it,
        secs,
        cfg,
        cfg.size_clamped(),
        theme,
        badges,
        p,
        is_light,
        cx,
    )
}

/// Build a card for an explicit size; [`card`] passes the clamped active configuration size.
#[allow(clippy::too_many_arguments)]
pub(super) fn card_sized(
    it: &DetectItem,
    secs: u32,
    cfg: &DetectViewCfg,
    size: u8,
    theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let scfg = cfg.size_cfg(size);
    let dec = cfg.delta_decimals_clamped();
    let color = design::rgb_to_u32(it.color);
    let inner = match size {
        DETECT_SIZE_MINI => mini_layout(it, secs, scfg, dec, theme, badges, p, is_light, cx),
        DETECT_SIZE_LARGE => large_layout(it, secs, scfg, dec, theme, badges, p, is_light, cx),
        _ => medium_layout(it, secs, scfg, dec, theme, badges, p, is_light, cx),
    };
    base(scfg, color, p, cx).child(inner)
}

/// Build the neutral card surface and its server-color rail with gradient fade.
fn base(scfg: &DetectSizeCfg, color: u32, p: MoonPalette, cx: &App) -> Div {
    let w = design::ui_px(cx, f32::from(scfg.w));
    let h = design::ui_px(cx, f32::from(scfg.h));
    let mut card = div()
        .relative()
        .flex()
        .w(w)
        .max_w(w)
        .h(h)
        .flex_none()
        // The design's 6px radius sits between Moon UI's 4px button and 8px container radii.
        .rounded(design::ui_px(cx, 6.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.shell_high))
        .overflow_hidden()
        .hover(|s| s.border_color(rgba_from(color, 0.6)).bg(rgb(p.panel_high)));
    for layer in rail_layers(
        color,
        f32::from(scfg.rail_w_clamped()),
        f32::from(scfg.rail_grad_clamped()),
        f32::from(scfg.w),
        cx,
    ) {
        card = card.child(layer);
    }
    card
}

/// Build full-card background layers for the rail stripe and its gradient fade.
///
/// The fade has a hard stop at the stripe width. GPUI rounds each layer with its div, so the
/// stripe follows the card corner for the full height. A separate narrow bar would protrude at
/// the corner because `overflow_hidden` clips rectangularly and a narrow div's radius is clamped
/// to half its width. The layers use a 1px inset to remain beneath the card border.
pub(super) fn rail_layers(color: u32, rail_w: f32, grad_w: f32, card_w: f32, cx: &App) -> Vec<Div> {
    let mut out = Vec::new();
    let inner_w = (design::ui_value(cx, card_w) - 2.0).max(1.0);
    let r = design::ui_px(cx, 5.0);
    let stripe = design::ui_value(cx, rail_w) / inner_w;
    let layer = || {
        div()
            .absolute()
            .left(px(1.0))
            .right(px(1.0))
            .top(px(1.0))
            .bottom(px(1.0))
            .rounded(r)
    };
    if grad_w > 0.0 {
        let grad_end = ((design::ui_value(cx, rail_w + grad_w)) / inner_w).clamp(0.0, 1.0);
        out.push(layer().bg(linear_gradient(
            90.0,
            linear_color_stop(rgba_from(color, 0.125), stripe.min(1.0)),
            linear_color_stop(rgba_from(color, 0.0), grad_end.max(stripe + 0.001)),
        )));
    }
    if rail_w > 0.0 {
        // Use a hard stop with roughly half a pixel of transition for slight edge antialiasing.
        out.push(layer().bg(linear_gradient(
            90.0,
            linear_color_stop(rgba_from(color, 1.0), stripe.min(1.0)),
            linear_color_stop(rgba_from(color, 0.0), (stripe + 0.002).min(1.0)),
        )));
    }
    out
}

/// Return the nominal width of a medium card's chart zone.
fn medium_zone_w(scfg: &DetectSizeCfg, cx: &App) -> f32 {
    design::ui_value(cx, (f32::from(scfg.w) * MEDIUM_CHART_FRAC).max(30.0)).round()
}

/// Return the left content padding required to clear the rail plus a small gap.
fn pad_l(scfg: &DetectSizeCfg, base: f32, cx: &App) -> Pixels {
    design::ui_px(cx, base + f32::from(scfg.rail_w_clamped()))
}

// --- Field chips use the shared MoonText, MoonBadge, and delta styles. ---

/// Build the detection-type badge from its long/short code, theme color, and optional outline.
///
/// Return `None` when this detection type's badge is disabled.
fn type_badge(it: &DetectItem, badges: &BadgesConfig, is_light: bool) -> Option<MoonBadge> {
    badges.active(it.kind).then(|| {
        let code = badges.code(it.kind, it.is_short).to_string();
        let bcol = design::rgb_to_u32(badges.color(it.kind, is_light));
        let mut badge = MoonBadge::new(code)
            .variant(MoonBadgeVariant::Soft)
            .size(MoonBadgeSize::Tiny)
            .bg_color(bcol)
            .text_color(bcol)
            .mono(true);
        if let Some(oc) = badges.outline_color(it.kind, it.is_short, is_light) {
            let ocol = design::rgb_to_u32(oc);
            badge = badge.border_color(ocol).border_alpha(0.9);
        }
        badge
    })
}

/// Build a tiny core-name badge using the server color.
fn core_badge(it: &DetectItem, color: u32) -> MoonBadge {
    MoonBadge::new(it.core_name.clone())
        .variant(MoonBadgeVariant::Soft)
        .size(MoonBadgeSize::Tiny)
        .bg_color(color)
        .text_color(color)
        .border_color(color)
        .border_alpha(0.4)
        .mono(true)
}

/// Build the coin token as a prominent monospace label.
fn coin_text(it: &DetectItem, p: MoonPalette, size: f32) -> MoonText {
    MoonText::new(it.base.clone())
        .color(p.text)
        .font_size(size)
        .line_height(size + 3.0)
        .weight(600.0)
        .mono(true)
        .uppercase(false)
}

/// Build a small muted label, used for time, at MoonText's default 9px size.
fn muted(text: String, p: MoonPalette) -> MoonText {
    MoonText::new(text)
        .color(p.text_muted)
        .mono(true)
        .uppercase(false)
}

/// Build an exchange or exchange-kind label in the soft text tone.
fn soft(text: String, p: MoonPalette) -> MoonText {
    MoonText::new(text)
        .color(p.text_soft)
        .mono(true)
        .uppercase(false)
}

/// Reuse the terminal header's positive and negative delta colors.
use design::{danger_color as neg_col, positive_color as pos_col};

/// Build a bold monospace percentage delta such as `+1.23%`.
///
/// `over` adds a backing for legibility over a chart. `decimals` is the popup's precision setting
/// shared by all card sizes.
fn delta_chip(val: f32, over: bool, decimals: usize, p: MoonPalette, cx: &App) -> Div {
    // Same percentage contract as the header deltas: classify by the ROUNDED value, so a small
    // negative cannot print a minus while being coloured positive.
    let (label, col) = match moon_core::util::fmt::signed_pct(f64::from(val), decimals) {
        Some((text, sign)) => (text, sign.pick(pos_col(p), neg_col(p), p.text_soft)),
        None => ("—".to_string(), p.text_muted),
    };
    let text = MoonText::new(label)
        .color(col)
        .weight(700.0)
        .mono(true)
        .uppercase(false);
    let mut chip = div().child(text);
    if over {
        chip = chip
            .px(px(2.0))
            .rounded(design::ui_px(cx, 2.0))
            .bg(rgba_from(p.surface, 0.72));
    }
    chip
}

/// Build one configured slot field.
///
/// Args:
///     field: Configured field kind for this card slot.
///     over: Whether the field is drawn over a chart.
///     it: Detection snapshot supplying field values.
///     secs: Rounded detection age in seconds.
///     coin_px: Available coin-label width.
///     decimals: Percentage precision selected for the card.
///     badges: Detection-type badge configuration.
///     p: Active Moon palette.
///     is_light: Whether the active palette is light.
///     cx: Application context used for scaled dimensions.
///
/// Returns:
///     Rendered field, or `None` for an empty field, blank exchange, or disabled type badge.
#[allow(clippy::too_many_arguments)]
fn chip(
    field: DetectField,
    over: bool,
    it: &DetectItem,
    secs: u32,
    coin_px: f32,
    decimals: usize,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Option<AnyElement> {
    let el: AnyElement = match field {
        DetectField::None => return None,
        DetectField::Coin => coin_text(it, p, coin_px).render().into_any_element(),
        DetectField::Time => muted(format!("{secs}s"), p).render().into_any_element(),
        DetectField::Badge => type_badge(it, badges, is_light)?
            .render()
            .into_any_element(),
        DetectField::Core => core_badge(it, design::rgb_to_u32(it.color))
            .render()
            .into_any_element(),
        DetectField::Delta24h => delta_chip(it.delta_24h, over, decimals, p, cx).into_any_element(),
        DetectField::Delta1h => delta_chip(it.delta_1h, over, decimals, p, cx).into_any_element(),
        DetectField::Exchange => {
            let exchange = crate::controls::exchange_display_name(&it.exchange_name);
            if exchange.is_empty() {
                return None;
            }
            soft(exchange, p).render().into_any_element()
        }
        DetectField::ExchangeKind => {
            if it.exchange_kind.is_empty() {
                return None;
            }
            soft(it.exchange_kind.clone(), p)
                .render()
                .into_any_element()
        }
    };
    // Give non-delta chart overlays the same design backing used for readable overlay chips.
    if over && !matches!(field, DetectField::Delta24h | DetectField::Delta1h) {
        return Some(
            div()
                .px(px(2.0))
                .rounded(design::ui_px(cx, 2.0))
                .bg(rgba_from(p.surface, 0.72))
                .child(el)
                .into_any_element(),
        );
    }
    Some(el)
}

/// Build a chip cluster from the surviving slots while preserving iterator order.
///
/// Return `None` when every slot is empty or filtered out.
#[allow(clippy::too_many_arguments)]
fn cluster<'a>(
    slots: impl Iterator<Item = &'a DetectSlot>,
    over: bool,
    it: &DetectItem,
    secs: u32,
    coin_px: f32,
    decimals: usize,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Option<Div> {
    let chips: Vec<AnyElement> = slots
        .filter_map(|s| {
            chip(
                s.field, over, it, secs, coin_px, decimals, badges, p, is_light, cx,
            )
        })
        .collect();
    if chips.is_empty() {
        return None;
    }
    Some(
        h_flex()
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .whitespace_nowrap()
            .children(chips),
    )
}

/// Build the configured chart element from either vector renderer.
///
/// Both renderers paint against the zone's actual bounds; bitmap thumbnails distorted when
/// stretched and required baking and invalidation. Return `None` when the chart is disabled or
/// its selected renderer has no data.
fn chart_el(
    it: &DetectItem,
    scfg: &DetectSizeCfg,
    theme: &moon_core::config::ChartTheme,
) -> Option<AnyElement> {
    match scfg.chart {
        DetectChart::None => None,
        DetectChart::Candles => candle_canvas(&it.bars, theme),
        DetectChart::Line => line_canvas(&it.line, theme),
    }
}

/// Draw hollow vector candles as quads in the element's actual bounds.
///
/// The high-low wick has segments above and below the body. Rising and doji bodies are outlined;
/// falling bodies are filled. Scaling uses the high-low range with a 1px inset.
fn candle_canvas(
    bars: &[(f32, f32, f32, f32)],
    theme: &moon_core::config::ChartTheme,
) -> Option<AnyElement> {
    if bars.is_empty() {
        return None;
    }
    let up = rgba_from(design::rgb_to_u32(theme.candle_up), 1.0);
    let down = rgba_from(design::rgb_to_u32(theme.candle_down), 1.0);
    let neutral = rgba_from(design::rgb_to_u32(theme.candle_neutral), 1.0);
    let bars: Vec<(f32, f32, f32, f32)> = bars.to_vec();
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                if w < 2.0 || h < 2.0 {
                    return;
                }
                let (mut hi, mut lo) = (f32::NEG_INFINITY, f32::INFINITY);
                for &(_, bh, bl, _) in &bars {
                    hi = hi.max(bh);
                    lo = lo.min(bl);
                }
                if !hi.is_finite() || !lo.is_finite() {
                    return;
                }
                let span = (hi - lo).max(1e-9);
                let pad = if h > 6.0 { 1.0 } else { 0.0 };
                let usable = (h - 2.0 * pad).max(1.0);
                let yof = |price: f32| pad + (hi - price) / span * usable;
                let n = bars.len() as f32;
                let col_w = (w / n).max(1.0);
                let wick_w = (col_w * 0.22).clamp(1.0, 3.0);
                let body_w = (col_w * 0.68).max(wick_w);
                let (ox, oy) = (bounds.origin.x, bounds.origin.y);
                let mut quad = |x0: f32, x1: f32, y0: f32, y1: f32, c: Hsla| {
                    let (y0, y1) = (y0.min(y1), (y0.max(y1)).max(y0.min(y1) + 1.0));
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            gpui::point(ox + px(x0), oy + px(y0)),
                            gpui::point(ox + px(x1.max(x0 + 1.0)), oy + px(y1)),
                        ),
                        c,
                    ));
                };
                for (i, &(o, bh, bl, c)) in bars.iter().enumerate() {
                    let xc = (i as f32 + 0.5) * col_w;
                    let color = if c > o {
                        up
                    } else if c < o {
                        down
                    } else {
                        neutral
                    };
                    let (x0, x1) = (xc - body_w * 0.5, xc + body_w * 0.5);
                    let (yt, yb) = (yof(o).min(yof(c)), yof(o).max(yof(c)));
                    let (wx0, wx1) = (xc - wick_w * 0.5, xc + wick_w * 0.5);
                    if yt - yof(bh) > 0.5 {
                        quad(wx0, wx1, yof(bh), yt, color);
                    }
                    if yof(bl) - yb > 0.5 {
                        quad(wx0, wx1, yb, yof(bl), color);
                    }
                    if c < o {
                        quad(x0, x1, yt, yb, color);
                    } else {
                        // Draw a hollow body with a 1px outline.
                        quad(x0, x1, yt, yt + 1.0, color);
                        quad(x0, x1, yb - 1.0, yb, color);
                        quad(x0, x0 + 1.0, yt, yb, color);
                        quad(x1 - 1.0, x1, yt, yb, color);
                    }
                }
            },
        )
        .size_full()
        .into_any_element(),
    )
}

/// Draw a vector sparkline against the element's actual bounds with a fixed-width stroke.
///
/// Binning keeps roughly one point per 3px; the high/low range uses the smoothed series with 18%
/// vertical padding.
fn line_canvas(line: &[f32], theme: &moon_core::config::ChartTheme) -> Option<AnyElement> {
    if line.len() < 2 {
        return None;
    }
    let rgb3 = if line[line.len() - 1] >= line[0] {
        theme.candle_up
    } else {
        theme.candle_down
    };
    let color = rgba_from(design::rgb_to_u32(rgb3), 1.0);
    let prices: Vec<f32> = line.to_vec();
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                if w < 2.0 || h < 2.0 {
                    return;
                }
                let target = ((w / 3.0) as usize).clamp(2, prices.len());
                let pts: Vec<f32> = (0..target)
                    .map(|k| {
                        let a = k * prices.len() / target;
                        let b = (((k + 1) * prices.len() / target).max(a + 1)).min(prices.len());
                        let sl = &prices[a..b];
                        sl.iter().copied().filter(|v| v.is_finite()).sum::<f32>()
                            / (sl.iter().filter(|v| v.is_finite()).count().max(1) as f32)
                    })
                    .collect();
                let (mut hi, mut lo) = (f32::NEG_INFINITY, f32::INFINITY);
                for &v in &pts {
                    if v.is_finite() {
                        hi = hi.max(v);
                        lo = lo.min(v);
                    }
                }
                if !hi.is_finite() || !lo.is_finite() {
                    return;
                }
                let span = (hi - lo).max(1e-9);
                let pad = (h * 0.18).max(2.0);
                let usable = (h - 2.0 * pad).max(1.0);
                let n = pts.len();
                let mut pb = PathBuilder::stroke(px(2.0));
                for (k, &v) in pts.iter().enumerate() {
                    let x = bounds.origin.x + px(k as f32 / ((n - 1).max(1) as f32) * w);
                    let y = bounds.origin.y + px(pad + (hi - v) / span * usable);
                    if k == 0 {
                        pb.move_to(gpui::point(x, y));
                    } else {
                        pb.line_to(gpui::point(x, y));
                    }
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .size_full()
        .into_any_element(),
    )
}

/// Return whether a slot is effectively overlaid, which requires an enabled chart.
fn eff_over(scfg: &DetectSizeCfg, slot: &DetectSlot) -> bool {
    scfg.chart != DetectChart::None && slot.over
}

// --- Mini: two rows split between left and right, without a chart. ---

fn mini_layout(
    it: &DetectItem,
    secs: u32,
    scfg: &DetectSizeCfg,
    decimals: usize,
    _theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let n = detect_slot_count(DETECT_SIZE_MINI);
    let slots = &scfg.slots[..n];
    let row = |range: std::ops::Range<usize>| {
        let l = cluster(
            slots[range.clone()].iter().filter(|s| !s.right),
            false,
            it,
            secs,
            12.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        let r = cluster(
            slots[range].iter().filter(|s| s.right),
            false,
            it,
            secs,
            12.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .children(l)
            .children(r)
    };
    v_flex()
        .size_full()
        .pl(pad_l(scfg, 7.0, cx))
        .pr(design::ui_px(cx, 6.0))
        .py(design::ui_px(cx, 4.0))
        .justify_between()
        .child(row(0..2))
        .child(row(2..4))
}

// --- Medium: side text columns, a central chart, and overlay chips at chart edges. ---

fn medium_layout(
    it: &DetectItem,
    secs: u32,
    scfg: &DetectSizeCfg,
    decimals: usize,
    theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let n = detect_slot_count(DETECT_SIZE_MEDIUM);
    let slots = &scfg.slots[..n];
    let half = n / 2;
    // Each side column takes its top row from the first three slots and bottom row from the rest.
    let column = |right: bool| -> Div {
        let top = cluster(
            slots[..half]
                .iter()
                .filter(|s| !eff_over(scfg, s) && s.right == right),
            false,
            it,
            secs,
            13.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        let bot = cluster(
            slots[half..]
                .iter()
                .filter(|s| !eff_over(scfg, s) && s.right == right),
            false,
            it,
            secs,
            13.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        // Size the column to its content so the chart takes the remaining width; an empty column
        // occupies no space and leaves no gap along the side.
        let mut col = v_flex()
            .h_full()
            .flex_none()
            .min_w(px(0.0))
            .overflow_hidden()
            .justify_between()
            .py(design::ui_px(cx, 5.0));
        if right {
            col = col.items_end();
        }
        col.children(top).children(bot)
    };
    // Place chart-overlay chips at zone corners with a single horizontal anchor. GPUI does not
    // resolve an absolute element's size from paired insets, which pushed content past the frame.
    let corner = |top_row: bool, right: bool| -> Option<Div> {
        let range = if top_row { 0..half } else { half..n };
        let c = cluster(
            slots[range]
                .iter()
                .filter(|s| eff_over(scfg, s) && s.right == right),
            true,
            it,
            secs,
            13.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        )?;
        let mut host = div().absolute();
        host = if right {
            host.right(px(3.0))
        } else {
            host.left(px(3.0))
        };
        host = if top_row {
            host.top(px(4.0))
        } else {
            host.bottom(px(4.0))
        };
        Some(host.child(c))
    };

    let chart_on = scfg.chart != DetectChart::None;
    let mid = if chart_on {
        // The zone takes the space between columns. Stretch the vector across its full width;
        // otherwise the fixed box hugs the left content and is clipped on the right. Keep the
        // computed height exact because vertical stretching clipped the line.
        let (_zw, zh) = zone_dims(DETECT_SIZE_MEDIUM, scfg, cx);
        let mut zone = div()
            .relative()
            .flex_1()
            .min_w(design::ui_px(cx, 20.0))
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden();
        if let Some(el) = chart_el(it, scfg, theme) {
            zone = zone.child(div().w_full().h(px(zh)).flex_none().child(el));
        }
        zone = zone
            .children(corner(true, false))
            .children(corner(true, true))
            .children(corner(false, false))
            .children(corner(false, true));
        zone
    } else {
        div().flex_1()
    };

    h_flex()
        .size_full()
        .pl(pad_l(scfg, 8.0, cx))
        .pr(design::ui_px(cx, 6.0))
        .items_stretch()
        .gap(design::ui_px(cx, 6.0))
        .child(column(false))
        .child(mid)
        .child(column(true))
}

// --- Large: bands above and below the chart plus overlay corners. ---

fn large_layout(
    it: &DetectItem,
    secs: u32,
    scfg: &DetectSizeCfg,
    decimals: usize,
    theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let n = detect_slot_count(DETECT_SIZE_LARGE);
    let slots = &scfg.slots[..n];
    // Build each non-overlay band above or below the chart with clusters at both edges.
    let band = |below: bool| -> Div {
        let l = cluster(
            slots
                .iter()
                .filter(|s| !eff_over(scfg, s) && s.below == below && !s.right),
            false,
            it,
            secs,
            14.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        let r = cluster(
            slots
                .iter()
                .filter(|s| !eff_over(scfg, s) && s.below == below && s.right),
            false,
            it,
            secs,
            14.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        // An empty band retains its height so field settings do not move the chart.
        h_flex()
            .w_full()
            .h(design::ui_px(cx, BAND_H))
            .justify_between()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .children(l)
            .children(r)
    };
    // Build each chart-overlay corner from its above/below and left/right flags.
    let corner = |below: bool, right: bool| -> Option<Div> {
        let c = cluster(
            slots
                .iter()
                .filter(|s| eff_over(scfg, s) && s.below == below && s.right == right),
            true,
            it,
            secs,
            14.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        )?;
        let mut host = div().absolute();
        host = if right {
            host.right(px(4.0))
        } else {
            host.left(px(4.0))
        };
        host = if below {
            host.bottom(px(2.0))
        } else {
            host.top(px(2.0))
        };
        Some(host.child(c))
    };

    let chart_on = scfg.chart != DetectChart::None;
    let mid = if chart_on {
        // The zone sits between the bands. Fill its width and preserve the exact computed height
        // because vertical stretching clipped the line. Overlay corners use one-sided anchors.
        let (_zw, zh) = zone_dims(DETECT_SIZE_LARGE, scfg, cx);
        let mut zone = div()
            .relative()
            .flex_1()
            .min_h(design::ui_px(cx, 12.0))
            .my(px(3.0))
            .flex()
            .items_center()
            .overflow_hidden();
        if let Some(el) = chart_el(it, scfg, theme) {
            zone = zone.child(div().w_full().h(px(zh)).flex_none().child(el));
        }
        zone = zone
            .children(corner(false, false))
            .children(corner(false, true))
            .children(corner(true, false))
            .children(corner(true, true));
        zone
    } else {
        div().flex_1()
    };

    v_flex()
        .size_full()
        .pl(pad_l(scfg, 8.0, cx))
        .pr(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 6.0))
        .child(band(false))
        .child(mid)
        .child(band(true))
}
