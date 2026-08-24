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
            .any(|row| matches!(row.status, ConnStatus::Failed(_) | ConnStatus::Disconnected));
        let p = MoonPalette::active(cx);
        let badge_col = if all_ok {
            p.green
        } else if any_failed {
            p.red
        } else {
            p.amber
        };
        // Include only non-ready cores in the tooltip, each as name, reason and NEXT STEP.
        //
        // This is the surface a user who has opened no panel at all reads first, so it carries the
        // action and not just the complaint. It used to print the raw `ConnStatus` payload: an
        // English stage name such as "connected, init..." and a MoonProto error string, both built
        // in a crate that cannot translate them. The verdict replaces both, and the two remaining
        // arms are the states that genuinely carry no further evidence.
        let down_text: String = conn
            .down
            .iter()
            .map(|row| {
                let reason = match moon_core::feed::diagnose(
                    &row.status,
                    row.fault.as_ref(),
                    &row.startup,
                ) {
                    Some(d) => crate::conn_diag::fault_line(&d),
                    None => match row.status {
                        ConnStatus::Disconnected => t!("status.disconnected").to_string(),
                        _ => t!("status.connecting").to_string(),
                    },
                };
                format!("{}: {reason}", row.name)
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

        let mut right_items = Vec::new();
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            let backend = self.backend.clone();
            let group = self.group.clone();
            right_items.extend([
                MoonStatusItem::new("debug")
                    .id("debug-status-open")
                    .color(p.amber)
                    .gap_after(8.0)
                    .tooltip("debug")
                    .on_click(move |_, window, cx| {
                        crate::diagnostics::debug_window::open_debug_perf_window(
                            cx,
                            backend.clone(),
                            group.clone(),
                            Some(window.window_handle()),
                        )
                    }),
                MoonStatusItem::group_separator().gap_after(8.0),
            ]);
        }
        right_items.push(
            MoonStatusItem::new("moonbot.pro")
                .id("moonbot-link")
                .color(p.blue)
                .gap_after(0.0)
                .tooltip("moonbot.pro")
                .on_click(|_, _window, cx| cx.open_url("https://moonbot.pro")),
        );

        let mut host = div()
            .id("status-bar-host")
            .w_full()
            // Mirror MoonStatusBar's default 22/13/4.5 fit triple so the aggregate telemetry
            // tooltip owns the same full-height region as the scaled status row.
            .h(design::fit_h_px(cx, design::STATUS_H, 13.0, 4.5))
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
        if let Some(tooltip_text) = tooltip_text {
            host = host.tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tooltip_text.clone()).max_width(420.0))
                    .into()
            });
        }
        host
    }
}
