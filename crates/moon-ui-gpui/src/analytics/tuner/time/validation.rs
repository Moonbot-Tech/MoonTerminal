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

/// Show validation only for an unedited completed suggestion; warnings wrap under metric lines.
pub(super) fn summary(
    result: Option<&TimeEvaluation>,
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
            .when_some(result.train.as_ref(), |el, train| {
                el.child(super::super::split::line(
                    t!("analytics.tuner.split_train").to_string(),
                    train,
                    p.text_soft,
                    p,
                    cx,
                ))
            })
            .when_some(result.holdout.as_ref(), |el, holdout| {
                el.child(super::super::split::line(
                    t!("analytics.tuner.split_holdout").to_string(),
                    holdout,
                    if holdout.profit > 0.0 {
                        design::positive_color(p)
                    } else {
                        design::danger_color(p)
                    },
                    p,
                    cx,
                ))
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
