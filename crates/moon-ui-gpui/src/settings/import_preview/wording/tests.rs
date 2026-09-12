//! Regression tests for rendering actual import plans in the active locale.

use super::{caption, preview_value, reason, warning};
use moon_core::config::hotkeys::HotkeysConfig;
use moon_core::config::moonbot_import::plan::{PlanContext, build_plan};
use moon_core::config::moonbot_import::preview::{
    ImportReason, ImportWarning, PreviewCaption, PreviewValue,
};
use moon_core::config::moonbot_import::reader::IniSection;
use moon_core::config::moonbot_import::schema_v7::*;
use moon_core::config::orders::OrdersStyleSet;
use moon_core::config::theme::ChartThemeSet;

/// Builds a MoonBotConfig with meaningful values for plan tests.
fn mb_config() -> MoonBotConfig {
    let mut shortcuts = [0u16; 27];
    shortcuts[0] = 0x8000 | 0x5A; // CancelBuy = Alt+Z
    shortcuts[4] = 0x0075; // ReloadBook = F6 (no corresponding Terminal action)
    shortcuts[25] = 0x4000 | 0x2E; // CancelAllBuys = Ctrl+Delete (matches the default)
    MoonBotConfig {
        config_version: 1,
        ui: UiBlock {
            hide_demo_button: false,
            confirm_close: false,
            new_markets_on_top: false,
            coins_sort_order: 0,
            hotkeys: HotkeysPublic {
                filled: true,
                ver: 2,
                order_sizes: [111.0, 222.0, 333.0, 444.0, 555.0, 666.0],
                order_size_sel: 3,
                order_size_keys: [0x70, 0x71, 0x72, 0x73, 0x74, 0x75], // F1..F6
                split_parts: 2,
                fixed_sell_sel: 1,
                fixed_sell_keys: [0, 0, 0, 0, 0, 0], // Empty shortcuts are not imported.
                fixed_sell_prices: [1.0, 5.0, 10.0, 25.0, 50.0, 100.0],
                shortcuts: Shortcuts(shortcuts),
            },
            strat_editor_chapters: String::new(),
            markets_table: MarketsTable {
                sort_col: 0,
                col_visible: [false; 41],
                col_pos: [0; 41],
            },
            main_buttons_index: 0,
            strat_expanded: [false; 11],
        },
        theme: ThemeBlock {
            current_style: 3, // Dark theme.
            sections: vec![
                IniSection {
                    name: "ColorsDark".into(),
                    entries: vec![
                        // 0xFF00FF00 → green, alpha FF (opaque), which is valid.
                        ("CandleGreen".into(), format!("{}", 0xFF00_FF00u32 as i64)),
                        // Alpha 0x80 is meaningful and must go to unsupported.
                        ("CandleRed".into(), format!("{}", 0x80FF_0000u32 as i64)),
                        ("graphBK".into(), "1973790".into()), // 0x001E1E1E
                        ("Unknown".into(), "123".into()),
                        ("BuyOrder".into(), "junk".into()), // Not a number.
                    ],
                },
                IniSection {
                    name: "ColorsLight".into(),
                    entries: vec![("graphBK".into(), "16777215".into())], // White.
                },
            ],
        },
        ini: IniBlock {
            sections: vec![IniSection {
                name: "Charts".into(),
                entries: vec![("Some".into(), "1".into())],
            }],
        },
    }
}

/// Reintroducing core-owned prose or choosing the Russian key leaks Cyrillic into a ready preview.
/// This renders a real plan, including invalid VK/color evidence and both theme sets.
#[test]
fn ready_plan_uses_the_active_locale() {
    let mut mb = mb_config();
    mb.ui.hotkeys.order_size_keys[1] = 0x006E;
    mb.ui.hotkeys.split_parts = 25;
    let (hotkeys, theme, orders) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let plan = build_plan(
        &mb,
        &PlanContext {
            hotkeys: &hotkeys,
            theme: &theme,
            orders: &orders,
            ui_theme_light: true,
        },
    );
    for locale in ["en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let mut rendered = Vec::new();
        for item in plan.local_items() {
            rendered.extend([
                caption(&item.label),
                preview_value(&item.current),
                preview_value(&item.new),
            ]);
        }
        for item in plan.unsupported.iter().chain(&plan.unsupported_hotkeys) {
            rendered.extend([caption(&item.name), reason(&item.reason)]);
        }
        rendered.extend(plan.warnings.iter().map(warning));
        for text in rendered {
            assert!(
                !text.chars().any(|c| ('\u{0400}'..='\u{04ff}').contains(&c)),
                "preview contains Cyrillic in {locale}"
            );
            assert!(
                !text.contains("import.preview."),
                "unresolved locale key in {locale}"
            );
            assert!(!text.contains("%{"), "unresolved parameter in {locale}");
        }
        let item = plan
            .local_items()
            .find(|item| item.id == "group.order_sizes")
            .unwrap();
        assert_eq!(
            caption(&item.label),
            if locale == "en" {
                "Order sizes B1-B6"
            } else {
                "Tamaños de orden B1-B6"
            }
        );
        assert_eq!(
            preview_value(&item.current),
            if locale == "en" {
                "depends on the selected group"
            } else {
                "depende del grupo seleccionado"
            }
        );
        assert_eq!(preview_value(&item.new), "111, 222, 333, 444, 555, 666");
        let unknown = plan
            .unsupported_hotkeys
            .iter()
            .find(|item| matches!(item.reason, ImportReason::UnknownKey { .. }))
            .unwrap();
        assert_eq!(
            reason(&unknown.reason),
            if locale == "en" {
                "unknown key (VK 0x6E)"
            } else {
                "tecla desconocida (VK 0x6E)"
            }
        );
    }
}

