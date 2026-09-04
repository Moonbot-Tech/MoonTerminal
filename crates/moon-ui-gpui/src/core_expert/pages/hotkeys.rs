//! Moonbot's "Hotkeys" page, control for control — including its six inner tabs.
//!
//! This page is READ-ONLY rather than dead, and the difference is worth stating. The terminal DOES
//! project the core's hotkeys (`ManualSettings::core_hotkeys`, `strat_buttons`, `order_sizes`), so
//! the bindings shown here are the core's real ones, decoded through the same
//! `moonbot_import::shortcut` the Settings window pulls them with. What it may not do is write them
//! back: `FieldMask::RENDERED_SECTIONS` excludes the whole manual block, deliberately, so an OK
//! from this window can never touch a manual-trading field.
//!
//! Moonbot's own mouse bindings (the "Orders Controls" tab) are the exception — those live in
//! `trading.multi_orders`, which the terminal does not project at all, so that tab shows em dashes.
//!
//! To CHANGE these, the Settings window's Hotkeys tab pulls them into the terminal's own set; this
//! page is the mirror of what the core holds.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::config::moonbot_import::shortcut;
use moon_core::feed::{CoreConfig, CoreHotkeyAction};

use crate::design;

use super::super::CoreExpertView;
use super::super::widgets::{caption, dropdown, flag, group, hint, rows, text_block};

/// Value shown where Moonbot prints something this terminal has not read.
const NO_VALUE: &str = "—";

/// Moonbot's built-in shortcut lines, in its own order.
const BUILT_IN_LINES: [&str; 6] = [
    "core_expert.hk_builtin_1",
    "core_expert.hk_builtin_2",
    "core_expert.hk_builtin_3",
    "core_expert.hk_builtin_4",
    "core_expert.hk_builtin_5",
    "core_expert.hk_builtin_6",
];

/// Moonbot's touch gestures, in its own order.
const TOUCH_LINES: [&str; 5] = [
    "core_expert.hk_touch_1",
    "core_expert.hk_touch_2",
    "core_expert.hk_touch_3",
    "core_expert.hk_touch_4",
    "core_expert.hk_touch_5",
];

/// Moonbot's inner tabs on this page, in its own order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum HotkeysSub {
    /// Mouse bindings for placing and moving orders.
    #[default]
    Orders,
    /// The six fixed order sizes and their keys.
    Sizes,
    /// The six fixed sell prices and their keys.
    Sells,
    /// The ten manual-strategy buttons and their keys.
    Manual,
    /// Moonbot's built-in, unbindable shortcuts.
    BuiltIn,
    /// Moonbot's touch gestures.
    Touch,
}

impl HotkeysSub {
    pub(crate) const ALL: [HotkeysSub; 6] = [
        Self::Orders,
        Self::Sizes,
        Self::Sells,
        Self::Manual,
        Self::BuiltIn,
        Self::Touch,
    ];

    /// Localized label, in Moonbot's own wording — the four English ones are English there too.
    pub(crate) fn title(self) -> String {
        match self {
            Self::Orders => "Orders Controls".to_string(),
            Self::Sizes => "Fixed Order Sizes".to_string(),
            Self::Sells => "Fixed Sell Prices".to_string(),
            Self::Manual => "Manual strategies".to_string(),
            Self::BuiltIn => t!("core_expert.hk_built_in").to_string(),
            Self::Touch => t!("core_expert.hk_touch").to_string(),
        }
    }

    /// The sub-tab at one position in the strip.
    pub(crate) fn at(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// One core binding as text, or an em dash when the slot is unbound or holds a key this build
/// cannot name.
fn key_text(raw: u16) -> String {
    shortcut::to_gpui_keystroke(shortcut::decode(raw)).unwrap_or_else(|| NO_VALUE.to_string())
}

/// A read-only binding cell: Moonbot draws a text box, and this draws what the core holds in it.
fn key_cell(label: String, value: String, p: MoonPalette, cx: &App) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap(design::ui_px(cx, 2.0))
        .child(hint(label, p, cx))
        .child(
            div()
                .w_full()
                .px(design::ui_px(cx, 6.0))
                .py(design::ui_px(cx, 3.0))
                .rounded(design::r_button(cx))
                .border_1()
                .border_color(rgb(p.border))
                .child(caption(value, false, p, cx)),
        )
}

/// A row of binding cells, so a page reads as Moonbot's grid rather than as a column.
fn key_row(cells: Vec<impl IntoElement>, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 8.0))
        .children(cells)
}

