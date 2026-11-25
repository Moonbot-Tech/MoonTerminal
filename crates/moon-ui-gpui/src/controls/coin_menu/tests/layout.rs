//! Headless geometry of the localized Report-row menu.

use gpui::{
    App, AppContext as _, Context, Font, FontWeight, IntoElement, Render, Styled as _, Window, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonMenuItem, MoonMenuSize, MoonTheme, Root};
use rust_i18n::t;

use super::super::{MENU_MAX_WIDTH, open_menu_items, plan, state};
use plan::{CoinMenuOrigin, MenuFacts, ReportFacts, Slot};

/// Inert view underneath the same Root-owned popup used by the Report.
struct Host;

impl Render for Host {
    /// Give the popup a full-size host without setting up a live session.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div().size_full()
    }
}

/// Source text and trailing parts for one action row of a fully populated Report menu.
struct Row {
    label: String,
    right: Option<String>,
    caret: bool,
    emphasized: bool,
}

/// Build the localized paint fixture in the production planner's order, without action handlers.
fn report_rows() -> Vec<Option<Row>> {
    let coin = "XALPHA";
    let copy = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    };
    let hint =
        |binding| gpui_component::kbd::Kbd::format(&gpui::Keystroke::parse(binding).unwrap());
    plan::plan(&MenuFacts {
        surface: CoinMenuOrigin::OrderTable,
        has_market: true,
        coin_shown: true,
        strategy: Some(1),
        order: None,
        split_n: 3,
        report: Some(ReportFacts {
            trade_window: (),
            trade_log: (),
            filter_coin: Some(()),
        }),
        screener_copy: false,
        blacklist: true,
    })
    .into_iter()
    .map(|slot| {
        let (label, right) = match slot {
            Slot::TradeWindow(()) => (t!("trade_window.open"), Some(hint("enter"))),
            Slot::ShowOnChart => (t!("coin_menu.view_trades"), Some(hint("g"))),
            Slot::TradeLog(()) => (t!("report.trade_log.open"), Some(hint("l"))),
            Slot::FilterHere(()) => (t!("coin_menu.coin_filter", coin = coin), Some(hint("f"))),
            Slot::CoinReport => (
                t!("coin_menu.coin_report", coin = coin),
                Some(t!("coin_menu.new_report").to_string()),
            ),
            Slot::Compare => (t!("coin_menu.compare"), None),
            Slot::CopyTicker => (t!("coin_menu.copy_coin"), Some(hint(copy))),
            Slot::StrategyGoto(_) => (t!("coin_menu.strategy_goto"), None),
            Slot::StrategyReport(_) => (
                t!("coin_menu.strategy_report"),
                Some(t!("coin_menu.new_report").to_string()),
            ),
            Slot::Blacklist => (
                t!("coin_menu.blacklist"),
                state::bl_right_label(&state::BlState {
                    listed: 1,
                    ban: Some((7_200_000, false)),
                }),
            ),
            Slot::Sep => return None,
            _ => panic!("unexpected Report row {slot:?}"),
        };
        Some(Row {
            label: label.to_string(),
            right,
            caret: matches!(slot, Slot::Blacklist),
            emphasized: slot.emphasized(),
        })
    })
    .collect()
}

/// Convert the inert row fixture into the actual MoonUI item type.
fn menu_item(row: Option<&Row>) -> MoonMenuItem {
    let Some(row) = row else {
        return MoonMenuItem::separator();
    };
    let mut item = MoonMenuItem::new(row.label.clone()).emphasized(row.emphasized);
    if let Some(right) = &row.right {
        item = item.right_label(right.clone());
    }
    if row.caret {
        item = item.submenu([MoonMenuItem::new(t!("coin_menu.where_listed").to_string())]);
    }
    item
}

