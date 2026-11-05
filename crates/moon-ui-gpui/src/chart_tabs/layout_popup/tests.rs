//! Layout of the chart-tab layout popup: where the price-axis selector sits and that it fits.

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
};
use gpui::{InteractiveElement as _, Window, div, px};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonInputState, MoonPalette, MoonTheme};
use rust_i18n::t;

use super::{GridControls, content_width, render_layout_popup};
use crate::persistence::chart_persist::{PriceAxisPos, StackLayoutMode, StackOrientation};

/// Inert host that draws the production popup content at the width every host declares.
struct LayoutPopupProbe {
    fit: Entity<MoonInputState>,
    scroll: Entity<MoonInputState>,
}

impl Render for LayoutPopupProbe {
    /// Render the real popup with no-op handlers and no detect frame.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let popup = render_layout_popup(
            "lp",
            StackLayoutMode::Fit,
            StackOrientation::Vertical,
            None,
            &self.fit,
            &self.scroll,
            false,
            PriceAxisPos::Right,
            true,
            true,
            true,
            GridControls {
                shown: false,
                columns: 1,
                exact: false,
                min_slot_input: None,
                on_pick_columns: Box::new(|_, _| {}),
                on_toggle_exact: Box::new(|_, _| {}),
            },
            None,
            p,
            cx,
            |_, _| {},
            String::new(),
            |_| {},
            |_, _| {},
            |_| {},
            |_, _| {},
            |_, _| {},
            |_, _| {},
            |_, _| {},
            |_, _, _| {},
        );
        div()
            .debug_selector(|| "lp-probe".into())
            .w(content_width(cx))
            .child(popup)
    }
}

/// Moving the selector back out of the display frame, below the time axis, or letting its caption
/// or segments overflow the declared popup width fails here in every locale.
#[gpui::test]
fn price_axis_row_opens_the_display_frame_in_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for zoom in [1.0, 1.25] {
            let window = cx.open_window(gpui::size(px(600.0), px(900.0)), |window, cx| {
                LayoutPopupProbe {
                    fit: cx.new(|cx| MoonInputState::new(window, cx)),
                    scroll: cx.new(|cx| MoonInputState::new(window, cx)),
                }
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            visual.update(|window, cx| window.set_content_zoom(zoom, cx));
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let at = format!("{} zoom {zoom}", language.code());
            let get = |visual: &mut gpui::VisualTestContext, sel: &'static str| {
                visual
                    .debug_bounds(sel)
                    .unwrap_or_else(|| panic!("{sel} not laid out ({at})"))
            };
            let probe = get(&mut visual, "lp-probe");
            let body = get(&mut visual, "lp-display-body");
            let row = get(&mut visual, "lp-price-axis-pos-row");
            let caption = get(&mut visual, "lp-price-axis-pos-caption");
            let seg = get(&mut visual, "lp-price-axis-pos-seg");
            let time = get(&mut visual, "lp-time-axis-row");

            assert!(
                row.origin.x >= body.origin.x
                    && row.right() <= body.right()
                    && row.origin.y >= body.origin.y
                    && row.bottom() <= body.bottom(),
                "price-axis row is outside the display frame ({at})"
            );
            assert!(
                (f32::from(row.origin.y - body.origin.y)).abs() < 0.5,
                "price-axis row is not the display frame's first row ({at})"
            );
            assert!(
                row.bottom() <= time.origin.y,
                "price-axis row is not above the time-axis checkbox ({at})"
            );
            assert!(
                seg.right() <= probe.right() && seg.right() <= row.right(),
                "price-axis segments overflow the popup width ({at})"
            );
            assert!(
                caption.right() <= seg.origin.x,
                "caption overlaps the segments ({at})"
            );
            let need = visual.update(|_, cx| {
                crate::design::ui_caption_text_width(cx, &t!("chart.layout.price_axis"), 400.0)
            });
            assert!(
                f32::from(caption.size.width) + 0.5 >= need,
                "price-axis caption clipped: {} < {need} ({at})",
                f32::from(caption.size.width)
            );
            assert!(
                f32::from(row.size.height) < 40.0,
                "price-axis row wrapped ({at})"
            );
        }
    }
}
