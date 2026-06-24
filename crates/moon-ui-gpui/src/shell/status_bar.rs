//! Нижняя строка состояния окна группы (порт egui `shell::mod`): бейдж соединения,
//! лицензия и диагностика book/fps/CPU/GPU/RAM. Вынесено из `shell.rs`.

use gpui::*;
use rust_i18n::t;

use moon_ui::{MoonPalette, MoonStatusBar, MoonStatusIndicator, MoonStatusItem, MoonTooltipView};

use moon_core::feed::ConnStatus;
use moon_core::metrics::MetricsSnapshot;
use moon_core::session::{ConnSummary, LicenseSummary};

use crate::design;

use super::Shell;

impl Shell {
    /// Нижняя строка состояния (порт egui `shell::mod`): слева — бейдж соединения
    /// «● N/M подключено» (зелёный=все на связи, красный=есть упавшие, иначе янтарный)
    /// с тултипом по не-подключённым; затем диагностика book/fps/CPU/RAM.
    pub(super) fn status_bar(
        &self,
        conn: ConnSummary,
        license: LicenseSummary,
        snap: MetricsSnapshot,
        book_levels: usize,
        fps: f32,
        cx: &App,
    ) -> impl IntoElement {
        let all_ok = conn.total > 0 && conn.ready == conn.total;
        let any_failed = conn
            .down
            .iter()
            .any(|(_, s)| matches!(s, ConnStatus::Failed(_) | ConnStatus::Disconnected));
        let p = MoonPalette::active(cx);
        let badge_col = if all_ok {
            p.green
        } else if any_failed {
            p.red
        } else {
            p.amber
        };
        // Текст тултипа — только про НЕ подключённых (имя: причина).
        let down_text: String = conn
            .down
            .iter()
            .filter_map(|(name, st)| {
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

        let status_text = if all_ok {
            "Connection: OK".to_string()
        } else {
            format!("Connection: {}/{}", conn.ready, conn.total)
        };
        let (license_text, license_color) = if license.total == 0 || license.known == 0 {
            ("License: …".to_string(), p.text_muted)
        } else if license.known < license.total {
            (
                format!("License: {}/{}", license.known, license.total),
                p.amber,
            )
        } else if license.paid == license.total {
            ("PRO".to_string(), p.green)
        } else if license.free == license.total {
            ("FREE".to_string(), p.amber)
        } else {
            (format!("PRO {}/{}", license.paid, license.total), p.amber)
        };

        let mut host = div()
            .id("status-bar-host")
            .w_full()
            .h(px(design::STATUS_H))
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
                        MoonStatusItem::new(status_text)
                            .color(badge_col)
                            .weight(600.0)
                            .gap_after(10.0),
                        MoonStatusItem::new("Binance Futures")
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new("ping")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new("32ms")
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new("Mode:")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new("Demo")
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new(license_text)
                            .color(license_color)
                            .weight(600.0)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new("book")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new(format!("{book_levels}"))
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::new(format!("{fps:.0} fps"))
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new("CPU")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new(format!(
                            "{:.0}%/{:.0}%",
                            snap.cpu_process, snap.cpu_system
                        ))
                        .color(p.text_soft)
                        .gap_after(10.0),
                        MoonStatusItem::new("GPU")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new(format!("{:.0}%", snap.gpu_process))
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::new("RAM")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new(format!(
                            "{:.0} MB ({:+.1})",
                            snap.mem_mb, snap.mem_delta_mb
                        ))
                        .color(p.text_soft),
                    ])
                    .right_item(MoonStatusItem::new("moonbot.pro").color(p.blue))
                    .render(),
            );
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            let backend = self.backend.clone();
            host = host.child(
                div()
                    .id("debug-status-open")
                    .absolute()
                    .right(px(82.0))
                    .top(px(3.0))
                    .px(design::ui_px(cx, 6.0))
                    .h(design::fit_h_px(cx, 16.0, 10.0, 3.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .cursor_pointer()
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.amber))
                    .bg(rgba(0x00000044))
                    .hover(|s| s.bg(rgba(0x2A2520EE)).text_color(rgb(0xF7C663)))
                    .on_click({
                        let group = self.group.clone();
                        move |_, window, cx| {
                            crate::debug_window::open_debug_perf_window(
                                cx,
                                backend.clone(),
                                group.clone(),
                                Some(window.window_handle()),
                            )
                        }
                    })
                    .child("debug"),
            );
        }
        if !down_text.is_empty() {
            host = host.tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(down_text.clone()).max_width(420.0))
                    .into()
            });
        }
        host
    }
}
