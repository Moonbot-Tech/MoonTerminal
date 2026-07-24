//! Group-window status bar with glanceable connection, market/render, resource, and action groups.

use gpui::*;
use rust_i18n::t;

use moon_ui::{MoonPalette, MoonStatusBar, MoonStatusIndicator, MoonStatusItem, MoonTooltipView};

use moon_core::feed::ConnStatus;
use moon_core::metrics::MetricsSnapshot;
use moon_core::session::{ConnSummary, LicenseSummary};

use crate::design;

use super::Shell;

impl Shell {
    /// Build the lower status bar as three semantic groups plus right-aligned actions.
    ///
    /// The left connection indicator is green when every core is ready, red when any core failed
    /// or disconnected, and amber otherwise. Its tooltip lists non-ready cores. The row preserves
    /// license, book/FPS, CPU/GPU, current RAM, and five-second RAM-delta telemetry while stronger
    /// dividers separate those concerns.
    ///
    /// Args:
    ///     conn: Connection summary for the cores in this group window.
    ///     license: Aggregated license state for the same cores.
    ///     snap: Current process and system resource telemetry.
    ///     book_levels: Order-book level count for the active Main chart.
    ///     fps: Smoothed shell render rate.
    ///     chrome_width: Current window width used to select the compact narrow layout.
    ///     cx: Application context used for theme tokens, text measurement, and actions.
    ///
    /// Returns:
    ///     The complete scaled status row with connection tooltip and clickable right actions.
    pub(super) fn status_bar(
        &self,
        conn: ConnSummary,
        license: LicenseSummary,
        snap: MetricsSnapshot,
        book_levels: usize,
        fps: f32,
        chrome_width: f32,
        cx: &App,
    ) -> impl IntoElement {
        let all_ok = conn.total > 0 && conn.ready == conn.total;
        let any_failed = conn
            .down
            .iter()
            .any(|(_, _, s)| matches!(s, ConnStatus::Failed(_) | ConnStatus::Disconnected));
        let p = MoonPalette::active(cx);
        let badge_col = if all_ok {
            p.green
        } else if any_failed {
            p.red
        } else {
            p.amber
        };
        // Include only non-ready cores in the tooltip, formatted as name and reason.
        let down_text: String = conn
            .down
            .iter()
            .filter_map(|(_, name, st)| {
                let reason = match st {
                    ConnStatus::Connecting => t!("status.connecting").to_string(),
                    ConnStatus::Stage(s) => s.clone(),
                    ConnStatus::Failed(e) => e.clone(),
                    ConnStatus::Disconnected => t!("status.disconnected").to_string(),
                    ConnStatus::Ready => return None,
                };
                Some(format!("{name}: {reason}"))
            })
            .collect::<Vec<_>>()
            .join("\n");

        // The caption is translated WHOLE, colon included (`%{value}`), rather than glued together
        // as "label + ': ' + tail": punctuation and word order are the translator's business. The
        // tail itself is "OK" or a pair of numbers, identical in every locale.
        let connection_value = if all_ok {
            "OK".to_string()
        } else {
            format!("{}/{}", conn.ready, conn.total)
        };
        let status_text = t!("status.connection", value = connection_value.clone()).to_string();
        // PRO/FREE are plan names, on the deliberately-untranslated list (locales/README.md).
        let (license_value, license_color) = if license.total == 0 || license.known == 0 {
            ("…".to_string(), p.text_muted)
        } else if license.known < license.total {
            (format!("{}/{}", license.known, license.total), p.amber)
        } else if license.paid == license.total {
            ("PRO".to_string(), p.green)
        } else if license.free == license.total {
            ("FREE".to_string(), p.amber)
        } else {
            (format!("PRO {}/{}", license.paid, license.total), p.amber)
        };
        let license_text = t!("status.license", value = license_value.clone()).to_string();

        // At the supported 520px minimum, full localized captions cannot coexist with every live
        // metric and both right actions. Compact only the captions and spacing; every value remains
        // visible, while the host tooltip expands the app/system CPU meaning and exact units.
        let compact = chrome_width < design::font_w(cx, 880.0).max(design::ui_value(cx, 880.0));
        let status_gap = if compact { 4.0 } else { 8.0 };
        let inner_gap = if compact { 2.0 } else { 5.0 };
        let value_gap = if compact { 4.0 } else { 8.0 };
        let group_gap = if compact { 5.0 } else { 10.0 };
        let tooltip_text = if compact {
            let mut text = format!(
                "{status_text}\n{license_text}\nBOOK {book_levels} · FPS {fps:.0}\n\
                 CPU APP/SYS {:.1}% / {:.1}% · GPU {:.1}%\n\
                 RAM {:.1} MiB · Δ5s {:+.1} MiB",
                snap.cpu_process, snap.cpu_system, snap.gpu_process, snap.mem_mb, snap.mem_delta_mb
            );
            if !down_text.is_empty() {
                text.push_str("\n\n");
                text.push_str(&down_text);
            }
            Some(text)
        } else if down_text.is_empty() {
            None
        } else {
            Some(down_text)
        };

        let link_label = "moonbot.pro";
        let mut right_items = Vec::new();
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            right_items.extend([
                MoonStatusItem::new("debug").color(p.amber).gap_after(8.0),
                MoonStatusItem::group_separator().gap_after(8.0),
            ]);
        }
        right_items.push(MoonStatusItem::new(link_label).color(p.blue).gap_after(0.0));

