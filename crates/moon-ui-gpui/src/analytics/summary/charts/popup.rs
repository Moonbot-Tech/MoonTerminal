//! Summary chart popup helpers.

use std::rc::Rc;

use super::*;

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

/// Wrap a finished popup card in the shared window-aware overlay.
///
/// Args:
///     card: Rows already laid out inside [`popup_shell`].
///     content_w: Widest line of the card, measured by the caller.
///     frac: Anchor across the chart's laid-out width, 0..=1.
///     key: Chart bucket identity for scroll state and hover fences.
///     on_hover: Called with `true`/`false` as the pointer enters or leaves the popup.
///     p: Active palette.
///     cx: App context for scaled sizes.
///
/// Returns:
///     Deferred popup anchored beside the bucket.
pub(in crate::analytics::summary) fn popup_overlay(
    card: Div,
    content_w: f32,
    frac: f32,
    key: PopupKey,
    on_hover: Rc<dyn Fn(bool, &mut Window, &mut App)>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    PopupOverlay {
        card,
        width: popup_w(content_w, cx),
        frac,
        key,
        on_hover,
        palette: p,
    }
    .into_any_element()
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
///     content_w: Widest line the card will draw, measured by the caller in the card's own font.
///     cx: Analytics view context.
///
/// Returns:
///     Outer width for the popup holder.
pub(super) fn popup_w(content_w: f32, cx: &App) -> Pixels {
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

/// Empty card of a bucket popup — shared chrome, so the cumulative chart's popup and the
/// daily one cannot drift apart in looks.
pub(in crate::analytics::summary) fn popup_shell(p: MoonPalette, cx: &App) -> Div {
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
pub(in crate::analytics::summary) fn profit_trades(v: f64, trades: i64) -> String {
    format!("{} ({trades})", super::fmt_signed(v))
}

/// One core's line in a bucket popup: colour dot, name, `+500 (123)`.
pub(in crate::analytics::summary) fn popup_core_row(
    name: String,
    dot: Hsla,
    v: f64,
    trades: i64,
    p: MoonPalette,
    cx: &App,
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
    /// Pointer enter/leave of the popup, routed to the view's hover fences.
    on_hover: Rc<dyn Fn(bool, &mut Window, &mut App)>,
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
        let on_hover = self.on_hover;
        let popup = div()
            .id(id.clone())
            .relative()
            .occlude()
            .w(px(width))
            .on_hover(move |hovered, window, cx| on_hover(*hovered, window, cx))
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