/// Measure complete source strings independently of the fitter and compute the content budget.
fn source_widths(row: &Row, app: &App) -> (f32, f32, f32) {
    let tokens = MoonTheme::active_tokens(app);
    let MoonMenuSize::Tier(tier) = MoonMenuSize::from_theme(&tokens) else {
        panic!("Report menu uses the active tier");
    };
    let metrics = tier
        .nearest(&MoonMenuSize::SUPPORTED_TIERS)
        .control_metrics();
    let measure = |text: &str, size, weight| {
        let font = Font {
            family: tokens.font_family(true),
            weight: FontWeight(weight),
            ..Default::default()
        };
        let font = app.text_system().resolve_font(&font);
        text.chars()
            .map(|ch| {
                f32::from(
                    app.text_system()
                        .layout_width(font, px(tokens.ui(size)), ch),
                )
            })
            .sum::<f32>()
    };
    let label = measure(
        &row.label,
        metrics.font_size,
        if row.emphasized { 700.0 } else { 600.0 },
    );
    let right = row
        .right
        .as_deref()
        .map_or(0.0, |t| measure(t, metrics.font_size - 0.5, 400.0));
    let caret = if row.caret {
        measure("›", metrics.font_size, 600.0)
    } else {
        0.0
    };
    let gaps = 2.0 + f32::from(u8::from(row.right.is_some())) + f32::from(u8::from(row.caret));
    let chrome = tokens.ui(2.0 * metrics.pad_x + 12.0 + gaps * metrics.gap) + caret;
    (label, right, chrome)
}

/// A width clamp or loss of the combined status/caret fit would clip the translated Report menu.
#[gpui::test]
fn fitted_menu_no_truncation_all_locales(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        moon_ui::init(app);
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            app,
        );
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let rows = report_rows();
        let items = rows.iter().map(|row| menu_item(row.as_ref())).collect();
        let (_, visual) = cx.add_window_view(|window, app| {
            let host = app.new(|_| Host);
            Root::new(host, window, app).bordered(false)
        });
        visual.simulate_resize(gpui::size(px(960.0), px(640.0)));
        visual.update(move |window, app| {
            open_menu_items(
                "coin-context-menu",
                gpui::point(px(30.0), px(30.0)),
                items,
                window,
                app,
            );
        });
        let popup = (0..8)
            .find_map(|_| {
                visual.update(|window, _| window.refresh());
                visual.run_until_parked();
                visual.debug_bounds("coin-context-menu:menu:popup")
            })
            .expect("Report coin popup must paint");
        assert!(popup.size.width < px(MENU_MAX_WIDTH), "{}", language.code());

        for (index, row) in rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| row.as_ref().map(|row| (i, row)))
        {
            let selector = format!("coin-context-menu:menu:popup:item:{index}");
            let bounds = visual
                .debug_bounds(Box::leak(selector.clone().into_boxed_str()))
                .expect("action row");
            let (label_width, right_width, chrome) =
                visual.update(|_, app| source_widths(row, app));
            let at = format!("{} {}", language.code(), row.label);
            assert!(
                bounds.left() >= popup.left() && bounds.right() <= popup.right(),
                "{at}"
            );
            assert!(
                f32::from(bounds.size.width) + 1.0 >= label_width + right_width + chrome,
                "full label and trailing text exceed row budget ({at})"
            );
            // MoonUI exposes child selectors only for rows with both trailing elements.
            if row.caret {
                let mut part = |name| {
                    visual
                        .debug_bounds(Box::leak(format!("{selector}:{name}").into_boxed_str()))
                        .expect("submenu row child")
                };
                let label = part("label");
                let right = part("right-label");
                let caret = part("caret");
                assert!(
                    label.right() < right.left() && right.right() < caret.left(),
                    "{at}"
                );
                assert!(
                    (f32::from(label.size.width) - label_width).abs() <= 1.0,
                    "{at}"
                );
                assert!(
                    (f32::from(right.size.width) - right_width).abs() <= 1.0,
                    "{at}"
                );
            }
        }
    }
}
