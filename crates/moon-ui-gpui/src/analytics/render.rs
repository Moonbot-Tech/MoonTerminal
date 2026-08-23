//! Rendering for the Analytics window and its quote-safety fallback.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::QuoteBreakdown;
use moon_core::db::valuation::{ValuationMode, ValuationStatus};
use moon_ui::{MoonAlert, MoonPalette, MoonWindowFrame, h_flex, v_flex};
use rust_i18n::t;

use super::{ANALYTICS_HEADER_H, AnalyticsView, Tab, set_pnl_unit};
use crate::design::{moon, moon_alpha};
use crate::{design, valuation_health};

impl Render for AnalyticsView {
    /// Render the active Analytics tab with quote-safe scope state and shared chrome.
    ///
    /// Args:
    ///     window: Owning window used for responsive chrome width.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Complete Analytics surface.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.display_zone_fields_dirty {
            self.display_zone_fields_dirty = false;
            self.sync_period_pickers(window, cx);
        }
        let p = MoonPalette::active(cx);
        let (unit, split) = match self.tab {
            Tab::Summary => (self.data.unit(), self.data.split().cloned()),
            Tab::Strategies => (
                self.strategy_data.unit(),
                self.strategy_data.split().cloned(),
            ),
            Tab::Calendar => (self.cal_days.unit(), self.cal_days.split().cloned()),
        };
        // Arm shared formatters with the exact comparable unit published by the active tab.
        set_pnl_unit(unit);
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(b)
            | WindowBounds::Maximized(b)
            | WindowBounds::Fullscreen(b) => f32::from(b.size.width),
        };
        let body = match split {
            Some(totals) => {
                quote_split_note(&totals, &self.valuation_status, self.valuation_mode, p, cx)
            }
            None => match self.tab {
                Tab::Summary => self.summary_tab(p, cx),
                Tab::Strategies => self.strategies_tab(p, window, cx),
                Tab::Calendar => self.calendar_tab(p, cx),
            },
        };
        // Tabs divide their own height, pinning bottom bars to the window and scrolling content
        // internally, so there is no outer scroll.
        let body_scrolls = false;
        let integrity = self.integrity_note(cx);
        let write_error = self.write_error.clone();
        let busy_overlay = self.busy_overlay_due();
        v_flex()
            .size_full()
            .relative()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .child(analytics_header(p, cx))
            .child(self.tabs_bar(p, cx))
            // Calendar has its OWN month navigation, so hide the from/to period bar there; its
            // body has a separate Previous/month/Next row.
            .when(self.tab != Tab::Calendar, |el| {
                el.child(self.period_bar(p, cx))
            })
            // Show the integrity banner on EVERY tab: a damaged replica matters on Calendar too,
            // because it reads the same database.
            .when_some(integrity, |el, (title, detail)| {
                el.child(
                    // Do not use `.banner()`: MoonAlert renders the title only in the
                    // non-banner form (alert.rs `when(!self.banner, ..title..)`),
                    // so the banner variant would drop the localized heading and
                    // show the bare SQLite diagnostic line.
                    div()
                        .px(design::ui_px(cx, 10.0))
                        .pb(design::ui_px(cx, 6.0))
                        .child(MoonAlert::warning("an-integrity-banner", detail).title(title)),
                )
            })
            // A write that reached nobody. Above the undated-close notice deliberately: that one is
            // about numbers being incomplete, this one is about the user's strategies not
            // having changed when they were told they had.
            .when_some(write_error, |el, msg| {
                el.child(
                    div()
                        .px(design::ui_px(cx, 10.0))
                        .pb(design::ui_px(cx, 6.0))
                        .child(
                            h_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 6.0))
                                .items_start()
                                .child(
                                    div().flex_1().min_w_0().child(
                                        MoonAlert::error("an-write-error", msg)
                                            .title(t!("analytics.write_failed_title").to_string()),
                                    ),
                                )
                                .child(
                                    moon_ui::MoonButton::new("an-write-error-x")
                                        .variant(moon_ui::MoonButtonVariant::Ghost)
                                        .size(moon_ui::MoonButtonSize::Micro)
                                        .label(t!("analytics.write_failed_ok").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.write_error = None;
                                            cx.notify();
                                        }))
                                        .render(),
                                ),
                        ),
                )
            })
            // Keep omitted money adjacent to the period bar; `notice_strip` returns no element
            // when the scoped query has nothing to report.
            .children(self.notice_strip(p, cx))
            .child(
                div()
                    .id("analytics-body")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .when(body_scrolls, |el| el.overflow_y_scroll())
                    .child(body),
            )
            // If a background operation outlasts the delay, dim the window and occlude clicks;
            // otherwise long scans are invisible while clicks accumulate.
            .when(busy_overlay, |el| {
                el.child(
                    div()
                        .id("an-busy-overlay")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(moon_alpha(p.shell, 0.45))
                        .child(
                            h_flex()
                                .px(design::ui_px(cx, 14.0))
                                .py(design::ui_px(cx, 7.0))
                                .rounded(design::ui_px(cx, 6.0))
                                .bg(moon(p.panel_high))
                                .border_1()
                                .border_color(moon(p.border))
                                .text_size(design::t_body(cx))
                                .text_color(moon(p.text_soft))
                                .child(t!("common.loading").to_string()),
                        ),
                )
            })
            .child(
                MoonWindowFrame::tool("analytics-window-frame-hit", chrome_width)
                    .header_height(ANALYTICS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

/// Render the safe replacement for raw analytics over mixed or unknown quote currencies.
///
/// Args:
///     totals: Known quote buckets and unknown row count for the active scope.
///     status: Published valuation worker health, explaining a coverage figure that stopped moving.
///     mode: Which conversion produced the coverage figures.
///     p: Active MoonUI palette.
///     cx: Analytics render context.
///
/// Returns:
///     A centered, wrapping explanation with split totals and recovery guidance.
fn quote_split_note(
    totals: &QuoteBreakdown,
    status: &ValuationStatus,
    mode: ValuationMode,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let mut chips = h_flex().flex_wrap().justify_center().gap_2();
    for total in &totals.totals {
        // Colour from the sign the TEXT shows, not from the raw amount: a loss too small to survive
        // this currency's rounding prints as a plus and must not still be tinted red.
        let (amount, sign) = total.signed_display();
        chips = chips.child(
            div()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(moon(p.table_head))
                .text_color(moon(sign.pick(p.green, p.red, p.text_soft)))
                .child(amount),
        );
    }
    if totals.unknown_orders > 0 {
        chips = chips.child(
            div()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(moon(p.table_head))
                .text_color(moon(p.orange))
                .child(t!("analytics.quote_unknown_orders", n = totals.unknown_orders).to_string()),
        );
    }
    let coverage_note = totals.valuation.and_then(|coverage| {
        (coverage.eligible_orders > 0).then(|| {
            // The current-rate wording is shared with the Report footer rather than duplicated:
            // rust-i18n keeps one global key namespace, and one concept deserves one sentence.
            let mut text = t!(
                mode.key(
                    "analytics.quote_valuation_progress",
                    "report.valuation_current_progress"
                ),
                ready = coverage.valued_orders,
                total = coverage.eligible_orders
            )
            .to_string();
            if coverage.unavailable_orders > 0 {
                text.push_str(" · ");
                text.push_str(
                    &t!(
                        mode.key(
                            "analytics.quote_valuation_unavailable",
                            "report.valuation_current_unavailable"
                        ),
                        n = coverage.unavailable_orders
                    )
                    .to_string(),
                );
            }
            // Without this the ratio above simply stops moving, which reads as a slow backfill
            // however long it has actually been stuck.
            let stalled = valuation_health::stall_facts(status, moon_core::util::now_unix_ms_i64());
            if let Some(facts) = stalled {
                text.push_str(" · ");
                text.push_str(
                    &t!(
                        "analytics.quote_valuation_stalled",
                        stage = facts.stage,
                        kind = facts.kind,
                        minutes = facts.minutes
                    )
                    .to_string(),
                );
            }
            text
        })
    });
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.orange))
                .child(t!("analytics.quote_split_title").to_string()),
        )
        .child(
            div()
                .max_w(design::font_w_px(cx, 700.0))
                .text_center()
                .text_color(moon(p.text_soft))
                .child(
                    if totals.unknown_orders > 0 {
                        t!("analytics.quote_unknown_detail")
                    } else {
                        t!("analytics.quote_split_detail")
                    }
                    .to_string(),
                ),
        )
        .child(chips)
        .children(coverage_note.map(|note| {
            div()
                .text_color(moon(p.orange))
                .child(note)
                .into_any_element()
        }))
        .child(
            div()
                .text_color(moon(p.text_muted))
                .child(t!("analytics.quote_split_orders", n = totals.orders).to_string()),
        )
        .into_any_element()
}

/// Render the Analytics title bar with the current custom-window-control policy.
fn analytics_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("analytics-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, ANALYTICS_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("analytics-titlebar-title", 0.0)
                .title_cluster(t!("analytics.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("analytics-window-frame-visual", 0.0)
                    .header_height(ANALYTICS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}
