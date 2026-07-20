//! Tuner cards: the "Fact vs variants" KPI matrix and the histogram (the bottom
//! strip of the "Filters" mode) — the distribution of profit/loss and trades
//! across the quantile buckets of the selected field. Split out of tuner.rs
//! (file size limit).
//!
//! `kpi_matrix_card` is UNIVERSAL (a free function): it draws purely out of
//! `VarStats`, so every tuning mode reuses it (Filter/Time/…).

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::summary::{fmt_signed, sign_color};
use super::super::{AnalyticsView, LoadState};
use super::fields::card;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::{FIELDS, VarStats};

impl AnalyticsView {
    /// KPI matrix of the "By filter" mode: scope is the SELECTION, columns v1/v2.
    pub(super) fn kpi_matrix(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // An empty label list → the "v{i}" fallback (as it historically was).
        kpi_matrix_card(&self.tuner.stats, self.scope_label(), &[], p, cx)
    }

    /// Histogram of the selected field: wins up, losses down, the count and the edges.
    pub(super) fn hist_card(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // The title carries the field and the scope (strategy name / count / all).
        let scope = self.scope_label();
        let title = format!(
            "{} — {} — {}",
            t!("analytics.tuner.hist_title"),
            FIELDS[self.tuner.sel_field].label,
            scope,
        );
        let body: AnyElement = match self.tuner.hist.view(|h| h.is_empty()) {
            Err(note) => super::super::note_el("an-tuner-hist-note", note, 8.0, p, cx),
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
        };
        card(
            title,
            t!("analytics.tuner.hist_sub").to_string(),
            body,
            p,
            cx,
        )
    }
}

/// The UNIVERSAL "Fact vs variants" KPI matrix: drawn PURELY out of `VarStats`,
/// knowing nothing about fields/hours/coins — hence one for every tuning mode.
/// `scope` is the subtitle (usually the strategy name); `var_labels` are the
/// v1..vN column labels (column 0 is always "Fact"); shorter than the variant
/// set falls back to "v{i}".
pub(super) fn kpi_matrix_card(
    stats: &LoadState<Vec<VarStats>>,
    scope: String,
    var_labels: &[String],
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    // No empty state by design: `stats.len() == variants.len()` always, so
    // the only non-data cases are loading / not-ready / read failure.
    let stats = match stats.view(|_| false) {
        Ok(s) => s.clone(),
        Err(note) => {
            return card(
                t!("analytics.tuner.kpi_title").to_string(),
                scope,
                super::super::note_el("an-tuner-kpi-note", note, 8.0, p, cx),
                p,
                cx,
            );
        }
    };
    // (label, value, higher=better; None — no comparison against the fact)
    type Row = (String, fn(&VarStats) -> f64, Option<bool>, bool);
    let rows: Vec<Row> = vec![
        (
            t!("analytics.kpi.trades").to_string(),
            |s| s.n as f64,
            None,
            true,
        ),
        (
            t!("analytics.kpi.profit").to_string(),
            |s| s.profit,
            Some(true),
            false,
        ),
        // Order mirrors the strategy table on the left of this screen; kept by hand, since
        // these rows carry comparison flags the table's descriptors have no notion of.
        (
            t!("analytics.kpi.avg_short").to_string(),
            |s| s.avg,
            Some(true),
            false,
        ),
        (
            t!("analytics.kpi.winrate").to_string(),
            |s| s.winrate(),
            Some(true),
            false,
        ),
        (
            t!("analytics.col.pf").to_string(),
            |s| s.pf,
            Some(true),
            false,
        ),
        (
            t!("analytics.tuner.avg_win").to_string(),
            |s| s.avg_win,
            Some(true),
            false,
        ),
        (
            t!("analytics.tuner.avg_loss").to_string(),
            |s| s.avg_loss,
            Some(false),
            false,
        ),
        (
            t!("analytics.kpi.maxdd").to_string(),
            |s| s.max_dd,
            Some(false),
            false,
        ),
    ];
    let col_w = 92.0;
    let mut head = h_flex()
        .w_full()
        .px(design::ui_px(cx, 8.0))
        .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_soft))
        .bg(moon(p.table_head))
        .child(
            div()
                .flex_1()
                .child(t!("analytics.tuner.metric").to_string()),
        );
    for i in 0..stats.len() {
        // Column 0 is always "Fact"; then the supplied labels, otherwise "v{i}".
        let name = if i == 0 {
            t!("analytics.tuner.fact").to_string()
        } else {
            var_labels
                .get(i - 1)
                .cloned()
                .unwrap_or_else(|| format!("v{i}"))
        };
        head = head.child(
            div()
                .w(design::font_w_px(cx, col_w))
                .flex_none()
                .text_right()
                .child(name),
        );
    }

    let mut body = v_flex().w_full().child(head);
    for (label, get, better, int) in rows {
        let fact = get(&stats[0]);
        let mut row = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.5))
            .child(div().flex_1().text_color(moon(p.text_soft)).child(label));
        for (i, s) in stats.iter().enumerate() {
            let v = get(s);
            let text = if int {
                format!("{}", v as i64)
            } else {
                fmt_signed(v)
            };
            let color = match better {
                // A variant is coloured against the fact; the fact itself by sign.
                Some(hb) if i > 0 => {
                    if (v > fact) == hb && v != fact {
                        p.green
                    } else if v != fact {
                        p.orange
                    } else {
                        p.text
                    }
                }
                Some(_) => sign_color(p, v),
                None => p.text,
            };
            row = row.child(
                div()
                    .w(design::font_w_px(cx, col_w))
                    .flex_none()
                    .text_right()
                    .text_color(moon(color))
                    .child(text),
            );
        }
        body = body.child(row);
    }
    card(
        t!("analytics.tuner.kpi_title").to_string(),
        scope,
        body.into_any_element(),
        p,
        cx,
    )
}

/// Short number format for the bucket edges (volumes up to billions).
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
