//! Summary chart popup helpers.

use super::*;

/// Largest contributors retained in the popup; viewport scrolling separately bounds its height.
const POPUP_ROWS: usize = 16;

/// A core's colour: the server's own, or the cycled fallback when it has no config entry.
pub(in crate::analytics::summary) fn core_color(
    colors: &[Hsla],
    ci: usize,
    p: MoonPalette,
) -> Hsla {
    colors
        .get(ci)
        .copied()
        .unwrap_or_else(|| moon(fallback_core_color(p, ci)))
}

/// One line of a popup: a coloured dot, a label, and `+500 (123)`.
pub(in crate::analytics::summary) struct PopupRow {
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
pub(in crate::analytics::summary) fn popup_card(
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
pub(super) fn popup_content_w(
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
pub(super) fn popup_w(content_w: f32, cx: &Context<AnalyticsView>) -> Pixels {
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
pub(super) fn popup_outer_width(content: f32, chrome: f32, minimum: f32, track: f32) -> f32 {
    (content + chrome).max(minimum) + track
}

/// Which numbers a bucket popup reports — the ONE thing the two time charts' popups differ in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::analytics::summary) enum PopupMode {
    /// Just that bucket: what the daily-bars chart draws.
    Day,
    /// Everything up to and including it: what the cumulative curve draws.
    Running,
}

/// Empty card of a bucket popup — shared chrome, so the cumulative chart's popup and the
/// daily one cannot drift apart in looks.
pub(super) fn popup_shell(p: MoonPalette, cx: &Context<AnalyticsView>) -> Div {
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
pub(super) fn profit_trades(v: f64, trades: i64) -> String {
    format!("{} ({trades})", super::fmt_signed(v))
}

/// Popup header: the bucket's date on the left, `Σ <total> (<trades>)` on the right. The
/// total lives in the HEADER because with dozens of cores the list's bottom is off screen.
pub(super) fn popup_head(
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
pub(super) fn popup_core_row(
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
pub(super) fn popup_limits(width: f32, viewport_w: f32, viewport_h: f32, inset: f32) -> (f32, f32) {
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
