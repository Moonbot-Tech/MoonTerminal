//! Chrome, layout and the state overlays of the trade-detail window.
//!
//! Every state renders inside the SAME frame: header, chart area, figures rail. Only the chart
//! area changes, so a state transition reads as the chart answering rather than as the window
//! rebuilding itself, and the trade's own numbers stay legible throughout — including while the
//! fetch is still running and while it has failed outright.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::market::trade_replay::{
    TickStatus, TradeReplayEmpty, TradeReplayFailure, TradeReplaySource,
};
use moon_ui::{
    MoonButton, MoonButtonSize, MoonPalette, MoonWindowFrame, MoonWindowFrameControls, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::{TradeWindowState, TradeWindowView, figures};
use crate::design;
use crate::design::moon;

/// Width below which the figures rail moves from a side column to a wrapped header strip.
///
/// A panel owes a DEFINED narrow behaviour, and a horizontal scrollbar is not one. Below this the
/// rail would squeeze the chart into a sliver; above it a side column keeps the numbers beside the
/// picture where they are compared.
///
/// The arithmetic, recorded so it is not re-derived: the rail's own `RAIL_W` (200, in `figures`)
/// plus the narrowest chart still worth drawing (360). It happens to be the value this constant
/// already carried, so widening the rail did not move it.
///
/// One honest caveat: `window.rs` opens this window with a `MIN_W` of 720, so the OS should
/// never hand us a viewport under that and this branch is unreachable in practice. It stays
/// because the floor is the platform's promise rather than ours — a window manager that ignores a
/// minimum size must still get a defined layout, not a rail one character wide. Do NOT raise this
/// to or past 720: that would flip EVERY window to the wrapped strip.
const NARROW_W: f32 = 560.0;

/// Nominal header height before UI scaling.
const HEADER_H: f32 = 34.0;

/// Measure of the state sentence in the empty chart area, in logical pixels before the font scale.
///
/// PIXELS, which is what [`design::font_w_px`] takes — it returns font-scaled pixels and has never
/// taken a character count. Reading it as one is what put `44.0` here and rendered the sentence as
/// a 44-pixel column of single words, breaking mid-word. The value is a readable measure for the
/// longest string the dictionary holds for these states, so the sentence wraps into two or three
/// lines at word boundaries.
const MESSAGE_MAX_W: f32 = 360.0;

/// Whether the figures rail wraps into a header strip instead of standing as a side column.
///
/// A free function, and a pure one, so the DECISION can be exercised without a window: a test that
/// only compared the constants would stay green through an inverted comparison or a flipped
/// branch, which is exactly the mutation this layout rule needs to be able to fail on.
///
/// Args:
///     viewport_w: The window's current width in logical pixels.
///
/// Returns:
///     `true` when the rail must wrap.
pub(super) fn rail_wraps(viewport_w: f32) -> bool {
    viewport_w < NARROW_W
}

impl Render for TradeWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::hotkeys::restore_root_focus(&self.focus, window, cx);
        let p = MoonPalette::active(cx);
        let narrow = rail_wraps(f32::from(window.viewport_size().width));
        // The header's ACTUAL height, not the constant it is built from: that constant is scaled,
        // so a raw value drifts under a non-default UI or font scale.
        let header_h = design::fit_h_px(cx, HEADER_H, 13.0, 10.5);
        let frame = MoonWindowFrame::detached_chart("trade-window-frame", 0.0)
            .header_height(HEADER_H)
            .controls(MoonWindowFrameControls::Close)
            .show_controls(design::show_custom_window_controls());
        let title = format!("{} · {}", self.record.coin, self.stamps.1);
        // Built per branch rather than cloned: `AnyElement` is single-use by construction, which
        // is also what keeps a stray second mount from silently sharing element ids.
        let body = self.chart_area(p, cx);
        v_flex()
            .size_full()
            .relative()
            // The root, not the chart, owns the keyboard: Escape is a WINDOW command. Capture
            // phase, because the chart panel below is focusable and would otherwise be free to
            // consume the press before it ever bubbles up here.
            .track_focus(&self.focus)
            .capture_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)),
            )
            .child(
                h_flex()
                    .h(header_h)
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, design::CHROME_GAP))
                    .pl(design::ui_px(cx, design::titlebar_leading_inset()))
                    .pr(design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(moon(p.border))
                    .bg(moon(p.shell_high))
                    .child(
                        frame
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    // THE VERTICAL-SCALE CONTROL: a look at this trade from another zoom, for as
                    // long as the window is open. The window itself opens on AUTO and fits the
                    // trade it was opened for — a remembered percentage fitted the trade it was
                    // chosen for and drew the next one off-screen, which read as a frozen chart.
                    //
                    // The chart's own badge states what the pane is CURRENTLY on and is part of
                    // this view's caption set; this trigger states what was PICKED. Reached
                    // through the `controls` facade because the `scale` module behind it is
                    // private.
                    //
                    // `chrome_section` is `flex_none`, for the same reason the close button below
                    // is: the title cluster beside it is `flex_1().min_w_0()` and would otherwise
                    // squeeze the trigger to nothing at the minimum window width. The row's own
                    // `CHROME_GAP` supplies the space on both sides of the rule, so the divider
                    // carries no margin of its own.
                    .child(design::chrome_divider(cx, p))
                    .child(design::chrome_section(cx).child(
                        crate::controls::scale_dropdown_for_trade_window(
                            cx,
                            self.scale(cx),
                            cx.entity(),
                            p,
                        ),
                    ))
                    // THE CLOSE BUTTON. `.controls(...)` and `.show_controls(...)` above only
                    // CONFIGURE the frame; the buttons are drawn by this separate call, and
                    // without it the window has no chrome affordance at all. That is not a
                    // cosmetic loss here: `trade_window_options` hides the taskbar button too,
                    // so an unmounted control leaves the window with no way out.
                    //
                    // `flex_none` because the sibling title cluster is `flex_1().min_w_0()` and
                    // would otherwise squeeze the button to zero at the minimum window width.
                    .when(design::show_custom_window_controls(), |el| {
                        el.child(frame.visual_controls(cx).flex_none())
                    }),
            )
            .when(narrow, |el| {
                el.child(figures::rail(&self.record, &self.stamps, true, p, cx))
            })
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    // The chart renders in its own GPU pass UNDER the GPUI scene, so nothing here
                    // may paint an opaque background over the chart body.
                    .child(div().flex_1().min_w_0().h_full().relative().child(body))
                    .when(!narrow, |el| {
                        el.child(figures::rail(&self.record, &self.stamps, false, p, cx))
                    }),
            )
    }
}

