//! Moonbot's "Hotkeys" page, control for control — including its six inner tabs.
//!
//! One of its six inner tabs is live and five are READ-ONLY, and the difference is worth stating.
//!
//! "Orders Controls" is live: its sixteen selectors and one checkbox are
//! `moon_core::feed::GestureSettings`, which this window both projects and writes — the mouse
//! gestures that place, move and reprice an order, the "move kind" column saying which orders each
//! gesture addresses, and the switch that makes the short columns follow the long ones.
//!
//! Those gestures are the CORE's, not this terminal's. The terminal fires its own from
//! `config::HotkeysConfig`, which the Settings window edits and which this page never touches: a
//! change here moves what Moonbot does on a click, not what MoonTerminal does.
//!
//! The other five are read-only rather than dead. The terminal DOES project the core's keyboard
//! bindings (`ManualSettings::core_hotkeys`, `strat_buttons`, `order_sizes`), so what they show are
//! the core's real ones, decoded through the same `moonbot_import::shortcut` the Settings window
//! pulls them with. What it may not do is write them back: `ExpertTab::add_sections` excludes the
//! whole manual block, deliberately, so an OK from this window can never touch a manual-trading
//! field. To CHANGE those, the Settings window's Hotkeys tab pulls them into the terminal's own set.
//!
//! Two places still print an em dash where a value would be. The six "Fixed Sell Prices"
//! percentages ride the compact `ClientSettings` channel (`s_price`), which this page does not
//! read; and Moonbot's "Center Chart" has no action ordinal in the projected layout, so its cell in
//! the "Управление" grid is blank — that grid is repeated under all six sub-tabs, the live one
//! included.

use std::sync::LazyLock;

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::config::moonbot_import::shortcut;
use moon_core::config::{MouseGestureBinding, MoveKind};
use moon_core::feed::{CoreConfig, CoreHotkeyAction, MoveRow};

use crate::design;

use super::super::CoreExpertView;
use super::super::widgets::{caption, choice_live, flag, group, hint, rows, text_block};

/// Value shown where Moonbot prints something this terminal has not read.
const NO_VALUE: &str = "—";

/// One id per manual-strategy slot.
///
/// Ten rows drawn from one loop, and a GPUI `ElementId` addresses interaction state: sharing a
/// single id across them makes ten checkboxes into one control. Harmless while all ten are disabled
/// and stage nothing, which is why it went unseen — and a real collision the day the manual block
/// becomes writable.
const MANUAL_SLOT_IDS: [&str; 10] = [
    "exp-hk-manual-slot-1",
    "exp-hk-manual-slot-2",
    "exp-hk-manual-slot-3",
    "exp-hk-manual-slot-4",
    "exp-hk-manual-slot-5",
    "exp-hk-manual-slot-6",
    "exp-hk-manual-slot-7",
    "exp-hk-manual-slot-8",
    "exp-hk-manual-slot-9",
    "exp-hk-manual-slot-10",
];

/// Moonbot's mouse-gesture list, as `(wire ordinal, menu key, label)` in the order Delphi declares
/// it.
///
/// The ordinal IS the index, which `moon_core::config::hotkeys`'s own tests pin: moonproto's
/// defaults annotate `buy_set_click: 1` as `Dbl_Click` and `sell_move_click: 2` as `CTRL_Click`,
/// which is this list at 1 and 2.
///
/// Built once per PROCESS, not per render: every part of it is a constant. The labels come from
/// `MouseGestureBinding::menu_label`, which the terminal's own Hotkeys tab draws from too, so the
/// two surfaces cannot come to name one gesture differently — a trader reads this window beside
/// Moonbot's dialog, where `Ctrl+Left` alone does not match `CTRL_Click` on sight.
static GESTURE_OPTIONS: LazyLock<Vec<(u8, SharedString, SharedString)>> = LazyLock::new(|| {
    MouseGestureBinding::ALL
        .iter()
        .enumerate()
        .map(|(ordinal, gesture)| {
            (
                ordinal as u8,
                SharedString::from(gesture.config_value()),
                SharedString::from(gesture.menu_label()),
            )
        })
        .collect()
});

