//! Regressions for the popup's placement rows: every block and every anchor must be NAMED, and
//! named differently.

use std::collections::HashSet;

use moon_core::config::layout::WindowLayout;
use rust_i18n::t;

use super::*;

/// A dropdown selects by index into [`Placement::all`], so the index and the entry must agree in
/// both directions. A mismatch shows a control naming one place while the block is drawn in
/// another, with nothing in the code that looks wrong.
#[test]
fn a_dropdowns_index_and_its_entry_are_the_same_thing() {
    let all: Vec<Placement> = Placement::all().collect();
    assert_eq!(all.len(), 1 + EmptySlot::ALL.len());
    assert_eq!(all[0], Placement::Hidden, "hidden must be the first entry");
    for (index, placement) in all.into_iter().enumerate() {
        assert_eq!(
            placement.index(),
            index,
            "{placement:?} selects the wrong row"
        );
    }
}

/// Ten entries, ten different names. A copy-paste in the match would print two of them
/// identically, and a list with the same words twice cannot be chosen from.
#[test]
fn every_entry_reads_differently_in_every_language() {
    for locale in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let mut seen = HashSet::new();
        for placement in Placement::all() {
            let label = t!(placement.label()).to_string();
            assert!(
                !label.is_empty() && !label.starts_with("crowd.slot."),
                "{placement:?} has no {locale} name"
            );
            assert!(
                seen.insert(label.clone()),
                "{locale} calls two entries {label:?}"
            );
        }
    }
}

/// The dropdown reads the two keys together: a block that is off is "hidden" whatever its anchor
/// says, and a block that is on is at its anchor. Nothing else may leak in — the anchor of a hidden
/// block is exactly what a later "show it again" must find.
#[test]
fn a_placement_is_the_switch_and_the_anchor_read_together() {
    let mut layout = WindowLayout::default();
    EmptyBlock::Minute.store(&mut layout, Some(EmptySlot::BottomStart));
    let places = EmptyPlaces::restore(&layout);

    layout.main_empty_minute = Some(false);
    let screen = EmptyScreen::restore(&layout);
    assert_eq!(
        Placement::of(EmptyBlock::Minute, &screen, places),
        Placement::Hidden
    );

    layout.main_empty_minute = Some(true);
    let screen = EmptyScreen::restore(&layout);
    assert_eq!(
        Placement::of(EmptyBlock::Minute, &screen, places),
        Placement::At(EmptySlot::BottomStart)
    );
    // The shipped screen: brand and hint in the middle, every table hidden.
    let screen = EmptyScreen::restore(&WindowLayout::default());
    assert_eq!(
        Placement::of(EmptyBlock::Logo, &screen, EmptyPlaces::default()),
        Placement::At(EmptySlot::MiddleCenter)
    );
    assert_eq!(
        Placement::of(EmptyBlock::Coins, &screen, EmptyPlaces::default()),
        Placement::Hidden
    );
}

/// The same for the five blocks: the rows are read as "this block goes there", so two rows with
/// one caption is a control nobody can aim.
#[test]
fn every_block_reads_differently_in_every_language() {
    for locale in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let mut seen = HashSet::new();
        for block in EmptyBlock::ALL {
            let label = t!(block_label(block)).to_string();
            assert!(
                !label.is_empty() && !label.starts_with("crowd.block."),
                "{block:?} has no {locale} name"
            );
            assert!(
                seen.insert(label.clone()),
                "{locale} calls two blocks {label:?}"
            );
        }
    }
}

/// A window to own the dropdowns: a `MoonSelectState` cannot exist without one.
struct Host;

impl gpui::Render for Host {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
    }
}

/// Catches the dropdowns keeping the language they were first opened in. They are built once and
/// live on the stack, so after a language switch saved in Settings the popup put a translated row
/// caption over entries still in the old language. Russian to Ukrainian on purpose: both name the
/// shown entry «По центру», so a check of the shown caption alone would leave the rest Russian.
#[gpui::test]
fn the_dropdowns_follow_a_language_switch(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let _locale = crate::test_locale::force("ru");
    let layout = WindowLayout::default();
    let screen = EmptyScreen::restore(&layout);
    let places = EmptyPlaces::restore(&layout);
    let block = EmptyBlock::Logo;
    let placement = Placement::of(block, &screen, places);
    let window = cx.add_window(|_, _| Host);
    let state = window
        .update(cx, |_, window, cx| {
            cx.new(|cx| {
                MoonSelectState::new(
                    placement_items(),
                    Some(IndexPath::new(placement.index())),
                    window,
                    cx,
                )
            })
        })
        .expect("the host window is open");
    let selects = PlaceSelects {
        selects: vec![(block, state.clone())],
        captioned_in: Rc::new(RefCell::new(rust_i18n::locale().to_string())),
    };
    let russian = t!(Placement::Hidden.label()).to_string();

    rust_i18n::set_locale("uk");
    let ukrainian = t!(Placement::Hidden.label()).to_string();
    assert_ne!(
        russian, ukrainian,
        "the check needs an entry the two name differently"
    );
    cx.update(|cx| selects.show(&screen, places, cx));

    let value = cx.read(|cx| state.read(cx).selected_value().copied());
    assert_eq!(value, Some(placement), "the refill moved the selection");
    // An entry other than the shown one; its caption is read by selecting it.
    let hidden = cx.update(|cx| {
        state.update(cx, |state, _| {
            state.set_selected_value(&Placement::Hidden);
            state.selected_label().map(|c| c.to_string())
        })
    });
    assert_eq!(hidden.as_deref(), Some(ukrainian.as_str()));
}