impl TradeWindowView {
    /// The chart, or the overlay that stands in for it.
    ///
    /// Args:
    ///     p: Active palette.
    ///     cx: View context.
    ///
    /// Returns:
    ///     The chart area's content.
    fn chart_area(&self, p: MoonPalette, cx: &mut Context<Self>) -> AnyElement {
        if !self.overlays_chart() {
            // EXHAUSTIVE over `Ready`, with no catch-all arm. The `_ =>` that used to stand here is
            // exactly what let the caption claim "minute candles" over five-minute bars: a
            // catch-all cannot be wrong about a state it never looked at, so it was never checked.
            // The timeframe now comes from the rows themselves.
            //
            // The number goes straight into the placeholder rather than through
            // `moon_core::util::fmt`: that module formats amounts — grouping, decimals, compaction
            // — and a timeframe of 1 to 60 has nothing to group. Do not "fix" this into `fmt`.
            let caption = match &self.state {
                TradeWindowState::Ready {
                    source: TradeReplaySource::Ticks,
                    tf_min,
                    bucket_ms,
                    partial,
                    ..
                } => {
                    let base = if *bucket_ms == 0 {
                        t!("trade_window.source.ticks").to_string()
                    } else {
                        t!(
                            "trade_window.source.ticks_bucketed",
                            secs = bucket_ms / 1_000
                        )
                        .to_string()
                    };
                    if *partial {
                        // The join happens IN CODE, so no locale value carries a separator glyph.
                        // Every sibling Russian caption in `trade_window.yml` joins its two halves
                        // with an em dash, never the ASCII hyphen the other locales use — so the
                        // glyph itself must follow the active locale, not just the words either
                        // side of it.
                        let edges = t!("trade_window.source.ticks_edges", min = tf_min).to_string();
                        let sep = match rust_i18n::locale().as_ref() {
                            "ru" => "—",
                            _ => "-",
                        };
                        format!("{base} {sep} {edges}")
                    } else {
                        base
                    }
                }
                TradeWindowState::Ready {
                    source: TradeReplaySource::Klines1m,
                    tf_min,
                    tick_status,
                    brand,
                    ..
                } => match tick_status {
                    TickStatus::Pending => {
                        t!("trade_window.source.candles_ticks_pending", min = tf_min).to_string()
                    }
                    TickStatus::NoRoute => t!(
                        "trade_window.source.candles_no_route",
                        min = tf_min,
                        brand = brand.display()
                    )
                    .to_string(),
                    TickStatus::OutOfRetention { retention_ms } => t!(
                        "trade_window.source.candles_retention",
                        min = tf_min,
                        hours = retention_ms / 3_600_000
                    )
                    .to_string(),
                    TickStatus::NoTrades => {
                        t!("trade_window.source.candles_no_trades", min = tf_min).to_string()
                    }
                    TickStatus::Failed => {
                        t!("trade_window.source.candles_failed", min = tf_min).to_string()
                    }
                    // Unreachable: `Served` only ever rides a `Ticks` source (see `TickStatus`'s
                    // own doc comment), never a `Klines1m` one. Answered rather than panicked, the
                    // way the `Loading | Empty | Failed` arm below answers its own unreachable
                    // case.
                    TickStatus::Served => String::new(),
                },
                // Unreachable: the caller checked `overlays_chart` first. Answered rather than
                // panicked, for the same reason `overlay_message` answers its own unreachable arm.
                TradeWindowState::Loading
                | TradeWindowState::Empty(_)
                | TradeWindowState::Failed(_) => String::new(),
            };
            return div()
                .size_full()
                .relative()
                .child(self.panel.clone())
                // The caption is a requirement, not decoration: a one-minute picture of a
                // forty-second scalp is an honest answer only while it says which it is.
                //
                // `t_body`, NOT `t_caption`: this is the one line saying WHAT is on screen —
                // ticks, bucketed ticks, or candles and the reason for them — so it must not be
                // the smallest text in the window. It now matches the figures rail's VALUES
                // (`figures.rs`, `t_body`) rather than its field labels, which is the right
                // company for it. A design step, never a hard-coded size, so the Font slider and
                // the UI scale keep carrying it.
                .child(
                    div()
                        .absolute()
                        .left(design::ui_px(cx, 8.0))
                        .bottom(design::ui_px(cx, 6.0))
                        .text_size(design::t_body(cx))
                        .text_color(moon(p.text_muted))
                        .child(caption),
                )
                .into_any_element();
        }
        let (message, tone) = self.overlay_message(p);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .max_w(design::font_w_px(cx, MESSAGE_MAX_W))
                    .text_center()
                    .text_size(design::t_body(cx))
                    .text_color(tone)
                    .child(message),
            )
            .when(self.state.retryable(), |el| {
                el.child(
                    MoonButton::new("trade-window-retry")
                        .size(MoonButtonSize::Micro)
                        .outline()
                        .label(t!("trade_window.retry").to_string())
                        .on_click(cx.listener(|this, _, _window, cx| this.fetch(cx)))
                        .render(),
                )
            })
            .into_any_element()
    }

    /// The sentence for the current non-chart state, and the tone to draw it in.
    ///
    /// Every arm is a DIFFERENT thing to tell the user; collapsing them into one "no data" is
    /// exactly the silent blank this window exists to replace.
    ///
    /// Args:
    ///     p: Active palette.
    ///
    /// Returns:
    ///     Localized message and its colour.
    fn overlay_message(&self, p: MoonPalette) -> (String, Hsla) {
        match &self.state {
            TradeWindowState::Loading => (
                t!("trade_window.state.loading").to_string(),
                moon(p.text_muted),
            ),
            // Unreachable: the caller checks `overlays_chart` first. Answered rather than
            // panicked, because a future state added above must not be able to crash a window.
            TradeWindowState::Ready { .. } => (String::new(), moon(p.text_muted)),
            TradeWindowState::Empty(empty) => {
                let text = match empty {
                    TradeReplayEmpty::NoDataInWindow => {
                        t!("trade_window.empty.no_data").to_string()
                    }
                    TradeReplayEmpty::NoEndpoint { brand } => {
                        // The brand crosses from moon-core as an ENUM and becomes text here; the
                        // sentence around it is the dictionary's.
                        t!("trade_window.empty.no_endpoint", brand = brand.display()).to_string()
                    }
                    TradeReplayEmpty::UnknownVenue => {
                        t!("trade_window.empty.unknown_venue").to_string()
                    }
                    TradeReplayEmpty::CoreNotConnected => {
                        t!("trade_window.empty.core_offline").to_string()
                    }
                    TradeReplayEmpty::DegenerateWindow => {
                        t!("trade_window.empty.bad_window").to_string()
                    }
                };
                (text, moon(p.text_muted))
            }
            TradeWindowState::Failed(failure) => {
                let text = match failure {
                    TradeReplayFailure::RateLimited { retry_in_s } => {
                        t!("trade_window.state.rate_limited", secs = retry_in_s).to_string()
                    }
                    // The transport's own words never reach the user; they went to the log.
                    TradeReplayFailure::Transient { .. } => {
                        t!("trade_window.state.failed").to_string()
                    }
                    TradeReplayFailure::UnknownSymbol => {
                        t!("trade_window.state.unknown_symbol").to_string()
                    }
                };
                (text, moon(p.amber))
            }
        }
    }
}
