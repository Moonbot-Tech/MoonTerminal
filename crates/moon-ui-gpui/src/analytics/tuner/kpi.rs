//! The "Fact vs variants" KPI matrix — drawn by EVERY tuning axis.
//!
//! It knows nothing about fields, hours or coins: it renders purely out of `VarStats`, which is
//! exactly why it lives at the tuner root rather than inside whichever axis happened to need it
//! first. An axis that wants a differently-shaped matrix should widen this one, not fork it.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::summary::{fmt_signed, sign_color};
use super::super::{AnalyticsView, LoadState};
use super::shared::card;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::VarStats;

/// A KPI column heading: its name, plus an optional second line saying what the variant is
/// made of. Two fields rather than one long string — see [`kpi_matrix_card`].
#[derive(Clone)]
pub(super) struct VarLabel {
    pub(super) title: String,
    pub(super) sub: Option<String>,
}

impl VarLabel {
    pub(super) fn new(title: String) -> Self {
        Self { title, sub: None }
    }

    pub(super) fn with_sub(title: String, sub: String) -> Self {
        Self {
            title,
            sub: Some(sub),
        }
    }
}

/// The UNIVERSAL "Fact vs variants" KPI matrix: drawn PURELY out of `VarStats`,
/// knowing nothing about fields/hours/coins — hence one for every tuning mode.
/// `scope` is the subtitle (usually the strategy name); `var_labels` are the
/// v1..vN column headings (column 0 is always "Fact"); shorter than the variant
/// set falls back to "v{i}".
///
/// A heading carries an optional SECOND line for what the variant is made of ("BL 2 / WL 129").
/// It is a line of its own rather than a longer title on purpose: that text grows with the
/// data, and inside a fixed-width column a one-liner wraps wherever it happens to run out —
/// which put "129)" alone on the next row and shoved the whole header down.
pub(super) fn kpi_matrix_card(
    stats: &LoadState<Vec<VarStats>>,
    scope: String,
    var_labels: &[VarLabel],
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
    // Tall enough for a heading WITH its second line, always — so a variant that gains or
    // loses that line does not move the rows underneath it.
    let mut head = h_flex()
        .w_full()
        .px(design::ui_px(cx, 8.0))
        .h(design::fit_h_px(cx, 34.0, 12.0, 5.0))
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
        let label = if i == 0 {
            VarLabel::new(t!("analytics.tuner.fact").to_string())
        } else {
            var_labels
                .get(i - 1)
                .cloned()
                .unwrap_or_else(|| VarLabel::new(format!("v{i}")))
        };
        head = head.child(
            v_flex()
                .w(design::font_w_px(cx, col_w))
                .flex_none()
                .items_end()
                .child(div().child(label.title))
                .children(label.sub.map(|s| {
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .truncate()
                        .child(s)
                })),
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
