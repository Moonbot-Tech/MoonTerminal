//! Aligned bound-grid headings and responsive variant sections shared by filter and time.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex};
use rust_i18n::t;

use crate::design::{self, moon};

/// Dense column gaps yield before body text or the pre-unification input widths do.
pub(super) const GAP: f32 = 1.0;

/// Measure the technical schedule labels once for both the grid and its slider captions.
pub(super) fn time_name_width(cx: &App) -> Pixels {
    let measured = [
        "analytics.time.field_week",
        "analytics.time.field_day",
        "analytics.time.field_hour",
    ]
    .iter()
    .map(|key| design::mono_body_text_width(cx, &t!(*key), 400.0))
    .fold(0.0_f32, f32::max);
    px(f32::from(design::font_w_px(cx, 100.0)).max(measured.ceil() + 1.0))
}

/// Geometry shared by a header and every field row in its section.
#[derive(Clone, Copy)]
pub(super) struct GridLayout {
    pub(super) compact: bool,
    pub(super) name: Pixels,
    pub(super) input: f32,
}

impl GridLayout {
    /// Keep full-size inputs; narrow docks stack the variants and move current values to tooltips.
    pub(super) fn new(width: Pixels, time: bool, cx: &App) -> Self {
        let name = if time {
            time_name_width(cx)
        } else {
            design::font_w_px(cx, 74.0)
        };
        let tokens = moon_ui::MoonTheme::active_tokens(cx);
        let checkbox = px(tokens.ui(16.0).max(
            tokens
                .line_height(20.0)
                .min(tokens.ui(16.0) + tokens.scale.font_delta.max(0.0)),
        ));
        // Four inputs, four 12px glyph controls, ten gaps, padding and the card borders.
        let minimum = name
            + design::font_w_px(cx, 60.0) * 4.0
            + checkbox
            + design::ui_px(cx, 48.0 + 10.0 * GAP + 16.0 + 2.0)
            + px(design::ui_body_text_width(cx, &t!("analytics.time.cur"), 400.0).ceil());
        Self {
            compact: width < minimum,
            name,
            input: 60.0,
        }
    }

    /// Each variant retains its own inputs and column actions when the sections stack.
    pub(super) fn sections(self) -> Vec<Vec<usize>> {
        if self.compact {
            vec![vec![0], vec![1]]
        } else {
            vec![vec![0, 1]]
        }
    }

    /// The field-name slot yields only in the single-variant narrow layout.
    pub(super) fn name_cell(self) -> Div {
        div()
            .min_w_0()
            .when(self.compact, |el| el.flex_1())
            .when(!self.compact, |el| el.w(self.name).flex_none())
    }

    /// A full-width row with one invariant gap contract for captions and inputs.
    pub(super) fn row(self, cx: &App) -> Div {
        h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 8.0))
            .items_center()
            .gap(design::ui_px(cx, GAP))
            .text_size(design::t_body(cx))
    }
}

/// Column actions stay inside the two bound slots, exactly over their matching input row.
pub(super) struct VariantHead {
    pub(super) index: usize,
    pub(super) copy: AnyElement,
    pub(super) clear: AnyElement,
}

/// Render one non-wrapping header, including copy arrows in the same row as the input columns.
pub(super) fn header(
    layout: GridLayout,
    selector: String,
    check: AnyElement,
    name: AnyElement,
    variants: Vec<VariantHead>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut row = layout
        .row(cx)
        .debug_selector(move || selector)
        .min_h(design::fit_h_px(cx, 26.0, 14.0, 5.0))
        .whitespace_nowrap()
        .font_family(design::ui_font())
        .bg(moon(p.table_head))
        .text_color(moon(p.text_soft))
        .child(check)
        .child(name)
        .when(!layout.compact, |el| {
            el.child(
                div()
                    .debug_selector(|| "bound-head-current".into())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(t!("analytics.time.cur").to_string()),
            )
        });
    for variant in variants {
        let cell = |text: String, side: &'static str| {
            div()
                .debug_selector(move || format!("bound-head-{side}-{}", variant.index))
                .w(design::font_w_px(cx, layout.input))
                .flex_none()
                .whitespace_nowrap()
                .text_center()
                .child(text)
        };
        row = row
            .child(cell(
                format!("v{} {}", variant.index + 1, t!("analytics.tuner.from")),
                "from",
            ))
            .child(variant.copy)
            .child(cell(t!("analytics.tuner.to").to_string(), "to"))
            .child(variant.clear);
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests;
