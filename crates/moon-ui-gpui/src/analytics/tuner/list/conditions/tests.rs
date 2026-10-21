//! Regression coverage for editable conditions and the strategy-cell precision contract.

use super::{Condition, Operator, keeps, number};
use moon_core::db::analytics::GroupStat;

/// Build one condition without tying the oracle to the implementation's metric getter.
fn condition(column: usize, operator: Operator, text: &str) -> Condition {
    Condition {
        id: 0,
        column,
        operator,
        number: text.into(),
    }
}

/// Reversing any comparison must change the boundary result, including strictness and equality.
#[test]
fn operators_keep_the_expected_side_of_a_boundary() {
    let row = GroupStat {
        n: 20,
        ..Default::default()
    };
    for (operator, below, equal, above) in [
        (Operator::Greater, true, false, false),
        (Operator::AtLeast, true, true, false),
        (Operator::Less, false, false, true),
        (Operator::AtMost, false, true, true),
        (Operator::Equal, false, true, false),
    ] {
        for (threshold, expected) in [("19", below), ("20", equal), ("21", above)] {
            assert_eq!(
                keeps(&row, &[condition(0, operator, threshold)]),
                expected,
                "{operator:?} {threshold}"
            );
        }
    }
}

/// OR composition or treating a partial edit as zero would hide rows while the user types.
#[test]
fn conditions_are_anded_and_invalid_rows_are_ignored_individually() {
    let row = GroupStat {
        n: 20,
        pf: 1.25,
        ..Default::default()
    };
    let mut rules = vec![
        condition(0, Operator::Greater, "10"),
        condition(6, Operator::Less, "1,3"),
    ];
    assert!(keeps(&row, &rules));
    rules.push(condition(0, Operator::Less, "5"));
    assert!(!keeps(&row, &rules));
    rules[2].number = "-".into();
    assert!(keeps(&row, &rules));
    rules[1].number = "1.1".into();
    assert!(
        !keeps(&row, &rules),
        "an invalid sibling cannot disable the valid conditions"
    );
}

/// Comma decimals and negative values must parse; malformed and non-finite inputs never apply.
#[test]
fn decimal_input_accepts_both_separators_and_rejects_partial_edits() {
    assert_eq!(number(" -25,5 "), Some(-25.5));
    assert_eq!(number(".5"), Some(0.5));
    for text in ["", "-", ".", "1,2.3", "NaN", "inf", "12x", "--1", "1 000"] {
        assert_eq!(number(text), None, "{text}");
    }
}

/// Reading raw PF instead of the cell's rounding would reject equality with the visible 1.23.
#[test]
fn comparisons_use_the_rounded_cell_and_unavailable_money_does_not_match() {
    let row = GroupStat {
        pf: 1.234,
        profit: -25.0,
        avg_order: 30.0,
        ..Default::default()
    };
    assert!(keeps(&row, &[condition(6, Operator::Equal, "1.23")]));
    assert!(!keeps(&row, &[condition(3, Operator::Greater, "0")]));
    assert!(keeps(&row, &[condition(3, Operator::Greater, "-")]));
}

/// Money thresholds use rounded displayed quote units, including a negative comma decimal.
#[test]
fn monetary_conditions_use_the_displayed_quote_amount() {
    let _locale = crate::test_locale::force("en");
    let row = GroupStat {
        profit: -25.504,
        avg_order: 208.364,
        quote: moon_core::db::QuoteScope::Single(moon_core::db::QuoteCurrency::usdt()),
        ..Default::default()
    };
    assert!(keeps(&row, &[condition(3, Operator::Equal, "208,36")]));
    // Average order is always quote-denominated, regardless of the profit lens.
    assert!(!keeps(&row, &[condition(3, Operator::Less, "208.36")]));
}

/// Render the production editor geometry rather than estimating translated control widths.
struct EditorProbe;

impl gpui::Render for EditorProbe {
    /// The invalid input is the tallest editor state, and long counts exercise the footer.
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{AppContext, InteractiveElement, ParentElement, Styled};
        use moon_ui::{
            MoonButton, MoonDropdown, MoonInput, MoonInputState, MoonPalette, MoonTone, v_flex,
        };
        use rust_i18n::t;
        let p = MoonPalette::active(cx);
        let input = cx.new(|cx| MoonInputState::new(window, cx).default_value("-"));
        let longest = super::COLUMNS
            .into_iter()
            .map(|i| t!(super::METRIC_COLS[i].key).to_string())
            .max_by_key(|text| text.len())
            .unwrap();
        let content = v_flex()
            .debug_selector(|| "editor-probe".into())
            .font_family(crate::design::ui_font())
            .gap(crate::design::ui_px(cx, 8.0))
            .child(super::condition_row(
                MoonDropdown::new("probe-column").label(longest),
                MoonDropdown::new("probe-operator").label("≥"),
                MoonInput::new("probe-number")
                    .state(&input)
                    .tone(MoonTone::Danger),
                MoonButton::new("probe-remove").label("×").render(),
                true,
                p,
                cx,
            ))
            .child(super::conditions_footer(
                MoonButton::new("probe-reset")
                    .label(t!("analytics.conditions.reset_all").to_string())
                    .render(),
                t!("analytics.conditions.matches", n = 1234, m = 5678).to_string(),
                MoonButton::new("probe-done")
                    .label(t!("analytics.conditions.done").to_string())
                    .render(),
                cx,
            ));
        moon_ui::MoonPopover::new("editor-popup-probe")
            .default_open(true)
            .fit_content()
            .trigger(
                MoonButton::new("editor-probe-trigger")
                    .label(t!("analytics.conditions.title").to_string())
                    .render(),
            )
            .content(content)
    }
}

