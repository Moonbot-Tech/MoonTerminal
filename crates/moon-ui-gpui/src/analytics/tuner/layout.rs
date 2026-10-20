//! Shared tuner page geometry and classified empty cards.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, MoonScrollableElement, h_flex, v_flex};
use rust_i18n::t;

use crate::design::{self, moon};

/// All four axes share this font-scaled preferred column width.
pub(super) const SIDE_W: f32 = 540.0;

/// Page padding is shared by the renderer and width-sensitive controls.
const PAGE_PAD: f32 = 10.0;

/// Stack the page in narrow docks, using the measured panel rather than the window width.
pub(super) fn is_narrow(width: f32, cx: &App) -> bool {
    width > 0.0 && width < f32::from(design::font_w_px(cx, SIDE_W + 240.0))
}

/// Resolve the width the column actually receives before deciding whether grid ranges fit.
pub(super) fn side_width(width: f32, cx: &App) -> Pixels {
    if is_narrow(width, cx) {
        px((width - f32::from(design::ui_px(cx, 2.0 * PAGE_PAD))).max(0.0))
    } else {
        design::font_w_px(cx, SIDE_W)
    }
}

/// Keep list details beside a bounded right column with scrolling inside Parameters.
pub(super) fn page(
    left: AnyElement,
    rail: AnyElement,
    side: Option<AnyElement>,
    narrow: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let left = div()
        .min_w_0()
        .when(narrow, |el| {
            el.w_full().h(design::ui_px(cx, 520.0)).flex_none()
        })
        .when(!narrow, |el| el.flex_1().h_full().min_h_0())
        .child(left);
    let side = side.map(|side| {
        let scroll = window
            .use_keyed_state("tuner-side-overflow", cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        div()
            .id("tuner-side-host")
            .debug_selector(|| "tuner-side".into())
            .relative()
            .min_w_0()
            .flex_none()
            // Stacked docks scroll the page to this bounded workspace. Leave enough height
            // for the expanded KPI's wrapped rows as well as the Parameters viewport.
            .when(narrow, |el| el.w_full().h(design::ui_px(cx, 1000.0)))
            .when(!narrow, |el| {
                el.w(design::font_w_px(cx, SIDE_W)).h_full().min_h_0()
            })
            .child(
                div()
                    .id("tuner-side-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .child(side),
            )
            .vertical_scrollbar(&scroll)
    });
    if narrow {
        div()
            .id("tuner-page-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .p(design::ui_px(cx, PAGE_PAD))
                    .gap(design::ui_px(cx, 8.0))
                    .child(left)
                    .child(div().h(design::ui_px(cx, 24.0)).child(rail))
                    .children(side),
            )
            .into_any_element()
    } else {
        h_flex()
            .size_full()
            .min_w_0()
            .p(design::ui_px(cx, PAGE_PAD))
            .gap(design::ui_px(cx, 8.0))
            .child(left)
            .child(rail)
            .children(side)
            .into_any_element()
    }
}

/// Fill the column, reserving a usable workspace that short hosts can reach by outer scrolling.
pub(super) fn side(kpi: AnyElement, params: AnyElement, cx: &App) -> AnyElement {
    v_flex()
        .debug_selector(|| "tuner-side-content".into())
        .size_full()
        .min_h(design::ui_px(cx, 800.0))
        .gap(design::ui_px(cx, 8.0))
        .child(kpi)
        .child(params)
        .into_any_element()
}

/// Keep the run/result bands fixed while the field content scrolls within the remaining height.
pub(super) fn parameters(
    header: AnyElement,
    body: AnyElement,
    p: MoonPalette,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let scroll = window
        .use_keyed_state("tuner-parameters-scroll", cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone();
    parameters_with_scroll(header, body, &scroll, p, cx)
}

/// Share the production card and scrollbar with geometry tests that drive its scroll handle.
fn parameters_with_scroll(
    header: AnyElement,
    body: AnyElement,
    scroll: &ScrollHandle,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    v_flex()
        .debug_selector(|| "tuner-parameters".into())
        .w_full()
        .min_w_0()
        .flex_1()
        .min_h_0()
        .rounded(design::r_container(cx))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .overflow_hidden()
        .child(div().w_full().flex_none().child(header))
        .child(
            div()
                .debug_selector(|| "tuner-parameters-viewport".into())
                .relative()
                .w_full()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("tuner-parameters-fields")
                        .debug_selector(|| "tuner-parameters-scroll".into())
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(scroll)
                        .child(div().w_full().child(body)),
                )
                .vertical_scrollbar(scroll),
        )
        .into_any_element()
}

/// Reserve both right-side cards while waiting for a selection or the initial read.
pub(super) fn empty_side(loading: bool, caret: AnyElement, p: MoonPalette, cx: &App) -> AnyElement {
    let message = |key: &str| {
        div()
            .w_full()
            .min_w_0()
            .p(design::ui_px(cx, 28.0))
            .text_center()
            .font_family(design::ui_font())
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_muted))
            .child(t!(key).to_string())
            .into_any_element()
    };
    let kpi = super::shared::card(
        t!("analytics.tuner.kpi_title").to_string(),
        String::new(),
        message(if loading {
            "common.loading"
        } else {
            "analytics.tuner.pick_strategy"
        }),
        Some(caret),
        p,
        cx,
    );
    let params = super::shared::card(
        t!("analytics.tuner.fields_title").to_string(),
        String::new(),
        message(if loading {
            "common.loading"
        } else {
            "analytics.tuner.no_strategy"
        }),
        None,
        p,
        cx,
    );
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(kpi)
        .child(params)
        .into_any_element()
}

impl super::super::AnalyticsView {
    /// Keep the same KPI disclosure during initial loading and absent selection.
    pub(super) fn tuner_empty_side(
        &self,
        loading: bool,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let caret = super::shared::collapse_caret(
            "an-tuner-kpi-collapse",
            self.kpi_collapsed,
            t!("analytics.tuner.kpi_collapse").to_string(),
            t!("analytics.tuner.kpi_expand").to_string(),
            p,
            cx.listener(|this, _, _, cx| this.toggle_kpi_collapsed(cx)),
        );
        empty_side(loading, caret, p, cx)
    }
}

#[cfg(test)]
mod tests;
