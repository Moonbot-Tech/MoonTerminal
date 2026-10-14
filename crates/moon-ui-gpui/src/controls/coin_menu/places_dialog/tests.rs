//! Pure hide/authorize rules, and a layout pass of the places column in every locale.
//!
//! The column is the same function the dialog renders. A backend is not required: the face is
//! data, and a cell that clips its text is narrower than the measured string.

use std::rc::Rc;

use gpui::{
    App, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, VisualTestContext,
    Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::MoonTheme;
use rust_i18n::t;

use super::super::places::CoinPlace;
use super::{
    ConfirmKind, FaceAction, FaceConfirm, FaceRow, HIDE_MS, HiddenPlace, PlaceKey, PlacesFace,
    RemovalUi, confirm_text, measure_text, note_removal, places_column, reconcile_hidden,
    removal_ui, visible_places,
};
use crate::test_locale::force;

fn core_place(core: u64) -> CoinPlace {
    CoinPlace::Core {
        core,
        list_enabled: true,
    }
}

fn strategy_place(core: u64, sid: u64) -> CoinPlace {
    CoinPlace::Strategy {
        core,
        sid,
        folder: String::new(),
        strategy: "UNIT_01".to_string(),
    }
}

/// A refused removal must not invent a hide. The row the user still sees is the store's row.
#[test]
fn a_refused_removal_keeps_the_row() {
    let places = vec![core_place(1)];
    let mut hidden = Vec::new();
    assert!(!note_removal(&mut hidden, PlaceKey::Core(1), false, 0));
    assert!(hidden.is_empty());
    assert_eq!(visible_places(&places, &hidden).len(), 1);
}

/// An accepted removal hides that place until the store drops it. A later snapshot that no
/// longer lists it ends the hide, including when the echo is the only change.
#[test]
fn a_late_echo_drops_the_hide_once_the_place_is_gone() {
    let mut hidden = vec![HiddenPlace {
        key: PlaceKey::Core(1),
        at_ms: 0,
    }];
    note_removal(&mut hidden, PlaceKey::Core(1), true, 0);
    reconcile_hidden(&mut hidden, &[], 1_000);
    assert!(hidden.is_empty());
}

/// A snapshot that still lists the hidden place, and lists something else too, is not evidence
/// the removal landed. The hide stays and the other row stays visible.
#[test]
fn an_unrelated_update_keeps_the_hidden_place_hidden() {
    let current = vec![core_place(1), strategy_place(1, 7)];
    let mut hidden = vec![HiddenPlace {
        key: PlaceKey::Core(1),
        at_ms: 0,
    }];
    reconcile_hidden(&mut hidden, &current, 1_000);
    assert_eq!(hidden.len(), 1);
    let shown = visible_places(&current, &hidden);
    assert_eq!(shown.len(), 1);
    assert!(matches!(shown[0], CoinPlace::Strategy { sid: 7, .. }));
}

/// The hide lasts through 15 seconds and not one millisecond past it. The place is still listed,
/// so dropping the hide puts the row back.
#[test]
fn a_hide_stays_at_fifteen_seconds_and_the_row_returns_after() {
    let current = vec![core_place(1)];
    let mut hidden = vec![HiddenPlace {
        key: PlaceKey::Core(1),
        at_ms: 0,
    }];
    reconcile_hidden(&mut hidden, &current, HIDE_MS);
    assert_eq!(
        hidden.len(),
        1,
        "an age of exactly {HIDE_MS} ms stays hidden"
    );
    assert!(visible_places(&current, &hidden).is_empty());
    reconcile_hidden(&mut hidden, &current, HIDE_MS + 1);
    assert!(hidden.is_empty());
    assert_eq!(visible_places(&current, &hidden).len(), 1);
}

/// A write that changed the list hides the row and confirms. A write that changed nothing hides
/// the row with no sentence. A refusal keeps the row; the refused sentence is the failure key.
#[test]
fn removal_ui_hides_a_noop_without_a_confirm_and_keeps_a_refusal() {
    assert_eq!(removal_ui(Ok(true)), RemovalUi::HideWithConfirm);
    assert_eq!(removal_ui(Ok(false)), RemovalUi::HideSilent);
    assert_eq!(removal_ui(Err(())), RemovalUi::KeepFailed);
}

/// Each outcome draws its own key. The oracle is `t!` of that key, not a string copied out of
/// the function, so a swapped arm fails.
#[test]
fn confirm_text_uses_the_key_for_that_outcome() {
    let _english = force("en");
    let cases = [
        (
            ConfirmKind::RemovedCore,
            "CORE_A",
            t!(
                "coin_bl_places.removed_core",
                coin = "XALPHA",
                core = "CORE_A"
            )
            .to_string(),
        ),
        (
            ConfirmKind::RemovedStrategy,
            "UNIT_01",
            t!(
                "coin_bl_places.removed_strategy",
                coin = "XALPHA",
                name = "UNIT_01"
            )
            .to_string(),
        ),
        (
            ConfirmKind::LiftedTemp,
            "CORE_A",
            t!(
                "coin_bl_places.lifted_temp",
                coin = "XALPHA",
                core = "CORE_A"
            )
            .to_string(),
        ),
        (
            ConfirmKind::Refused,
            "CORE_A",
            t!("coin_bl_places.remove_failed", coin = "XALPHA").to_string(),
        ),
    ];
    for (kind, name, oracle) in cases {
        assert_eq!(confirm_text(kind, "XALPHA", name), oracle, "{kind:?}");
    }
    drop(_english);
    let _russian = force("ru");
    assert_eq!(
        confirm_text(ConfirmKind::Refused, "XALPHA", "CORE_A"),
        t!("coin_bl_places.remove_failed", coin = "XALPHA").to_string()
    );
    assert_eq!(
        confirm_text(ConfirmKind::RemovedCore, "WBTC", "CORE_A"),
        t!(
            "coin_bl_places.removed_core",
            coin = "WBTC",
            core = "CORE_A"
        )
        .to_string()
    );
}

struct Expect {
    selector: String,
    text: String,
    caption: bool,
    weight: f32,
    mono: bool,
}

fn click() -> Rc<dyn Fn(&mut Window, &mut App)> {
    Rc::new(|_, _| {})
}

fn button(label: String) -> FaceAction {
    FaceAction {
        label,
        enabled: true,
        tip: None,
        on_click: click(),
    }
}

/// `CORE_A / <folder> / UNIT_01...` is long enough to overflow a 448 px dialog and still sits
/// inside the 1800 px probe window.
const LONG_PATH: &str =
    "CORE_A / folder_folder_folder_folder_folder_path / UNIT_01_strategy_name_XALPHA_WBTC";

fn summary(places: usize, cores: usize) -> String {
    let places_n = places as u64;
    let cores_n = cores as u64;
    let places_key = super::plural_key("coin_bl_places.places", places_n);
    let cores_key = super::plural_key("coin_bl_places.cores", cores_n);
    let places_text = t!(&places_key, n = places_n).to_string();
    let cores_text = t!(&cores_key, n = cores_n).to_string();
    t!(
        "coin_bl_places.summary",
        places = places_text,
        cores = cores_text
    )
    .to_string()
}

fn face_empty() -> PlacesFace {
    PlacesFace {
        coin: "XALPHA".to_string(),
        title: t!("coin_bl_places.title").to_string(),
        summary: summary(0, 0),
        rows: Vec::new(),
        empty: Some(t!("coin_bl_places.empty", coin = "XALPHA").to_string()),
        footer: t!("coin_bl_places.footer").to_string(),
        confirm: None,
    }
}

fn face_rows(confirm: bool) -> PlacesFace {
    let open = || Some(button(t!("coin_bl_places.open").to_string()));
    let rows = vec![
        FaceRow {
            title: "CORE_A".to_string(),
            sub: format!(
                "{} {}",
                t!("coin_bl_places.core_path"),
                t!("coin_bl_places.list_off")
            ),
            tag: t!("coin_bl_places.tag_core").to_string(),
            tag_tone: moon_ui::MoonTone::Danger,
            open: open(),
            action: button(t!("coin_bl_places.remove").to_string()),
        },
        FaceRow {
            title: "CORE_A".to_string(),
            sub: t!("coin_bl_places.temp_left", time = "4m").to_string(),
            tag: t!("coin_bl_places.tag_temp").to_string(),
            tag_tone: moon_ui::MoonTone::Warning,
            open: None,
            action: button(t!("coin_bl_places.lift").to_string()),
        },
        FaceRow {
            title: LONG_PATH.to_string(),
            sub: t!("coin_bl_places.strategy_field").to_string(),
            tag: t!("coin_bl_places.tag_strategy").to_string(),
            tag_tone: moon_ui::MoonTone::Info,
            open: open(),
            action: button(t!("coin_bl_places.remove").to_string()),
        },
    ];
    PlacesFace {
        coin: "XALPHA".to_string(),
        title: t!("coin_bl_places.title").to_string(),
        summary: summary(3, 1),
        rows,
        empty: None,
        footer: t!("coin_bl_places.footer").to_string(),
        confirm: confirm.then(|| FaceConfirm {
            text: t!(
                "coin_bl_places.removed_strategy",
                coin = "XALPHA",
                name = "UNIT_01"
            )
            .to_string(),
            ok: true,
        }),
    }
}

fn expects(face: &PlacesFace) -> Vec<Expect> {
    let mut out = vec![
        Expect {
            selector: "coin-bl-coin".to_string(),
            text: face.coin.clone(),
            caption: false,
            weight: 700.0,
            mono: true,
        },
        Expect {
            selector: "coin-bl-title".to_string(),
            text: face.title.clone(),
            caption: true,
            weight: 400.0,
            mono: false,
        },
        Expect {
            selector: "coin-bl-summary".to_string(),
            text: face.summary.clone(),
            caption: true,
            weight: 400.0,
            mono: false,
        },
        Expect {
            selector: "coin-bl-footer".to_string(),
            text: face.footer.clone(),
            caption: true,
            weight: 400.0,
            mono: false,
        },
    ];
    if let Some(empty) = &face.empty {
        out.push(Expect {
            selector: "coin-bl-empty".to_string(),
            text: empty.clone(),
            caption: false,
            weight: 400.0,
            mono: false,
        });
    }
    if let Some(confirm) = &face.confirm {
        out.push(Expect {
            selector: "coin-bl-confirm-text".to_string(),
            text: confirm.text.clone(),
            caption: false,
            weight: 400.0,
            mono: false,
        });
    }
    for (index, row) in face.rows.iter().enumerate() {
        out.push(Expect {
            selector: format!("coin-bl-row-{index}-title"),
            text: row.title.clone(),
            caption: false,
            weight: 400.0,
            mono: true,
        });
        out.push(Expect {
            selector: format!("coin-bl-row-{index}-sub"),
            text: row.sub.clone(),
            caption: true,
            weight: 400.0,
            mono: false,
        });
        if let Some(open) = &row.open {
            out.push(Expect {
                selector: format!("coin-bl-row-{index}-open"),
                text: open.label.clone(),
                caption: false,
                weight: 400.0,
                mono: false,
            });
        }
        out.push(Expect {
            selector: format!("coin-bl-row-{index}-remove"),
            text: row.action.label.clone(),
            caption: false,
            weight: 400.0,
            mono: false,
        });
    }
    out
}

struct Host {
    face: PlacesFace,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(places_column(&self.face, window.viewport_size(), cx))
    }
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn inside(child: Bounds<Pixels>, parent: Bounds<Pixels>) -> bool {
    child.origin.x + px(0.5) >= parent.origin.x
        && child.right() <= parent.right() + px(0.5)
        && child.origin.y + px(0.5) >= parent.origin.y
        && child.bottom() <= parent.bottom() + px(0.5)
}

fn fits(bounds: Bounds<Pixels>, width: f32) -> bool {
    f32::from(bounds.size.width) + 0.1 >= width
}

/// Every locale: an empty dialog, all three kinds with a long strategy path, and that column
/// with the confirm line. Text stays inside the column and at least as wide as its measure.
#[gpui::test]
fn places_column_fits_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        );
    });
    for language in Language::ALL {
        let _locale = force(language.code());
        let faces = [
            ("empty", face_empty()),
            ("rows", face_rows(false)),
            ("confirm", face_rows(true)),
        ];
        for (name, face) in faces {
            let expected = expects(&face);
            let window = cx.open_window(gpui::size(px(1800.0), px(900.0)), |_, _| Host { face });
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let at = format!("{} {name}", language.code());
            let dialog = visual
                .debug_bounds("coin-bl-dialog")
                .unwrap_or_else(|| panic!("coin-bl-dialog not laid out ({at})"));
            let (widths, cap): (Vec<f32>, f32) = visual.update(|_window, app| {
                let widths = expected
                    .iter()
                    .map(|item| measure_text(app, &item.text, item.caption, item.weight, item.mono))
                    .collect();
                (widths, super::title_cap(app))
            });
            for (item, width) in expected.iter().zip(widths) {
                let bounds = visual
                    .debug_bounds(leak(item.selector.clone()))
                    .unwrap_or_else(|| panic!("{} not laid out ({at})", item.selector));
                assert!(
                    inside(bounds, dialog),
                    "{} escaped the dialog ({at})",
                    item.selector
                );
                let row_title =
                    item.selector.starts_with("coin-bl-row-") && item.selector.ends_with("-title");
                if row_title {
                    assert!(
                        f32::from(bounds.size.width) <= cap + 0.5,
                        "{} is {:.1}px above the {:.1}px cap ({at})",
                        item.selector,
                        f32::from(bounds.size.width),
                        cap
                    );
                } else {
                    assert!(
                        fits(bounds, width),
                        "{} is {:.1}px for a {:.1}px string ({at})",
                        item.selector,
                        f32::from(bounds.size.width),
                        width
                    );
                }
            }
        }
    }
}