/// Losing intrinsic width on the popup or wrapping its hint would clip translated editor text.
#[gpui::test]
fn editor_rows_and_footer_fit_all_locales(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};
    use moon_core::config::{Language, UiThemeMode};
    use moon_ui::MoonTheme;
    cx.update(moon_ui::init);
    for theme in [
        UiThemeMode::Light,
        UiThemeMode::Dark,
        UiThemeMode::Graphite,
        UiThemeMode::LightExperimental,
        UiThemeMode::DarkExperimental,
    ] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for zoom in [1.0, 1.25] {
                let window =
                    cx.open_window(size(px(740.0 * zoom), px(350.0 * zoom)), |_, _| EditorProbe);
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                for _ in 0..8 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let root = visual.debug_bounds("editor-probe").expect("editor");
                let popup = visual
                    .debug_bounds("editor-popup-probe:popup")
                    .expect("popup");
                assert!(root.left() >= popup.left() && root.right() <= popup.right());
                assert!(
                    popup.right() <= px(740.0 * zoom) && popup.left() >= px(0.0),
                    "popup escaped viewport"
                );
                for selector in [
                    "an-condition-editor-row",
                    "an-condition-invalid",
                    "an-condition-footer",
                    "condition-count",
                ] {
                    let bounds = visual.debug_bounds(selector).expect(selector);
                    assert!(
                        bounds.left() >= root.left() && bounds.right() <= root.right() + px(0.5),
                        "{selector} escaped {} {theme:?} {zoom}",
                        language.code()
                    );
                }
                let hint = visual.debug_bounds("an-condition-invalid").unwrap();
                assert!(
                    hint.size.height < px(30.0 * zoom),
                    "the invalid hint must remain one line"
                );
                let text = rust_i18n::t!("analytics.conditions.invalid").to_string();
                let need =
                    visual.update(|_, cx| crate::design::ui_caption_text_width(cx, &text, 400.0));
                assert!(
                    f32::from(hint.size.width) + 0.5 >= need,
                    "invalid hint clipped: {}",
                    language.code()
                );
            }
        }
    }
}

/// The real chip renderer must fit even a long, unfinished numeric draft.
struct ChipsProbe;

impl gpui::Render for ChipsProbe {
    /// Draw several chips so the production wrap policy is exercised at narrow widths.
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let caption = format!(
            "{} > {} USDT",
            rust_i18n::t!("analytics.col.avg_order"),
            "1234567890".repeat(20)
        );
        super::chips_row(
            (0..3)
                .map(|id| {
                    super::condition_chip(id, caption.clone(), |_, _, _| {}, |_, _, _| {}, cx)
                })
                .collect(),
            moon_ui::MoonButton::new("probe-chip-reset")
                .label(rust_i18n::t!("analytics.conditions.reset").to_string())
                .render(),
            cx,
        )
    }
}

/// Removing the chip's fitting budget must fail at the narrowest card instead of clipping it.
#[gpui::test]
fn chips_wrap_without_clipping_numeric_drafts(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};
    use moon_core::config::{Language, UiThemeMode};
    use moon_ui::MoonTheme;
    for theme in [
        UiThemeMode::Light,
        UiThemeMode::Dark,
        UiThemeMode::Graphite,
        UiThemeMode::LightExperimental,
        UiThemeMode::DarkExperimental,
    ] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for zoom in [1.0, 1.25] {
                let window =
                    cx.open_window(size(px(740.0 * zoom), px(250.0 * zoom)), |_, _| ChipsProbe);
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let row = visual
                    .debug_bounds("an-condition-chips")
                    .expect("chips row");
                for selector in ["condition-chip-0", "condition-chip-1", "condition-chip-2"] {
                    let chip = visual.debug_bounds(selector).expect("chip");
                    assert!(
                        chip.left() >= row.left()
                            && chip.right() <= row.right() + px(0.5)
                            && chip.bottom() <= row.bottom(),
                        "chip clipped {} {theme:?} {zoom}",
                        language.code()
                    );
                }
            }
        }
    }
}