        let mut host = div()
            .id("status-bar-host")
            .w_full()
            // Mirror MoonStatusBar's default 22/13/4.5 fit triple so the host, its child, and the
            // transparent action hitboxes grow together when the Font slider increases.
            .h(design::fit_h_px(cx, design::STATUS_H, 13.0, 4.5))
            .relative()
            .child(
                MoonStatusBar::new("status-bar")
                    .indicator(
                        MoonStatusIndicator::new(badge_col)
                            .alpha(0.685)
                            .size(6.0)
                            .glow(8.0, 0.30),
                    )
                    .items([
                        MoonStatusItem::new(if compact {
                            connection_value
                        } else {
                            status_text
                        })
                        .color(badge_col)
                        .weight(600.0)
                        .gap_after(status_gap),
                        MoonStatusItem::separator().gap_after(status_gap),
                        MoonStatusItem::new(if compact { license_value } else { license_text })
                            .color(license_color)
                            .weight(600.0)
                            .gap_after(group_gap),
                        MoonStatusItem::group_separator().gap_after(group_gap),
                        MoonStatusItem::new("BOOK")
                            .color(p.text_muted)
                            .gap_after(inner_gap),
                        MoonStatusItem::new(format!("{book_levels}"))
                            .color(p.text_soft)
                            .gap_after(value_gap),
                        MoonStatusItem::new("FPS")
                            .color(p.text_muted)
                            .gap_after(inner_gap),
                        MoonStatusItem::new(format!("{fps:.0}"))
                            .color(p.text_soft)
                            .gap_after(group_gap),
                        MoonStatusItem::group_separator().gap_after(group_gap),
                        MoonStatusItem::new(if compact { "CPU" } else { "CPU APP/SYS" })
                            .color(p.text_muted)
                            .gap_after(inner_gap),
                        MoonStatusItem::new(if compact {
                            format!("{:.0}/{:.0}%", snap.cpu_process, snap.cpu_system)
                        } else {
                            format!("{:.0}% / {:.0}%", snap.cpu_process, snap.cpu_system)
                        })
                        .color(p.text_soft)
                        .gap_after(value_gap),
                        MoonStatusItem::new("GPU")
                            .color(p.text_muted)
                            .gap_after(inner_gap),
                        MoonStatusItem::new(format!("{:.0}%", snap.gpu_process))
                            .color(p.text_soft)
                            .gap_after(value_gap),
                        MoonStatusItem::new("RAM")
                            .color(p.text_muted)
                            .gap_after(inner_gap),
                        MoonStatusItem::new(if compact {
                            format!("{:.1}GiB", snap.mem_mb / 1024.0)
                        } else {
                            format!("{:.1} GiB", snap.mem_mb / 1024.0)
                        })
                        .color(p.text_soft)
                        .gap_after(value_gap),
                        MoonStatusItem::new(if compact {
                            format!("Δ5s{:+.1}MiB", snap.mem_delta_mb)
                        } else {
                            format!("Δ5s {:+.1} MiB", snap.mem_delta_mb)
                        })
                        .color(p.text_muted)
                        .gap_after(0.0),
                    ])
                    .right_items(right_items)
                    .render(),
            );
        // MoonStatusItem has no click handler. Measure the rendered monospaced label instead of
        // keeping the old approximate fixed width, then overlay only its hit area.
        let action_pad = design::ui_value(cx, 3.0);
        let right_offset = design::ui_value(cx, 8.0);
        let link_width = design::ui_text_width(cx, link_label, 10.0, 400.0, true);
        host = host.child(
            div()
                .id("moonbot-link")
                .absolute()
                .top_0()
                .bottom_0()
                .right(px((right_offset - action_pad).max(0.0)))
                .w(px(link_width + action_pad * 2.0))
                .cursor_pointer()
                .tooltip(|_window, cx| {
                    cx.new(|_| moon_ui::MoonTooltipView::new("moonbot.pro"))
                        .into()
                })
                .on_click(|_, _window, cx: &mut App| cx.open_url("https://moonbot.pro")),
        );
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            let backend = self.backend.clone();
            let debug_width = design::ui_text_width(cx, "debug", 10.0, 400.0, true);
            let debug_right =
                right_offset + link_width + design::ui_value(cx, 8.0 + 1.0 + 8.0) - action_pad;
            host = host.child(
                div()
                    .id("debug-status-open")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(debug_right))
                    .w(px(debug_width + action_pad * 2.0))
                    .cursor_pointer()
                    .tooltip(|_window, cx| cx.new(|_| MoonTooltipView::new("debug")).into())
                    .on_click({
                        let group = self.group.clone();
                        move |_, window, cx| {
                            crate::diagnostics::debug_window::open_debug_perf_window(
                                cx,
                                backend.clone(),
                                group.clone(),
                                Some(window.window_handle()),
                            )
                        }
                    }),
            );
        }
        if let Some(tooltip_text) = tooltip_text {
            host = host.tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tooltip_text.clone()).max_width(420.0))
                    .into()
            });
        }
        host
    }
}
