//! The coin table's filter bar as a FREE render function over a plain model plus handler
//! closures, so a layout test can draw it in a probe view (see `fields`).
//!
//! Two rows: search, "Hide coins with fewer trades than [N]", the "no trades" switch and the
//! "hidden X of Y" caption; under them the view chips.

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonInput, MoonInputState, MoonPalette, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::rows::CoinCounts;
use super::state::{CoinFilter, CoinsState};
use crate::design;
use crate::design::moon;

/// One view chip.
pub(in crate::analytics::tuner) struct FilterChip {
    pub(in crate::analytics::tuner) id: &'static str,
    pub(in crate::analytics::tuner) filter: CoinFilter,
    pub(in crate::analytics::tuner) label: String,
    pub(in crate::analytics::tuner) tip: String,
    pub(in crate::analytics::tuner) on: bool,
    pub(in crate::analytics::tuner) enabled: bool,
}

/// Everything the bar draws besides its two input entities.
pub(in crate::analytics::tuner) struct FilterBarModel {
    pub(in crate::analytics::tuner) hide_zero: bool,
    /// "hidden X of Y · listed coins are always shown".
    pub(in crate::analytics::tuner) caption: String,
    pub(in crate::analytics::tuner) chips: Vec<FilterChip>,
}

/// The bar's handlers.
pub(in crate::analytics::tuner) struct FilterBarActs {
    pub(in crate::analytics::tuner) on_filter: Rc<dyn Fn(CoinFilter, &mut Window, &mut App)>,
    pub(in crate::analytics::tuner) on_hide_zero: Rc<dyn Fn(bool, &mut Window, &mut App)>,
}

impl FilterBarModel {
    /// Build the bar's model from the coin state and the row counts.
    ///
    /// Args:
    ///     s: Coin state (current view, lists, switch).
    ///     counts: Counts of the searched coin set.
    ///
    /// Returns:
    ///     The model; a list chip reads "shown/total" when the list names coins the searched
    ///     set does not cover.
    pub(in crate::analytics::tuner) fn new(s: &CoinsState, counts: CoinCounts) -> Self {
        let listed = |shown: usize, total: usize| {
            if shown == total {
                shown.to_string()
            } else {
                format!("{shown}/{total}")
            }
        };
        let chip = |id, filter, label: String, tip: String| FilterChip {
            id,
            filter,
            label,
            tip,
            on: s.filter == filter,
            enabled: s.filter_available(filter),
        };
        let chips = vec![
            chip(
                "an-coin-f-all",
                CoinFilter::All,
                format!("{} {}", t!("report.filter.all"), counts.all),
                t!("analytics.coins.tip_chip_all").to_string(),
            ),
            chip(
                "an-coin-f-bl",
                CoinFilter::Black,
                format!(
                    "{} {}",
                    t!("analytics.coins.chip_in_bl"),
                    listed(counts.black, s.work.black.len())
                ),
                t!("analytics.coins.tip_chip_bl").to_string(),
            ),
            chip(
                "an-coin-f-wl",
                CoinFilter::White,
                format!(
                    "{} {}",
                    t!("analytics.coins.chip_in_wl"),
                    listed(counts.white, s.work.white.len())
                ),
                t!("analytics.coins.tip_chip_wl").to_string(),
            ),
            chip(
                "an-coin-f-sug",
                CoinFilter::Suggested,
                format!(
                    "{} {}",
                    t!("analytics.coins.chip_suggested"),
                    counts.suggested
                ),
                t!("analytics.coins.tip_chip_suggested").to_string(),
            ),
            chip(
                "an-coin-f-ch",
                CoinFilter::Changed,
                format!("{} {}", t!("analytics.coins.changed"), counts.changed),
                t!("analytics.coins.tip_chip_changed").to_string(),
            ),
        ];
        Self {
            hide_zero: s.hide_zero,
            caption: t!(
                "analytics.coins.hidden_caption",
                hidden = counts.hidden,
                total = counts.all
            )
            .to_string(),
            chips,
        }
    }
}

/// The bar.
///
/// Args:
///     m: The bar's model.
///     search: The coin search input.
///     min_trades: The "fewer trades than" input.
///     acts: View and switch handlers.
///     p: Theme palette.
///     cx: App, for scale.
///
/// Returns:
///     The rendered two-row bar.
pub(in crate::analytics::tuner) fn filter_bar_el(
    m: &FilterBarModel,
    search: &Entity<MoonInputState>,
    min_trades: &Entity<MoonInputState>,
    acts: &FilterBarActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let on_hide_zero = acts.on_hide_zero.clone();
    let controls = h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .child(
            div()
                .id("an-coin-search-ctl")
                .flex_1()
                .min_w(design::font_w_px(cx, 120.0))
                .tooltip(crate::panels::common::text_tooltip(
                    t!("analytics.coins.tip_search").to_string(),
                ))
                .child(
                    MoonInput::new("an-coin-search")
                        .state(search)
                        .size(design::INPUT_SIZE),
                ),
        )
        .child(
            h_flex()
                .id("an-coin-min-trades-ctl")
                .flex_none()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .tooltip(crate::panels::common::text_tooltip(
                    t!("analytics.coins.tip_min_trades").to_string(),
                ))
                .child(
                    div()
                        .debug_selector(|| "an-coin-hide-lbl".into())
                        .child(t!("analytics.coins.hide_lbl").to_string()),
                )
                .child(
                    div().w(design::font_w_px(cx, 46.0)).flex_none().child(
                        MoonInput::new("an-coin-min-trades")
                            .state(min_trades)
                            .size(design::INPUT_SIZE),
                    ),
                ),
        )
        .child(
            div()
                .id("an-coin-hide-zero-ctl")
                .flex_none()
                .tooltip(crate::panels::common::text_tooltip(
                    t!("analytics.coins.tip_no_trades").to_string(),
                ))
                .child(
                    MoonCheckbox::new("an-coin-hide-zero")
                        .label(t!("analytics.coins.no_trades").to_string())
                        .checked(m.hide_zero)
                        .on_change(move |on: &bool, window, app| on_hide_zero(*on, window, app)),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_color(moon(p.text_muted))
                .child(m.caption.clone()),
        );
    let chips = h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .font_family(design::ui_font())
        .children(m.chips.iter().map(|c| {
            let f = c.filter;
            let act = acts.on_filter.clone();
            let button = MoonButton::new(c.id)
                .variant(if c.on {
                    MoonButtonVariant::Amber
                } else {
                    MoonButtonVariant::Soft
                })
                .selected(c.on)
                .disabled(!c.enabled)
                .label(c.label.clone())
                .tooltip(c.tip.clone())
                .on_click(move |_, window, app| act(f, window, app))
                .render();
            super::layout::tagged(c.id, button)
        }));
    v_flex()
        .id("an-coin-filter-bar")
        .debug_selector(|| "an-coin-filter-bar".into())
        .w_full()
        .flex_none()
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 6.0))
        .gap(design::ui_px(cx, 6.0))
        .bg(moon(p.table_head))
        .child(controls)
        .child(chips)
        .into_any_element()
}
