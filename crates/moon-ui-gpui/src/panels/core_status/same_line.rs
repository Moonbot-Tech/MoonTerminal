//! The line that states values every reporting core shares.
//!
//! Those columns leave the table. The line wraps onto as many rows as it needs and never clips
//! a value. An empty fact list draws nothing.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex};
use rust_i18n::t;

use super::uniform::UniformFact;
use crate::design;

/// Draw "same for all" and one label/value pair per uniform column.
///
/// Args:
///     facts: Uniform columns in display order. A `tz_off` fact may carry the clock-offset hover.
///     cx: Application context used for the density scale and the palette. Not the view.
///
/// Returns:
///     The wrapping line, or `None` when `facts` is empty.
pub(super) fn same_line(facts: &[UniformFact], cx: &App) -> Option<impl IntoElement + use<>> {
    if facts.is_empty() {
        return None;
    }
    let p = MoonPalette::active(cx);
    let caption = design::t_caption(cx);
    Some(
        h_flex()
            .id("core-status-same")
            .debug_selector(|| "cs-same".into())
            .w_full()
            .min_w(px(0.0))
            .flex_wrap()
            .items_center()
            .gap_x(design::ui_px(cx, 14.0))
            .gap_y(design::ui_px(cx, 4.0))
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 6.0))
            .font_family(design::ui_font())
            .child(
                div()
                    .debug_selector(|| "cs-same-title".into())
                    .flex_none()
                    .text_size(caption)
                    .text_color(rgb(p.text_muted))
                    .child(t!("core_status.same.title").to_string()),
            )
            .children(facts.iter().map(|fact| same_fact(fact, p, caption))),
    )
}

/// One fact: a UI-face label and a mono value. The clock-offset fact keeps its hover.
///
/// Args:
///     fact: The shared column.
///     p: Active palette. The caption is muted and the value is soft.
///     caption: Caption text size, already scaled.
///
/// Returns:
///     The pair. It does not shrink, so a wrap moves the whole pair.
fn same_fact(fact: &UniformFact, p: MoonPalette, caption: Pixels) -> impl IntoElement {
    let key = fact.key;
    h_flex()
        .id(SharedString::from(format!("core-status-same-{}", fact.key)))
        .debug_selector(move || format!("cs-same-{key}"))
        .flex_none()
        .items_center()
        .gap_1()
        .child(
            div()
                .text_size(caption)
                .text_color(rgb(p.text_muted))
                .child(same_label(fact.key)),
        )
        .child(
            div()
                .flex_none()
                .font_family(design::mono())
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(p.text_soft))
                .child(fact.text.clone()),
        )
        .when_some(fact.tip.clone(), |row, tip| {
            row.tooltip(crate::panels::common::text_tooltip(tip))
        })
}

/// Translated name of one uniform column.
///
/// Args:
///     key: Flat column id from [`super::uniform::UniformFact`].
///
/// Returns:
///     The label. An unknown id is returned unchanged.
fn same_label(key: &str) -> String {
    match key {
        "tz_off" => t!("core_status.same.tz_off").to_string(),
        "cpus" => t!("core_status.same.cpus").to_string(),
        "api_key" => t!("core_status.same.api_key").to_string(),
        "api_quota" => t!("core_status.same.api_quota").to_string(),
        "version" => t!("core_status.same.version").to_string(),
        other => other.to_string(),
    }
}
