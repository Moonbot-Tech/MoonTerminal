//! The Hotkeys page: the search and conflict filter, when a gesture editor shows, the pull that
//! lands in the draft only, and the layout sweep over the banner, categories, table and built-ins.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use moon_core::config::{AppConfig, KeySlot, MANUAL_STRATEGY_KEYS};
use moon_core::feed::{CoreHotkeyAction, CoreHotkeyLayout};

use super::super::clash::Severity;
use super::super::pull::{PullRow, apply_pull, preview_core_hotkeys};
use super::super::registry::HotkeyGroup;
use super::{EDITOR_W, HkRow, RowText, banner, builtin, categories, filter_rows, gesture_shown};
use super::{split_combo, table};
use crate::settings::layout_probe::{ProbeSpec, Probed, sweep};
use crate::settings::{Tab, dirty_pages};

fn text(name: &str, moonbot: Option<&str>, keys: &[&str], problem: bool) -> RowText {
    RowText {
        name: name.to_string(),
        moonbot: moonbot.map(str::to_string),
        keys: keys.iter().map(|k| k.to_string()).collect(),
        problem,
    }
}

fn rows() -> Vec<RowText> {
    vec![
        text("Отменить покупку", Some("Cancel Buy"), &["alt-z"], false),
        text(
            "Продать всё по рынку",
            Some("Panic Sell"),
            &["alt-6"],
            false,
        ),
        text(
            "Разделить ордер на части",
            Some("Split Order X"),
            &["ctrl-x"],
            true,
        ),
        text("Новый Long", Some("New Long"), &[], false),
    ]
}

/// A key typed the way the user reads it ("Alt+Z", any case, blanks) finds the row whose stored
/// binding is `alt-z`, and nothing else.
#[test]
fn the_search_finds_a_row_by_its_key() {
    let rows = rows();
    assert_eq!(filter_rows(&rows, "alt+z", false), vec![0]);
    assert_eq!(filter_rows(&rows, "Alt + Z", false), vec![0]);
    assert_eq!(filter_rows(&rows, "ALT-Z", false), vec![0]);
}

/// A name query matches the Russian name and the MoonBot name alike; a blank query keeps every
/// row; the conflict filter keeps only problem rows, and both filters combine.
#[test]
fn the_search_finds_a_row_by_either_name_and_the_conflict_filter_narrows_it() {
    let rows = rows();
    assert_eq!(filter_rows(&rows, "продать", false), vec![1]);
    assert_eq!(filter_rows(&rows, "panic sell", false), vec![1]);
    assert_eq!(filter_rows(&rows, "  ", false), vec![0, 1, 2, 3]);
    assert_eq!(filter_rows(&rows, "", true), vec![2]);
    assert_eq!(filter_rows(&rows, "продать", true), Vec::<usize>::new());
}

/// Outside the Mouse category an unset gesture stays hidden until "+ жест"; a set one shows; on
/// the Mouse category every gesture shows.
#[test]
fn a_gesture_row_is_hidden_when_unset_outside_the_mouse_category() {
    assert!(!gesture_shown(false, false, false));
    assert!(gesture_shown(false, true, false));
    assert!(gesture_shown(false, false, true));
    assert!(gesture_shown(true, false, false));
}

/// A core layout whose Cancel Buy is Ctrl+Shift+`letter` and every other key unset.
fn layout_with_cancel_buy(letter: u8) -> CoreHotkeyLayout {
    use moon_core::config::moonbot_import::shortcut::{MOD_CTRL, MOD_SHIFT};
    CoreHotkeyLayout {
        order_size: [0; 6],
        sell_preset: [0; 6],
        named: CoreHotkeyAction::ALL.map(|a| {
            (
                a,
                if a == CoreHotkeyAction::CancelBuy {
                    MOD_CTRL | MOD_SHIFT | u16::from(letter)
                } else {
                    0
                },
            )
        }),
    }
}

/// The pull dialog's Apply writes the diff it SHOWED: a refresh that replaces the core's layout
/// (Ctrl+Shift+Q -> Ctrl+Shift+W) while the dialog is open must not change what lands. It lands
/// in the draft, so the Hotkeys page reads dirty until Save. (That `confirm_core_pull` never
/// touches the saved config or `hotkeys.toml` is pinned by a theme_contract source check.)
#[test]
fn a_pull_applies_the_shown_diff_into_the_draft() {
    let config = AppConfig::headless(Vec::new());
    let mut draft = config.clone();
    let manual = [0u16; MANUAL_STRATEGY_KEYS];
    let shown = preview_core_hotkeys(&draft.hotkeys, &layout_with_cancel_buy(b'Q'), &manual);
    // Layout B arrives after the dialog opened; its preview is what a re-read would apply.
    let refreshed = preview_core_hotkeys(&draft.hotkeys, &layout_with_cancel_buy(b'W'), &manual);
    let new_key = |rows: &[PullRow]| {
        rows.iter()
            .find(|r| r.slot == KeySlot::CancelBuy)
            .and_then(|r| r.new_key.clone())
            .expect("Cancel Buy will apply")
    };
    assert_ne!(new_key(&shown), new_key(&refreshed));

    assert!(apply_pull(&mut draft.hotkeys, &shown, &[]));

    assert_eq!(draft.hotkeys.key(KeySlot::CancelBuy), new_key(&shown));
    assert!(dirty_pages(&config, &draft).contains(Tab::Hotkeys));
}

/// The built-in lines split into their keys and what they do; a line without the bracket stays
/// whole.
#[test]
fn a_builtin_line_splits_into_keys_and_description() {
    assert_eq!(
        split_combo("[Esc] закрыть активный чарт"),
        (Some("Esc".to_string()), "закрыть активный чарт".to_string())
    );
    assert_eq!(split_combo("без скобок"), (None, "без скобок".to_string()));
}

