use super::super::state::PendingDelta;
use super::{ChipKind, FieldChip, ListFieldModel, MAX_CHIPS, pending_text};
use gpui::SharedString;

fn chip(token: &str, kind: ChipKind) -> FieldChip {
    FieldChip {
        token: SharedString::from(token.to_string()),
        text: SharedString::from(token.to_string()),
        tip: SharedString::default(),
        id: SharedString::from(format!("an-pick-b-{token}")),
        kind,
        alpha: 1.0,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// An empty whitelist says what empty MEANS for the strategy, and offers nothing to clear
/// or copy.
#[test]
fn an_empty_whitelist_explains_itself_and_cannot_be_cleared() {
    let _locale = crate::test_locale::force("en");
    let m = ListFieldModel::new(false, Some(Vec::new()));
    assert_eq!(m.count, Some(0));
    assert!(!m.can_clear && !m.can_copy);
    assert_eq!(m.key, "CoinsWhiteList");
    assert_eq!(m.empty.as_deref(), Some("The list is empty."));
    assert!(
        m.legend
            .iter()
            .any(|l| l.contains("every coin except the blacklist")),
        "legend: {:?}",
        m.legend
    );
}

/// Without a strategy both fields say so, and the count is not claimed.
#[test]
fn no_strategy_fields_say_so() {
    let _locale = crate::test_locale::force("en");
    for black in [true, false] {
        let m = ListFieldModel::new(black, None);
        assert_eq!(m.count, None);
        assert_eq!(m.empty.as_deref(), Some("No strategy selected"));
        assert!(!m.can_clear && !m.can_copy);
    }
}

/// The count is the WORKING list: a removal shown for undo is not counted, and a cut past
/// the chip cap is stated.
#[test]
fn removed_chips_are_not_counted_and_the_cap_is_stated() {
    let _locale = crate::test_locale::force("en");
    let mut chips = vec![
        chip("XAAA", ChipKind::Saved),
        chip("XBBB", ChipKind::Removed),
    ];
    chips.push(chip("XCCC", ChipKind::Added));
    let m = ListFieldModel::new(true, Some(chips));
    assert_eq!(m.count, Some(2));
    assert!(m.can_clear);

    let many: Vec<_> = (0..MAX_CHIPS + 3)
        .map(|i| chip(&format!("X{i:04}"), ChipKind::Saved))
        .collect();
    let m = ListFieldModel::new(true, Some(many));
    assert_eq!((m.chips.len(), m.more), (MAX_CHIPS, 3));
}

/// The band is built from the pending delta: per side, sorted, additions then removals.
#[test]
fn the_pending_band_is_signed_per_side() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(pending_text(&PendingDelta::default()), None);
    let d = PendingDelta {
        added_bl: strings(&["XAAA", "XBBB"]),
        removed_bl: strings(&["XCCC"]),
        added_wl: Vec::new(),
        removed_wl: strings(&["XDDD"]),
    };
    assert_eq!(
        pending_text(&d).as_deref(),
        Some("BL +XAAA +XBBB \u{2212}XCCC · WL \u{2212}XDDD")
    );
}

/// The count is in COINS: two written entries of one coin count once.
#[test]
fn two_entries_of_one_coin_count_once() {
    let _locale = crate::test_locale::force("en");
    let mut second = chip("BTC", ChipKind::Saved);
    second.text = SharedString::from("BTC_0626");
    let m = ListFieldModel::new(true, Some(vec![chip("BTC", ChipKind::Saved), second]));
    assert_eq!(m.count, Some(1));
    assert_eq!(m.chips.len(), 2, "both entries are still drawn");
}

/// Exercise intrinsic chip sizing inside the real wrapping list well, rather than a fixed box.
struct ChipProbe;

impl gpui::Render for ChipProbe {
    /// Render both list kinds with short, added and removed synthetic names.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{IntoElement, ParentElement, Styled};
        let acts = super::ListFieldActs {
            on_remove: std::rc::Rc::new(|_, _, _| {}),
            on_restore: std::rc::Rc::new(|_, _, _| {}),
            on_copy: std::rc::Rc::new(|_, _| {}),
            on_clear: std::rc::Rc::new(|_, _| {}),
        };
        moon_ui::v_flex()
            .w_full()
            .children([true, false].map(|black| {
                let mut names = vec![
                    chip("XALPHA", ChipKind::Saved),
                    chip("XBETA", ChipKind::Added),
                    chip("XGAMMA", ChipKind::Removed),
                ];
                for name in &mut names {
                    name.id = format!("chip-{}-{}", black, name.token).into();
                }
                super::list_field_el(
                    &ListFieldModel::new(black, Some(names)),
                    None,
                    &acts,
                    moon_ui::MoonPalette::active(cx),
                    cx,
                )
                .into_any_element()
            }))
    }
}

/// A fixed or shrinking chip width must still cover the measured name inside the wrapping well.
#[gpui::test]
fn livefix_coin_names_keep_their_measured_width(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for width in [320.0, 470.0, 620.0] {
            cx.update(|cx| {
                moon_ui::MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(
                        moon_core::config::UiThemeMode::Light,
                        1.0,
                    ),
                    cx,
                )
            });
            let window = cx.open_window(gpui::size(px(width), px(900.0)), |_, _| ChipProbe);
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            for black in [true, false] {
                for (token, label) in [
                    ("XALPHA", "XALPHA"),
                    ("XBETA", "+XBETA"),
                    ("XGAMMA", "XGAMMA"),
                ] {
                    let selector = Box::leak(format!("chip-{black}-{token}").into_boxed_str());
                    let bounds = visual.debug_bounds(selector).unwrap();
                    let need = visual
                        .update(|_, cx| crate::design::mono_caption_text_width(cx, label, 400.0));
                    assert!(
                        bounds.size.width + px(0.5) >= px(need),
                        "coin label collapsed in {} at {width}",
                        language.code()
                    );
                }
            }
        }
    }
}
