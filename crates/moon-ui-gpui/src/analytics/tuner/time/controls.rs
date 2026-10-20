//! Production configuration row for schedule fitting, also rendered by the layout probe.

use crate::design::{self, moon};
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonInput, MoonInputState, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

/// Keep the training control content-sized and let the configuration row wrap in a narrow dock.
pub(in crate::analytics::tuner) fn config_row(
    min: &Entity<MoonInputState>,
    train: AnyElement,
    busy: bool,
    option: AnyElement,
    on_run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let label = |text| div().flex_none().text_color(moon(p.text_muted)).child(text);
    let settings = h_flex()
        .w_full()
        .flex_none()
        .flex_wrap()
        .items_center()
        .debug_selector(|| "time-config".into())
        .px(design::ui_px(cx, 12.0))
        .pb(design::ui_px(cx, 6.0))
        .gap(design::ui_px(cx, 6.0))
        .font_family(design::ui_font())
        .text_size(design::t_body(cx))
        .child(label(t!("analytics.tuner.min_trades").to_string()))
        .child(
            div()
                .w(design::font_w_px(cx, 52.0))
                .flex_none()
                .font_family(design::mono())
                .child(
                    MoonInput::new("tun-cfg-mn-t")
                        .state(min)
                        .size(design::INPUT_SIZE),
                ),
        )
        .child(label(t!("analytics.tuner.train_share").to_string()))
        .child(train)
        .child(div().flex_1())
        .child(option);
    v_flex()
        .w_full()
        .flex_none()
        .child(settings)
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .justify_end()
                .debug_selector(|| "tuner-run-t".into())
                .px(design::ui_px(cx, 12.0))
                .pb(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .flex_none()
                        .debug_selector(|| "time-run".into())
                        .child(
                            MoonButton::new("tun-suggest-run-t")
                                .variant(MoonButtonVariant::Blue)
                                .label(if busy {
                                    "…".to_string()
                                } else {
                                    t!("analytics.tuner.suggest_run").to_string()
                                })
                                .on_click(on_run)
                                .render(),
                        ),
                ),
        )
        .into_any_element()
}
