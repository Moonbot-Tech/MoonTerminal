//! One train/holdout metric renderer for filter and schedule suggestions.

use crate::design::{self, moon};
use gpui::*;
use moon_core::db::metrics::Tally;
use moon_ui::{MoonPalette, h_flex};
use rust_i18n::t;

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

/// Render a muted caption and a mono metric value that yields at narrow widths.
pub(super) fn line(
    caption: String,
    tally: &Tally,
    color: u32,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    h_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .w(design::font_w_px(cx, 74.0))
                .flex_none()
                .truncate()
                .font_family(design::ui_font())
                .text_color(moon(p.text_muted))
                .child(caption),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .text_color(moon(color))
                .child(
                    t!(
                        "analytics.tuner.split_stats",
                        profit = super::super::summary::fmt_signed(tally.profit),
                        n = tally.n,
                        wr = format!("{:.1}", tally.winrate()),
                        pf = super::super::summary::fmt_signed_plain(tally.profit_factor()),
                        dd = super::super::summary::fmt_signed_plain(tally.max_dd)
                    )
                    .to_string(),
                ),
        )
        .into_any_element()
}
