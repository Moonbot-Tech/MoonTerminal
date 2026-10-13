//! Root-owned menu layout regressions using the application's production opening functions.

use gpui::{App, AppContext as _, Context, IntoElement, Point, Render, Styled as _, Window, px};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonMenuItem, MoonMenuSize, MoonTheme, Root};
use rust_i18n::t;

/// Inert content underneath the real Root-owned context-menu overlay.
struct MenuHost;

impl Render for MenuHost {
    /// Render an empty surface; Root paints the menu above it.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div().size_full()
    }
}

/// Open the production menu route and compare its row bounds against independent full-label
/// font measurements. Reverting the route to a fixed width or reducing its cap clips long labels.
pub(crate) fn assert_menu_fits(
    cx: &mut gpui::TestAppContext,
    id: &str,
    labels: Vec<String>,
    open: impl FnOnce(Vec<MoonMenuItem>, Point<gpui::Pixels>, &mut Window, &mut App) + 'static,
) {
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| MenuHost);
        Root::new(view, window, cx).bordered(false)
    });
    visual.simulate_resize(gpui::size(px(720.0), px(480.0)));
    let rows = labels.iter().cloned().map(MoonMenuItem::new).collect();
    visual.update(move |window, app| {
        let viewport = window.viewport_size();
        open(
            rows,
            gpui::point(viewport.width - px(2.0), viewport.height - px(2.0)),
            window,
            app,
        );
    });
    let menu_selector = Box::leak(format!("{id}:menu:popup").into_boxed_str());
    let menu = (0..8)
        .find_map(|_| {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            visual.debug_bounds(menu_selector)
        })
        .expect("Root painted the context menu");
    let (text_widths, row_chrome, outer_chrome) = visual.update(|_, app| {
        let tokens = MoonTheme::active_tokens(app);
        let MoonMenuSize::Tier(tier) = MoonMenuSize::from_theme(&tokens) else {
            panic!("application menus use the density tier");
        };
        let metrics = tier.control_metrics();
        // Measure unmodified labels independently of the menu's fit/ellipsis implementation.
        // Item rows reserve the check column and two gaps even when unchecked.
        let font = gpui::Font {
            family: tokens.font_family(true),
            weight: gpui::FontWeight(400.0),
            ..Default::default()
        };
        let font_id = app.text_system().resolve_font(&font);
        let widths = labels
            .iter()
            .map(|label| {
                label
                    .chars()
                    .map(|ch| {
                        f32::from(app.text_system().layout_width(
                            font_id,
                            px(tokens.ui(metrics.font_size)),
                            ch,
                        ))
                    })
                    .sum::<f32>()
            })
            .collect::<Vec<_>>();
        (
            widths,
            tokens.ui(2.0 * metrics.pad_x + 12.0 + 2.0 * metrics.gap),
            tokens.ui(8.0) + 2.0,
        )
    });
    let viewport = visual.update(|window, _| window.viewport_size());
    assert!(menu.origin.x >= px(0.0) && menu.right() <= viewport.width);
    assert!(menu.origin.y >= px(0.0) && menu.bottom() <= viewport.height);
    for (index, full_text_width) in text_widths.into_iter().enumerate() {
        let selector = Box::leak(format!("{id}:menu:popup:item:{index}").into_boxed_str());
        let row = visual.debug_bounds(selector).expect("menu row painted");
        let at = format!("{} row {index}", &*rust_i18n::locale());
        assert!(
            row.origin.x >= menu.origin.x && row.right() <= menu.right(),
            "{at}"
        );
        assert!(
            row.origin.y >= menu.origin.y && row.bottom() <= menu.bottom(),
            "{at}"
        );
        assert!(
            f32::from(row.size.width) + 0.5 >= full_text_width + row_chrome,
            "full text would truncate at {at}: {row:?}, text {full_text_width}, chrome {row_chrome}"
        );
        assert!(
            f32::from(menu.size.width) + 0.5 >= full_text_width + row_chrome + outer_chrome,
            "menu narrower than its full row at {at}"
        );
    }
}

/// A fixed 240px tab menu clips the longest counter switches in translated locales.
#[gpui::test]
fn tab_menu_labels_fit_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for mode in [
        UiThemeMode::Dark,
        UiThemeMode::Light,
        UiThemeMode::Graphite,
        UiThemeMode::LightExperimental,
        UiThemeMode::DarkExperimental,
    ] {
        cx.update(|app| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                app,
            );
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            let labels = vec![
                t!("dock.tab_menu.counters").to_string(),
                t!("dock.tab_menu.no_colour_split").to_string(),
            ];
            assert_menu_fits(cx, "tab-menu-report", labels, |items, at, window, app| {
                super::open_items("report", at, items, window, app);
            });
        }
    }
}