/// Wrong locale keys or dropped parameters would hide the exact refusal or imported cap.
/// Golden wording is independent of the dictionary lookup and includes native f32 precision.
#[test]
fn english_wording_preserves_warning_and_refusal_evidence() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(preview_value(&PreviewValue::ThemeLight(true)), "light");
    assert_eq!(preview_value(&PreviewValue::ThemeLight(false)), "dark");
    assert_eq!(
        caption(&PreviewCaption::FixedSellSelection),
        "Selected fixed sell slot"
    );
    assert_eq!(caption(&PreviewCaption::MouseGestures), "Mouse gestures");
    assert_eq!(
        caption(&PreviewCaption::ColorField {
            key: "graphBK".into(),
            light: false
        }),
        "graphBK (dark)"
    );
    assert_eq!(
        reason(&ImportReason::ColorAlpha { alpha: 0x80 }),
        "color carries alpha 0x80, which the target field does not support"
    );
    assert_eq!(
        reason(&ImportReason::InvalidColor {
            value: "junk".into()
        }),
        "value \"junk\" could not be parsed as a color"
    );
    assert_eq!(
        reason(&ImportReason::NoAction),
        "Terminal has no such action (no core command)"
    );
    assert_eq!(
        warning(&ImportWarning::SplitPartsClamped {
            parts: 25,
            max: 20,
            value: 20
        }),
        "SplitParts = 25 exceeds the maximum 20; importing 20"
    );
    assert_eq!(
        warning(&ImportWarning::HotkeysUnfilled),
        "The clipboard Hotkeys block is unfilled; sizes, percentages and selected slots are not transferred"
    );
    assert_eq!(
        warning(&ImportWarning::OrderSizeSelection { value: -2 }),
        "bNum = -2 is outside 0..=5; the selected preset is not transferred"
    );
    assert_eq!(
        warning(&ImportWarning::FixedSellSelection { value: 6 }),
        "sbNum = 6 is outside 0..=5; the selected fixed sell slot is not transferred"
    );
    assert_eq!(
        warning(&ImportWarning::InvalidFixedSellPrices {
            values: [33.3, -1.0, 0.0, 5.0, 10.0, 100.0]
        }),
        "SPrice (S1-S6) = 33.3, -1, 0, 5, 10, 100 contains a nonfinite or negative value; percentages are not transferred"
    );
    assert_eq!(
        warning(&ImportWarning::InvalidOrderSizes {
            values: [1.0, 0.0, -2.0, 3.0, 4.0, 5.0]
        }),
        "OSize (F1-F6) = 1, 0, -2, 3, 4, 5 contains a nonfinite, zero or negative value; sizes are not transferred"
    );
}

/// A missing translation must not silently fall back to English or leak a raw key.
/// Check every import-preview dictionary entry, including less common chart/action captions.
#[test]
fn every_preview_key_has_all_locales_and_matching_parameters() {
    let yaml = include_str!("../../../../../../locales/import.yml");
    let mut key = "";
    let mut translations: Vec<(&str, String)> = Vec::new();
    for line in yaml.lines().chain(["end:"]) {
        if !line.starts_with(' ') && line.ends_with(':') {
            if key.starts_with("import.preview.") {
                assert_eq!(
                    translations.len(),
                    3,
                    "every key requires three translations"
                );
                assert_eq!(
                    translations
                        .iter()
                        .map(|(locale, _)| *locale)
                        .collect::<Vec<_>>(),
                    ["ru", "en", "es"]
                );
                let mut expected = None;
                for (locale, text) in &translations {
                    if *locale != "ru" {
                        assert!(
                            !text.chars().any(|c| ('\u{0400}'..='\u{04ff}').contains(&c)),
                            "translated preview key contains Cyrillic"
                        );
                    }
                    let _locale = crate::test_locale::force(locale);
                    assert_eq!(rust_i18n::t!(key).as_ref(), text);
                    let mut params: Vec<_> = text
                        .split("%{")
                        .skip(1)
                        .map(|part| part.split('}').next().unwrap())
                        .collect();
                    params.sort();
                    if let Some(ref expected) = expected {
                        assert_eq!(&params, expected);
                    } else {
                        expected = Some(params);
                    }
                }
            }
            key = line.trim_end_matches(':');
            translations.clear();
        } else if key.starts_with("import.preview.")
            && let Some((locale, value)) = line.trim().split_once(": ")
        {
            let value: String = serde_json::from_str(value).unwrap();
            translations.push((locale, value));
        }
    }
}
