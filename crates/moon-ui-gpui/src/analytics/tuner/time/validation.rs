//! Schedule validation states and their compact, wrapping presentation.

use crate::design::{self, moon};
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::tuner::TimeEvaluation;
use moon_ui::{MoonPalette, v_flex};
use rust_i18n::t;

/// Decide which warning accompanies a completed fit without any GPUI dependency.
pub(super) fn warning(result: &TimeEvaluation) -> Option<&'static str> {
    if result.train.is_none() {
        Some("analytics.tuner.time_small")
    } else if result.holdout.is_none() {
        Some("analytics.tuner.time_no_split")
    } else if result.below_fact() {
        Some("analytics.tuner.time_below_fact")
    } else {
        None
    }
}

/// Describe the fitted fields before their train/holdout metrics, without rescoring them.
fn result_line(result: &TimeEvaluation) -> Option<String> {
    let train = result.train.as_ref()?;
    let mut windows = Vec::new();
    if let Some(span) = result.suggestion.week_span {
        windows.push(format!(
            "WorkingWeekTime {}",
            moon_core::db::tuner::format_week_span(span)
        ));
    }
    if let Some(window) = result.suggestion.tod {
        windows.push(format!(
            "WorkingTime {}",
            moon_core::db::tuner::format_working_time(window)
        ));
    }
    let window = if windows.is_empty() {
        t!("analytics.tuner.time_all_hours").to_string()
    } else {
        windows.join(" · ")
    };
    Some(
        t!(
            "analytics.tuner.time_window",
            window = window,
            n = train.n + result.holdout.as_ref().map_or(0, |part| part.n)
        )
        .to_string(),
    )
}

/// Build the production holdout row so its assigned metric color can be checked independently.
pub(super) fn holdout_line(result: &TimeEvaluation, p: MoonPalette, cx: &App) -> Option<Div> {
    let holdout = result.holdout.as_ref()?;
    Some(
        super::super::split::line(
            t!("analytics.tuner.split_holdout").to_string(),
            holdout,
            super::super::split::holdout_color(
                holdout.profit,
                result.fact_holdout.as_ref().map(|fact| fact.profit),
                p,
            ),
            p,
            cx,
        )
        .debug_selector(|| "time-holdout-line".into()),
    )
}

/// Keep unedited validation visible across report appends, explicitly marking its snapshot stale.
pub(super) fn summary(
    result: Option<&TimeEvaluation>,
    stale: bool,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    let result = result?;
    Some(
        v_flex()
            .w_full()
            .min_w_0()
            .flex_none()
            .debug_selector(|| "time-validation".into())
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 6.0))
            .gap(design::ui_px(cx, 2.0))
            .border_t_1()
            .border_color(moon(p.border))
            .text_size(design::t_caption(cx))
            .children(result_line(result).map(|line| {
                div()
                    .debug_selector(|| "time-result-line".into())
                    .w_full()
                    .min_w_0()
                    .font_family(design::ui_font())
                    .text_color(moon(p.text_soft))
                    .child(line)
            }))
            .when_some(result.train.as_ref(), |el, train| {
                el.child(div().debug_selector(|| "time-train-line".into()).child(
                    super::super::split::line(
                        t!("analytics.tuner.split_train").to_string(),
                        train,
                        p.text_soft,
                        p,
                        cx,
                    ),
                ))
            })
            .children(holdout_line(result, p, cx))
            .when_some(result.fact_holdout.as_ref(), |el, fact| {
                el.child(super::super::split::fact_line(fact, p, cx))
            })
            .when(stale, |el| {
                el.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .debug_selector(|| "time-stale-warning".into())
                        .font_family(design::ui_font())
                        .text_color(moon(p.orange))
                        .child(t!("analytics.coins.stale_note_report").to_string()),
                )
            })
            .when_some(warning(result), |el, key| {
                el.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .debug_selector(|| "time-warning".into())
                        .font_family(design::ui_font())
                        .text_color(moon(p.orange))
                        .child(t!(key).to_string()),
                )
            })
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests;
