//! One train/holdout metric renderer and baseline comparison for tuner suggestions.

use crate::design::{self, moon};
use gpui::*;
use moon_core::db::metrics::Tally;
use moon_ui::{MoonPalette, h_flex};
use rust_i18n::t;

/// Color unseen profit by its comparison with Fact when available, then by its sign.
pub(super) fn holdout_color(profit: f64, fact: Option<f64>, p: MoonPalette) -> u32 {
    if fact.map_or(profit > 0.0, |fact| profit >= fact) {
        design::positive_color(p)
    } else {
        design::danger_color(p)
    }
}

/// Render the unchanged Fact cohort in a muted third line through the shared metric formatter.
pub(super) fn fact_line(tally: &Tally, p: MoonPalette, cx: &App) -> AnyElement {
    div()
        .debug_selector(|| "split-fact-holdout-line".into())
        .child(line(
            t!("analytics.tuner.split_fact_holdout").to_string(),
            tally,
            p.text_muted,
            p,
            cx,
        ))
        .into_any_element()
}

/// Render the identical MoonUI train-share dropdown for filter and time modes.
pub(super) fn train_dropdown_el(
    kind: super::shared::TunerKind,
    pct: usize,
    on_change: impl Fn(&mut App, usize) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    use moon_ui::{MoonButtonSize, MoonButtonVariant, MoonDropdown};
    let items = crate::panels::radio_items(
        super::filter::state::TRAIN_OPTIONS.map(|n| {
            (
                n,
                SharedString::from(format!("tun-tr-{n}")),
                SharedString::from(super::shell::train_label(n)),
            )
        }),
        pct,
        crate::panels::RadioMark::Highlight,
        on_change,
    );
    div()
        .flex_none()
        .debug_selector(|| "time-train-select".into())
        .child(
            MoonDropdown::new(SharedString::from(
                if kind == super::shared::TunerKind::Time {
                    "tun-cfg-tr-t"
                } else {
                    "tun-cfg-tr-f"
                },
            ))
            .label(super::shell::train_label(pct))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .fit_menu_width(96.0, 560.0)
            .items(items),
        )
        .into_any_element()
}

/// Render explicit caption typography and a mono value inheriting the row's metric color.
pub(super) fn line(caption: String, tally: &Tally, color: u32, p: MoonPalette, cx: &App) -> Div {
    let (pf, dd) = super::kpi::split_metrics(tally);
    let caption_width = [
        "analytics.tuner.split_train",
        "analytics.tuner.split_holdout",
        "analytics.tuner.split_fact_holdout",
    ]
    .map(|key| design::ui_caption_text_width(cx, &t!(key), 400.0))
    .into_iter()
    .fold(f32::from(design::font_w_px(cx, 74.0)), f32::max);
    h_flex()
        .w_full()
        .min_w_0()
        .text_size(design::t_caption(cx))
        .text_color(moon(color))
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .id(SharedString::from(format!("split-caption-{caption}")))
                .debug_selector(|| "split-caption".into())
                .w(px(caption_width.ceil() + 1.0))
                .flex_none()
                .truncate()
                .font_family(design::ui_font())
                .text_color(moon(p.text_muted))
                .tooltip(crate::panels::common::text_tooltip(caption.clone()))
                .child(caption),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .child(
                    t!(
                        "analytics.tuner.split_stats",
                        profit = super::super::summary::fmt_signed(tally.profit),
                        n = tally.n,
                        wr = format!("{:.1}", tally.winrate()),
                        pf = pf,
                        dd = dd
                    )
                    .to_string(),
                ),
        )
}

#[cfg(test)]
mod tests;