/// Moonbot's "move kind" list, the same way — against moonproto's `ReplaceMultiKind`, whose
/// constants run None=0 through LastMoved=7 exactly as `MoveKind::ALL` is ordered.
///
/// Per page rather than per process, unlike the gestures: these labels are localized, and a static
/// would hold the language the process started in. `MoveKind::locale_key` is the one place the key
/// is spelled, shared with the terminal's own Hotkeys tab.
fn move_kind_options() -> Vec<(u8, SharedString, SharedString)> {
    MoveKind::ALL
        .iter()
        .enumerate()
        .map(|(ordinal, kind)| {
            let key = kind.locale_key();
            (
                ordinal as u8,
                SharedString::from(kind.id()),
                SharedString::from(t!(&key).to_string()),
            )
        })
        .collect()
}

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
            let g = &draft.gestures;
            // With "one set for Long and Short" ticked the short MOVE columns follow the long ones,
            // so the dialog stops offering them — Moonbot's own rule, and what the terminal's
            // Hotkeys tab does with its own copy of these gestures. It gates the four move rows
            // only: placing a short and placing a pending short are their own gestures, which the
            // flag does not name.
            //
            // The two sources for that reading DISAGREE, and the disagreement is recorded rather
            // than resolved silently. Moonbot's caption says Long and Short, and
            // `moon_core::config::hotkeys` implements it that way in `move_gestures`; moonproto's
            // one-line gloss for the field instead says "the same hotkeys for both PRIMARY and
            // SECONDARY move". The caption and the shipped port win: a gloss is not a dialog.
            let shorts = !g.same_hotkeys_for_move;
            let gestures = GESTURE_OPTIONS.as_slice();
            let kinds = move_kind_options();
            // One row of Moonbot's grid: the long gesture, the short gesture, and the kind saying
            // which orders the pair addresses.
            //
            // Both halves of a Move row go through `GestureSettings`: the READ resolves the short
            // column to what actually fires (with the flag on, the long value), and the WRITE
            // carries the mirror that flag demands. Neither rule is spelled out here — they belong
            // to the projection, which is also where `moon_core::config::hotkeys` keeps the same
            // pair for the terminal's own copy of these gestures.
            let move_row = |label: String,
                            ids: [&'static str; 3],
                            row: MoveRow,
                            kind: u8,
                            kind_set: fn(&mut CoreConfig, u8)| {
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .w(design::ui_px(cx, 110.0))
                            .child(caption(label, true, p, cx)),
                    )
                    .child(choice_live(
                        ids[0],
                        gestures,
                        g.move_gesture(row, false),
                        true,
                        view,
                        move |d, v| d.gestures.set_move_gesture(row, false, v),
                    ))
                    .child(choice_live(
                        ids[1],
                        gestures,
                        g.move_gesture(row, true),
                        shorts,
                        view,
                        move |d, v| d.gestures.set_move_gesture(row, true, v),
                    ))
                    .child(choice_live(ids[2], &kinds, kind, true, view, kind_set))
            };
            // One pending row: Moonbot stacks these two under their own labels rather than in the
            // grid, and the wire keeps them in different records — the long one is
            // `trading.pending_order_set_click`, the short one is inside `multi_orders`.
            let pending =
                |label: String, id: &'static str, value: u8, set: fn(&mut CoreConfig, u8)| {
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(design::ui_px(cx, 2.0))
                        .child(hint(label, p, cx))
                        .child(choice_live(id, gestures, value, true, view, set))
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
                            true,
                            p,
                            cx,
                        ))
                        .child(choice_live(
                            "exp-hk-place-long",
                            gestures,
                            g.buy_set_click,
                            true,
                            view,
                            |d, v| d.gestures.buy_set_click = v,
                        ))
                        .child(caption(
                            t!("core_expert.hk_place_short").to_string(),
                            true,
                            p,
                            cx,
                        ))
                        .child(choice_live(
                            "exp-hk-place-short",
                            gestures,
                            g.short_set_click,
                            true,
                            view,
                            |d, v| d.gestures.short_set_click = v,
                        )),
                )
                .child(flag(
                    "exp-hk-same-long-short",
                    t!("core_expert.hk_same_long_short").to_string(),
                    g.same_hotkeys_for_move,
                    true,
                    view,
                    |d, on| d.gestures.set_same_hotkeys(on),
                ))
                .child(move_row(
                    t!("core_expert.hk_move_order").to_string(),
                    ["exp-hk-move-long", "exp-hk-move-short", "exp-hk-move-kind"],
                    MoveRow::OpenPrimary,
                    g.replace_buy_kind,
                    |d, v| d.gestures.replace_buy_kind = v,
                ))
                .child(move_row(
                    t!("core_expert.hk_move_tp").to_string(),
                    ["exp-hk-tp-long", "exp-hk-tp-short", "exp-hk-tp-kind"],
                    MoveRow::TpPrimary,
                    g.replace_sell_kind,
                    |d, v| d.gestures.replace_sell_kind = v,
                ))
                .child(hint(t!("core_expert.hk_additional").to_string(), p, cx))
                .child(move_row(
                    t!("core_expert.hk_move_order").to_string(),
                    [
                        "exp-hk-move-long2",
                        "exp-hk-move-short2",
                        "exp-hk-move-kind2",
                    ],
                    MoveRow::OpenSecondary,
                    g.replace_buy_kind_2,
                    |d, v| d.gestures.replace_buy_kind_2 = v,
                ))
                .child(move_row(
                    t!("core_expert.hk_move_tp").to_string(),
                    ["exp-hk-tp-long2", "exp-hk-tp-short2", "exp-hk-tp-kind2"],
                    MoveRow::TpSecondary,
                    g.replace_sell_kind_2,
                    |d, v| d.gestures.replace_sell_kind_2 = v,
                ))
                .child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap(design::ui_px(cx, 8.0))
                        .child(pending(
                            t!("core_expert.hk_pending_long").to_string(),
                            "exp-hk-pending-long",
                            g.pending_order_set_click,
                            |d, v| d.gestures.pending_order_set_click = v,
                        ))
                        .child(pending(
                            t!("core_expert.hk_pending_short").to_string(),
                            "exp-hk-pending-short",
                            g.pending_short_set_click,
                            |d, v| d.gestures.pending_short_set_click = v,
                        )),
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
                        MANUAL_SLOT_IDS[index],
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

    // The window prints its "not editable here" banner per PAGE, and this page is only partly
    // live, so it says so itself — with two different sentences, because the scope differs. Over
    // the five read-only sub-tabs the note covers the whole page below it; over the live one it
    // must not, since seventeen controls there ARE editable, so the grid carries a note naming the
    // grid.
    let live_tab = sub == HotkeysSub::Orders;
    let page_note = (!live_tab).then(|| hint(t!("core_expert.hk_mirror_only").to_string(), p, cx));
    let grid_note = live_tab.then(|| hint(t!("core_expert.hk_grid_read_only").to_string(), p, cx));

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 10.0))
        .children(page_note)
        .child(rows(cx).child(inner))
        .child(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 4.0))
                .children(grid_note)
                .child(controls),
        )
        .into_any_element()
}