/// The named action bindings, in the order Moonbot lays its "Управление" grid out.
///
/// Five columns, and the actions Moonbot puts in them; an action the projection does not carry
/// draws its own em dash rather than being skipped, so the grid keeps Moonbot's shape.
const CONTROL_GRID: [[(&str, Option<CoreHotkeyAction>); 5]; 5] = [
    [
        (
            "core_expert.hk_cancel_buy",
            Some(CoreHotkeyAction::CancelBuy),
        ),
        (
            "core_expert.hk_panic_sell",
            Some(CoreHotkeyAction::PanicSell),
        ),
        (
            "core_expert.hk_join_sells",
            Some(CoreHotkeyAction::JoinSells),
        ),
        (
            "core_expert.hk_switch_charts",
            Some(CoreHotkeyAction::SwitchCharts),
        ),
        (
            "core_expert.hk_switch_figure",
            Some(CoreHotkeyAction::SwitchFigure),
        ),
    ],
    [
        (
            "core_expert.hk_reload_book",
            Some(CoreHotkeyAction::ReloadBook),
        ),
        ("core_expert.hk_new_long", Some(CoreHotkeyAction::NewLong)),
        ("core_expert.hk_new_short", Some(CoreHotkeyAction::NewShort)),
        (
            "core_expert.hk_split_order",
            Some(CoreHotkeyAction::SplitOrder),
        ),
        ("core_expert.hk_fit_sells", Some(CoreHotkeyAction::FitSells)),
    ],
    [
        (
            "core_expert.hk_shift_buy_up",
            Some(CoreHotkeyAction::ShiftBuyUp),
        ),
        (
            "core_expert.hk_shift_buy_down",
            Some(CoreHotkeyAction::ShiftBuyDown),
        ),
        (
            "core_expert.hk_shift_sell_up",
            Some(CoreHotkeyAction::ShiftSellUp),
        ),
        (
            "core_expert.hk_shift_sell_down",
            Some(CoreHotkeyAction::ShiftSellDown),
        ),
        (
            "core_expert.hk_panic_sell_one",
            Some(CoreHotkeyAction::PanicSellOne),
        ),
    ],
    [
        (
            "core_expert.hk_chart_shot",
            Some(CoreHotkeyAction::MakeShot),
        ),
        (
            "core_expert.hk_bot_shot",
            Some(CoreHotkeyAction::MakeShotBot),
        ),
        ("core_expert.hk_center_chart", None),
        (
            "core_expert.hk_reload_chart",
            Some(CoreHotkeyAction::ReloadChart),
        ),
        (
            "core_expert.hk_cancel_all_buys",
            Some(CoreHotkeyAction::CancelAllBuys),
        ),
    ],
    [
        (
            "core_expert.hk_scale_plus",
            Some(CoreHotkeyAction::ScalePlus),
        ),
        (
            "core_expert.hk_scale_minus",
            Some(CoreHotkeyAction::ScaleMinus),
        ),
        ("core_expert.hk_sell_plus", Some(CoreHotkeyAction::SellPlus)),
        (
            "core_expert.hk_sell_minus",
            Some(CoreHotkeyAction::SellMinus),
        ),
        (
            "core_expert.hk_open_coin_all",
            Some(CoreHotkeyAction::Broadcast),
        ),
    ],
];

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    sub: HotkeysSub,
    draft: &CoreConfig,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let m = &draft.manual;
    let named = |action: Option<CoreHotkeyAction>| {
        action
            .and_then(|a| {
                m.core_hotkeys
                    .named
                    .iter()
                    .find(|(kind, _)| *kind == a)
                    .map(|(_, raw)| key_text(*raw))
            })
            .unwrap_or_else(|| NO_VALUE.to_string())
    };

    // --- The inner page, by sub-tab ---------------------------------------------------------------
    let inner: AnyElement = match sub {
        HotkeysSub::Orders => {
            // Moonbot's mouse bindings live in `trading.multi_orders`, which this terminal does not
            // project: every selector here shows what it is for and nothing about its value.
            let mouse_row = |label: &str, ids: [&'static str; 3]| {
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(div().w(design::ui_px(cx, 110.0)).child(caption(
                        label.to_string(),
                        false,
                        p,
                        cx,
                    )))
                    .child(dropdown(ids[0], NO_VALUE.to_string(), false))
                    .child(dropdown(ids[1], NO_VALUE.to_string(), false))
                    .child(dropdown(ids[2], NO_VALUE.to_string(), false))
            };
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(design::ui_px(cx, 8.0))
                        .child(caption(
                            t!("core_expert.hk_place_long").to_string(),
                            false,
                            p,
                            cx,
                        ))
                        .child(dropdown("exp-hk-place-long", NO_VALUE.to_string(), false))
                        .child(caption(
                            t!("core_expert.hk_place_short").to_string(),
                            false,
                            p,
                            cx,
                        ))
                        .child(dropdown("exp-hk-place-short", NO_VALUE.to_string(), false)),
                )
                .child(flag(
                    "exp-hk-same-long-short",
                    t!("core_expert.hk_same_long_short").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                ))
                .child(mouse_row(
                    &t!("core_expert.hk_move_order"),
                    ["exp-hk-move-long", "exp-hk-move-short", "exp-hk-move-kind"],
                ))
                .child(mouse_row(
                    &t!("core_expert.hk_move_tp"),
                    ["exp-hk-tp-long", "exp-hk-tp-short", "exp-hk-tp-kind"],
                ))
                .child(hint(t!("core_expert.hk_additional").to_string(), p, cx))
                .child(mouse_row(
                    &t!("core_expert.hk_move_order"),
                    [
                        "exp-hk-move-long2",
                        "exp-hk-move-short2",
                        "exp-hk-move-kind2",
                    ],
                ))
                .child(mouse_row(
                    &t!("core_expert.hk_move_tp"),
                    ["exp-hk-tp-long2", "exp-hk-tp-short2", "exp-hk-tp-kind2"],
                ))
                .child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap(design::ui_px(cx, 8.0))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(design::ui_px(cx, 2.0))
                                .child(hint(t!("core_expert.hk_pending_long").to_string(), p, cx))
                                .child(dropdown(
                                    "exp-hk-pending-long",
                                    NO_VALUE.to_string(),
                                    false,
                                )),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(design::ui_px(cx, 2.0))
                                .child(hint(t!("core_expert.hk_pending_short").to_string(), p, cx))
                                .child(dropdown(
                                    "exp-hk-pending-short",
                                    NO_VALUE.to_string(),
                                    false,
                                )),
                        ),
                )
                .into_any_element()
        }
        HotkeysSub::Sizes => {
            // Both halves ARE projected: the amounts in `manual.order_sizes`, the keys in
            // `core_hotkeys.order_size`.
            let slot = |index: usize| {
                let amount = m.order_sizes.get(index).copied().unwrap_or_default();
                let key = m
                    .core_hotkeys
                    .order_size
                    .get(index)
                    .copied()
                    .map_or_else(|| NO_VALUE.to_string(), key_text);
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 2.0))
                    .child(caption(
                        format!("({}) {amount:.1}$", index + 1),
                        false,
                        p,
                        cx,
                    ))
                    .child(key_cell(t!("core_expert.hk_key").to_string(), key, p, cx))
            };
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .child(key_row((0..4).map(slot).collect(), cx))
                .child(key_row((4..6).map(slot).collect(), cx))
                .into_any_element()
        }
        HotkeysSub::Sells => {
            // The keys are projected; the PERCENTAGES are not — they ride the compact
            // `ClientSettings` channel (`s_price`), which this page does not read.
            let slot = |index: usize| {
                let key = m
                    .core_hotkeys
                    .sell_preset
                    .get(index)
                    .copied()
                    .map_or_else(|| NO_VALUE.to_string(), key_text);
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 2.0))
                    .child(caption(
                        t!(
                            "core_expert.hk_sell_slot",
                            n = (index + 1).to_string(),
                            v = NO_VALUE
                        )
                        .to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .child(key_cell(t!("core_expert.hk_key").to_string(), key, p, cx))
            };
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .child(key_row((0..4).map(slot).collect(), cx))
                .child(key_row((4..6).map(slot).collect(), cx))
                .into_any_element()
        }
        HotkeysSub::Manual => {
            // The buttons and their keys are projected (`manual.strat_buttons`); the switch is drawn
            // from the core's own value and left read-only, like the rest of the manual block.
            let slot = |index: usize| {
                let shown = m
                    .strat_buttons
                    .show_button
                    .get(index)
                    .copied()
                    .unwrap_or(false);
                let key = m
                    .strat_buttons
                    .hot_keys
                    .get(index)
                    .copied()
                    .map_or_else(|| NO_VALUE.to_string(), key_text);
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 2.0))
                    .child(flag(
                        "exp-hk-manual-slot",
                        t!("core_expert.hk_button", n = (index + 1).to_string()).to_string(),
                        shown,
                        false,
                        view,
                        |_, _| {},
                    ))
                    .child(key_cell(t!("core_expert.hk_key").to_string(), key, p, cx))
            };
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .child(key_row((0..5).map(slot).collect(), cx))
                .child(key_row((5..10).map(slot).collect(), cx))
                .into_any_element()
        }
        HotkeysSub::BuiltIn => v_flex()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .children(
                BUILT_IN_LINES
                    .iter()
                    .map(|key| text_block(t!(*key).to_string(), p.text_soft, false, cx)),
            )
            .into_any_element(),
        HotkeysSub::Touch => v_flex()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .child(text_block(
                t!("core_expert.hk_touch_intro").to_string(),
                p.text_soft,
                false,
                cx,
            ))
            .children(
                TOUCH_LINES
                    .iter()
                    .map(|key| text_block(t!(*key).to_string(), p.text_soft, false, cx)),
            )
            .into_any_element(),
    };

    // --- Moonbot repeats its "Управление" grid under every sub-tab, and so does this page ---------
    let controls =
        group(
            "exp-hk-controls",
            t!("core_expert.hk_controls_frame").to_string(),
        )
        .child(v_flex().w_full().gap(design::ui_px(cx, 6.0)).children(
            CONTROL_GRID.iter().map(|row| {
                key_row(
                    row.iter()
                        .map(|(key, action)| key_cell(t!(*key).to_string(), named(*action), p, cx))
                        .collect(),
                    cx,
                )
            }),
        ));

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 10.0))
        .child(rows(cx).child(inner))
        .child(controls)
        .into_any_element()
}
