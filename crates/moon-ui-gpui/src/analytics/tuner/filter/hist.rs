//! Tuner cards: the "Fact vs variants" KPI matrix and the histogram (the bottom
//! strip of the "Filters" mode) — the distribution of profit/loss and trades
//! across the quantile buckets of the selected field. Split out of tuner.rs
//! (file size limit).
//!
//! `kpi_matrix_card` is UNIVERSAL (a free function): it draws purely out of
//! `VarStats`, so every tuning mode reuses it (Filter/Time/…).

use gpui::*;
use moon_ui::{h_flex, v_flex, MoonPalette};
use rust_i18n::t;

use super::super::super::summary::{fmt_signed, sign_color};
use super::super::super::AnalyticsView;
use super::super::kpi::kpi_matrix_card;
use super::super::shared::collapse_caret;
use super::card;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::FIELDS;

impl AnalyticsView {
    /// KPI matrix of the "By filter" mode: scope is the SELECTION, columns v1/v2.
    pub(in crate::analytics::tuner) fn kpi_matrix(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        // An empty label list → the "v{i}" fallback (as it historically was).
        kpi_matrix_card(
            &self.tuner.stats,
            self.scope_label(),
            &[],
            self.kpi_collapsed,
            p,
            cx,
        )
    }

    /// Histogram of the selected field: wins up, losses down, the count and the edges.
    pub(in crate::analytics::tuner) fn hist_card(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        // The title carries the field and the scope (strategy name / count / all).
        let scope = self.scope_label();
        let title = format!(
            "{} — {} — {}",
            t!("analytics.tuner.hist_title"),
            FIELDS[self.tuner.sel_field].label,
            scope,
        );
        // Built before the body so it does not blink out while the histogram is loading or
        // after a read error — the caret belongs to the title bar in every state.
        let caret = collapse_caret(
            "an-tuner-hist-collapse",
            self.hist_collapsed,
            t!("analytics.tuner.hist_collapse").to_string(),
            t!("analytics.tuner.hist_expand").to_string(),
            p,
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| this.toggle_hist_collapsed(cx)))
        .into_any_element();
        // Collapsed skips the bar geometry entirely, but NOT the read behind it: the histogram
        // keeps loading, so expanding shows the chart rather than a spinner.
        let body = if self.hist_collapsed {
            div().into_any_element()
        } else {
            self.hist_body(p, cx)
        };
        card(
            title,
            t!("analytics.tuner.hist_sub").to_string(),
            body,
            Some(caret),
            p,
            cx,
        )
    }

    /// Render the distribution body from its current load state.
    ///
    /// Keeping this separate from [`Self::hist_card`] lets a collapsed card skip all bar geometry
    /// while the surrounding title, subtitle, caret, and histogram read remain active.
    ///
    /// Args:
    ///     p: Active palette for the chart and read-state notice.
    ///     cx: Analytics context used for scaled geometry and text.
    ///
    /// Returns:
    ///     The chart body, or the current loading/error notice.
    fn hist_body(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        match self.tuner.hist.view(|h| h.is_empty()) {
            Err(note) => super::super::super::note_el("an-tuner-hist-note", note, 8.0, p, cx),
            Ok(h) => {
                let max = h.iter().map(|b| b.wsum.max(b.lsum)).fold(1e-9f64, f64::max);
                let half = design::ui_px(cx, 74.0);
                let mut row = h_flex().w_full().gap(design::ui_px(cx, 3.0)).items_start();
                for b in h.iter() {
                    let up = ((b.wsum / max) as f32).clamp(0.0, 1.0);
                    let dn = ((b.lsum / max) as f32).clamp(0.0, 1.0);
                    row = row.child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(2.0))
                            // Wins (up from the axis).
                            .child(
                                div()
                                    .w_full()
                                    .h(half)
                                    .flex()
                                    .items_end()
                                    .justify_center()
                                    .child(
                                        div()
                                            .w(relative(0.62))
                                            .h(relative(up.max(if b.wsum > 0.0 {
                                                0.02
                                            } else {
                                                0.0
                                            })))
                                            .rounded_t(px(2.0))
                                            .bg(moon(p.green)),
                                    ),
                            )
                            // Losses (down from the axis).
                            .child(
                                div()
                                    .w_full()
                                    .h(half)
                                    .flex()
                                    .items_start()
                                    .justify_center()
                                    .border_t_1()
                                    .border_color(moon_alpha(p.border, 0.8))
                                    .child(
                                        div()
                                            .w(relative(0.62))
                                            .h(relative(dn.max(if b.lsum > 0.0 {
                                                0.02
                                            } else {
                                                0.0
                                            })))
                                            .rounded_b(px(2.0))
                                            .bg(moon(p.orange)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(sign_color(p, b.wsum - b.lsum)))
                                    .child(fmt_signed(b.wsum - b.lsum)),
                            )
                            .child(
                                div()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_soft))
                                    .child(b.n.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_muted))
                                    .child(short_num(b.lo)),
                            ),
                    );
                }
                v_flex()
                    .w_full()
                    .px(design::ui_px(cx, 8.0))
                    .pb(design::ui_px(cx, 6.0))
                    .child(row)
                    .into_any_element()
            }
        }
    }
}

fn short_num(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}