fn install_theme(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        );
    });
}

fn face_long_sentences() -> PlacesFace {
    let sentence = "XALPHA WBTC ".repeat(40);
    PlacesFace {
        coin: "XALPHA".to_string(),
        title: "Places".to_string(),
        summary: "1 place".to_string(),
        rows: Vec::new(),
        empty: Some(sentence.clone()),
        footer: sentence.clone(),
        confirm: Some(FaceConfirm {
            text: sentence,
            ok: true,
        }),
    }
}

fn face_many_rows() -> PlacesFace {
    let mut rows = Vec::with_capacity(30);
    for _ in 0..30 {
        rows.push(FaceRow {
            title: "CORE_A".to_string(),
            sub: "XALPHA".to_string(),
            tag: "Core".to_string(),
            tag_tone: moon_ui::MoonTone::Danger,
            open: None,
            action: button("Remove".to_string()),
        });
    }
    PlacesFace {
        coin: "XALPHA".to_string(),
        title: "Places".to_string(),
        summary: "30 places".to_string(),
        rows,
        empty: None,
        footer: "XALPHA stays listed until the core drops it.".to_string(),
        confirm: None,
    }
}

/// A viewport narrower than the sentences keeps every run inside the column.
#[gpui::test]
fn places_column_wraps_inside_a_narrow_viewport(cx: &mut gpui::TestAppContext) {
    install_theme(cx);
    let _locale = force("en");
    let sentence = "XALPHA WBTC ".repeat(40);
    let window = cx.open_window(gpui::size(px(480.0), px(800.0)), |_, _| Host {
        face: face_long_sentences(),
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    let dialog = visual
        .debug_bounds("coin-bl-dialog")
        .expect("coin-bl-dialog not laid out");
    let (cap, sentence_w) = visual.update(|window, app| {
        let cap = super::column_width_cap(f32::from(window.viewport_size().width));
        let sentence_w = measure_text(app, &sentence, true, 400.0, false);
        (cap, sentence_w)
    });
    assert!(
        sentence_w > cap,
        "the fixture sentence is {:.1}px inside a {:.1}px cap",
        sentence_w,
        cap
    );
    assert!(
        f32::from(dialog.size.width) <= cap + 0.5,
        "dialog is {:.1}px above the {:.1}px cap",
        f32::from(dialog.size.width),
        cap
    );
    for selector in [
        "coin-bl-coin",
        "coin-bl-title",
        "coin-bl-summary",
        "coin-bl-empty",
        "coin-bl-footer",
        "coin-bl-confirm-text",
    ] {
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} not laid out"));
        assert!(inside(bounds, dialog), "{selector} escaped the dialog");
    }
}

/// Thirty places scroll inside the viewport cap, and the footer stays under that scroller.
#[gpui::test]
fn places_rows_scroll_under_the_viewport_cap(cx: &mut gpui::TestAppContext) {
    install_theme(cx);
    let _locale = force("en");
    let window = cx.open_window(gpui::size(px(1000.0), px(500.0)), |_, _| Host {
        face: face_many_rows(),
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    let rows = visual
        .debug_bounds("coin-bl-rows")
        .expect("coin-bl-rows not laid out");
    let footer = visual
        .debug_bounds("coin-bl-footer")
        .expect("coin-bl-footer not laid out");
    let cap = visual
        .update(|window, _app| super::rows_height_cap(f32::from(window.viewport_size().height)));
    assert!(
        f32::from(rows.size.height) <= cap + 0.5,
        "rows are {:.1}px above the {:.1}px cap",
        f32::from(rows.size.height),
        cap
    );
    assert!(
        footer.origin.y + px(0.5) >= rows.bottom(),
        "footer starts above the row scroller"
    );
}