/// The probe's host view: the builder at the body's width, inside a scroll box.
struct Host {
    scroll: ScrollHandle,
    width: f32,
    build: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx);
        div()
            .w(px(self.width))
            .h_full()
            .debug_selector(|| "probe-body".into())
            .child(
                div()
                    .id("probe-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().w_full().flex_none().child(content)),
            )
    }
}

impl Probed for Host {
    fn scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll.clone())
    }
}

/// A stand-in editor at the key column's editor width.
fn editor() -> AnyElement {
    div().w(px(EDITOR_W)).h(px(24.0)).into_any_element()
}

/// The conflict caption the sweep wraps; long on purpose.
fn conflict_text() -> String {
    rust_i18n::t!(
        "hotkeys.clash.key",
        rows = "Центрировать (Ctrl+→), Разделить ордер на части"
    )
    .to_string()
}

/// Synthetic rows: a plain key row, a conflict on a long name, a row whose gesture is hidden, and
/// a mouse row with two editors.
fn synthetic_rows() -> Vec<HkRow> {
    let noop: crate::settings::common::Action = Rc::new(|_, _| {});
    vec![
        HkRow {
            id: "cancel-buy".into(),
            name: rust_i18n::t!("hotkeys.cmd.cancel_buy").to_string(),
            moonbot: Some("Cancel Buy".into()),
            hint: None,
            scope: rust_i18n::t!("hotkeys.scope.cursor").to_string(),
            problems: Vec::new(),
            muted: false,
            editors: vec![editor()],
            add_gesture: Some(noop.clone()),
        },
        HkRow {
            id: "cancel-all-buys-all-cores".into(),
            name: rust_i18n::t!("hotkeys.cmd.cancel_all_buys_all_cores").to_string(),
            moonbot: Some("Cancel buys in all bots".into()),
            hint: None,
            scope: rust_i18n::t!("hotkeys.scope.window").to_string(),
            problems: vec![
                (conflict_text(), Severity::Shadowed),
                (
                    rust_i18n::t!("hotkeys.clash.bare_key").to_string(),
                    Severity::Bare,
                ),
            ],
            muted: false,
            editors: vec![editor()],
            add_gesture: Some(noop),
        },
        HkRow {
            id: "buy-move".into(),
            name: rust_i18n::t!("hotkeys.cmd.buy_move2").to_string(),
            moonbot: Some("Move Open Long #2".into()),
            hint: None,
            scope: rust_i18n::t!("hotkeys.scope.book").to_string(),
            problems: Vec::new(),
            muted: true,
            editors: vec![editor(), editor()],
            add_gesture: None,
        },
    ]
}

/// The banner, the category strip, a table with a conflict, an empty table and the unfolded
/// built-ins stay inside the body, wrap their prose uncut and never overlap, in every language
/// at the default window, the narrowest one with the sidebar (760) and the minimum window; the
/// problem and action cells of a conflicting row stay inside, so no column overflows.
#[gpui::test]
fn the_hotkeys_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |_window, _cx: &mut Context<Host>, (width, _)| Host {
            scroll: ScrollHandle::new(),
            width,
            build: Rc::new(move |_window, cx| {
                let noop: crate::settings::common::Action = Rc::new(|_, _| {});
                let pick: Rc<dyn Fn(HotkeyGroup, &mut Window, &mut App)> = Rc::new(|_, _, _| {});
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .children(banner(3, false, noop.clone(), cx))
                    .child(categories(
                        HotkeyGroup::Chart,
                        &[0, 1, 2, 0, 0, 0, 0],
                        pick,
                        cx,
                    ))
                    .child(table(synthetic_rows(), false, String::new(), cx))
                    .child(
                        div()
                            .debug_selector(|| "hotkeys-empty-host".into())
                            .child(table(
                                Vec::new(),
                                true,
                                rust_i18n::t!("hotkeys.conflicts.none").to_string(),
                                cx,
                            )),
                    )
                    .child(builtin(true, noop, cx))
                    .into_any_element()
            }),
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: vec![
                "hotkeys-banner".into(),
                "hotkeys-categories".into(),
                "hk-row-cancel-buy".into(),
                "hk-row-cancel-all-buys-all-cores".into(),
                "hk-row-buy-move".into(),
                // The row's last cells: a column that does not fit the body would push these out.
                "hk-problem-cancel-all-buys-all-cores".into(),
                "hk-action-cancel-all-buys-all-cores".into(),
                "hk-scope-cancel-all-buys-all-cores".into(),
                "hk-action-buy-move".into(),
                "hotkeys-empty".into(),
                "hotkeys-builtin-head".into(),
                "hotkeys-builtin-body".into(),
            ],
            no_overlap: vec![
                vec![
                    "hotkeys-banner".into(),
                    "hotkeys-categories".into(),
                    "hk-row-cancel-buy".into(),
                    "hk-row-cancel-all-buys-all-cores".into(),
                    "hk-row-buy-move".into(),
                    "hotkeys-empty-host".into(),
                    "hotkeys-builtin-head".into(),
                ],
                vec![
                    "hk-name-cancel-all-buys-all-cores".into(),
                    "hk-problem-cancel-all-buys-all-cores".into(),
                ],
            ],
            wraps: vec![(
                "hk-name-cancel-all-buys-all-cores".into(),
                rust_i18n::t!("hotkeys.cmd.cancel_all_buys_all_cores").to_string(),
            )],
            last: Some("hotkeys-builtin-body".into()),
            ..Default::default()
        },
    );
}
