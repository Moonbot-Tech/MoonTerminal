//! Interface tab for chart-theme editing, ported from egui's `settings/interface.rs`.
//!
//! It exposes chart, crosshair, order-book, panel, and candle colors plus numeric controls. Edits
//! update the draft for live preview; Save writes `theme.toml`. [`Iface`] owns editor controls.
//!
//! The trade-mark sizes and the bottom-volume band are NOT here. They describe a chart tab rather
//! than a colour scheme, so they live on `ChartGraphicsCfg` and are edited from the chart's palette
//! popup (`crate::chart_tabs::graphics_popup`).

use gpui::*;
use moon_ui::{MoonColorPickerState, MoonPalette, MoonSliderState, v_flex};
use rust_i18n::t;

use super::{SettingsView, color_row, section, separator, slider_row};
use crate::Backend;
use moon_core::{
    config::{ChartTheme, UiThemeMode},
    util::fmt,
};

/// Theme editor state with one retained control entity per field.
pub(super) struct Iface {
    label_font_delta: Entity<MoonSliderState>,
    bg: Entity<MoonColorPickerState>,
    grid: Entity<MoonColorPickerState>,
    grid_alpha: Entity<MoonSliderState>,
    cross: Entity<MoonColorPickerState>,
    cross_alpha: Entity<MoonSliderState>,
    cross_thickness: Entity<MoonSliderState>,
    candle_up: Entity<MoonColorPickerState>,
    candle_down: Entity<MoonColorPickerState>,
    candle_neutral: Entity<MoonColorPickerState>,
    candle_fill_alpha: Entity<MoonSliderState>,
    price_line: Entity<MoonColorPickerState>,
    price_line_alpha: Entity<MoonSliderState>,
    mark_line: Entity<MoonColorPickerState>,
    mark_line_alpha: Entity<MoonSliderState>,
    price_line_px: Entity<MoonSliderState>,
    book_bg: Entity<MoonColorPickerState>,
    book_bg_ask: Entity<MoonColorPickerState>,
    book_bg_bid: Entity<MoonColorPickerState>,
    book_bid: Entity<MoonColorPickerState>,
    book_ask: Entity<MoonColorPickerState>,
    book_level_alpha: Entity<MoonSliderState>,
    panel_bg: Entity<MoonColorPickerState>,
}

/// Bind a color picker to one field of the UI mode selected when Settings opened.
///
/// Initialize from the current config or draft and write changes to the matching
/// `Backend.preview.theme` variant for live application and backend notification, as in Lines.
fn color_field(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    get: fn(&ChartTheme) -> [u8; 3],
    set: fn(&mut ChartTheme, [u8; 3]),
) -> Entity<MoonColorPickerState> {
    let cur = {
        let b = backend.read(cx);
        let is_light = b.preview.as_ref().unwrap_or(&b.config).ui_theme_mode == UiThemeMode::Light;
        get(b.preview.as_ref().unwrap_or(&b.config).theme.get(is_light))
    };
    super::draft_color(window, cx, cur, move |p, c| {
        let is_light = p.ui_theme_mode == UiThemeMode::Light;
        if get(p.theme.get(is_light)) != c {
            set(p.theme.get_mut(is_light), c);
            true
        } else {
            false
        }
    })
}

/// Bind an `f32` slider to a field of the selected UI-mode theme for live preview.
#[allow(clippy::too_many_arguments)]
fn num_field(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
    get: fn(&ChartTheme) -> f32,
    set: fn(&mut ChartTheme, f32),
    min: f32,
    max: f32,
    step: f32,
) -> Entity<MoonSliderState> {
    let cur = {
        let b = backend.read(cx);
        let is_light = b.preview.as_ref().unwrap_or(&b.config).ui_theme_mode == UiThemeMode::Light;
        get(b.preview.as_ref().unwrap_or(&b.config).theme.get(is_light))
    };
    super::draft_slider(cx, min, max, step, cur, move |p, f, _bcx| {
        let is_light = p.ui_theme_mode == UiThemeMode::Light;
        if get(p.theme.get(is_light)) != f {
            set(p.theme.get_mut(is_light), f);
            true
        } else {
            false
        }
    })
}

/// Build the theme editor from the current draft, called by `SettingsView::new`.
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Iface {
    // Bind controls to the saved UI mode that was active when Settings opened, as in Lines. Saving
    // a mode change and reopening Settings builds controls for the other variant.
    Iface {
        label_font_delta: num_field(
            backend,
            cx,
            |t| t.label_font_delta,
            |t, v| t.label_font_delta = v,
            -4.0,
            12.0,
            0.5,
        ),
        bg: color_field(backend, window, cx, |t| t.bg, |t, v| t.bg = v),
        grid: color_field(backend, window, cx, |t| t.grid, |t, v| t.grid = v),
        grid_alpha: num_field(
            backend,
            cx,
            |t| t.grid_alpha,
            |t, v| t.grid_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        cross: color_field(backend, window, cx, |t| t.cross, |t, v| t.cross = v),
        cross_alpha: num_field(
            backend,
            cx,
            |t| t.cross_alpha,
            |t, v| t.cross_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        cross_thickness: num_field(
            backend,
            cx,
            |t| t.cross_thickness,
            |t, v| t.cross_thickness = v,
            0.5,
            4.0,
            0.1,
        ),
        candle_up: color_field(backend, window, cx, |t| t.candle_up, |t, v| t.candle_up = v),
        candle_down: color_field(
            backend,
            window,
            cx,
            |t| t.candle_down,
            |t, v| t.candle_down = v,
        ),
        candle_neutral: color_field(
            backend,
            window,
            cx,
            |t| t.candle_neutral,
            |t, v| t.candle_neutral = v,
        ),
        candle_fill_alpha: num_field(
            backend,
            cx,
            |t| t.candle_fill_alpha,
            |t, v| t.candle_fill_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        price_line: color_field(
            backend,
            window,
            cx,
            |t| t.price_line,
            |t, v| t.price_line = v,
        ),
        price_line_alpha: num_field(
            backend,
            cx,
            |t| t.price_line_alpha,
            |t, v| t.price_line_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        mark_line: color_field(backend, window, cx, |t| t.mark_line, |t, v| t.mark_line = v),
        mark_line_alpha: num_field(
            backend,
            cx,
            |t| t.mark_line_alpha,
            |t, v| t.mark_line_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        // Logical pixels: the device scale is applied where the uniform is built, never here.
        price_line_px: num_field(
            backend,
            cx,
            |t| t.price_line_px,
            |t, v| t.price_line_px = v,
            0.5,
            6.0,
            0.1,
        ),
        book_bg: color_field(backend, window, cx, |t| t.book_bg, |t, v| t.book_bg = v),
        book_bg_ask: color_field(
            backend,
            window,
            cx,
            |t| t.book_bg_ask,
            |t, v| t.book_bg_ask = v,
        ),
        book_bg_bid: color_field(
            backend,
            window,
            cx,
            |t| t.book_bg_bid,
            |t, v| t.book_bg_bid = v,
        ),
        book_bid: color_field(backend, window, cx, |t| t.book_bid, |t, v| t.book_bid = v),
        book_ask: color_field(backend, window, cx, |t| t.book_ask, |t, v| t.book_ask = v),
        book_level_alpha: num_field(
            backend,
            cx,
            |t| t.book_level_alpha,
            |t, v| t.book_level_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        panel_bg: color_field(backend, window, cx, |t| t.panel_bg, |t, v| t.panel_bg = v),
    }
}

impl SettingsView {
    /// Render the Interface tab for the portable `theme.toml` chart theme variant.
    ///
    /// Sections cover chart-label font, chart background/grid, crosshair, candles, order book, and
    /// panels. Personal light/dark mode and UI font settings belong to General in `settings.toml`.
    ///
    /// Args:
    ///     cx: Settings context that supplies the active palette and display scale.
    ///
    /// Returns:
    ///     The assembled Interface-tab content with formatted slider endpoints and values.
    pub(super) fn interface_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let i = &self.iface;
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .gap_1()
            // Chart-label font field from the portable theme.
            .child(section(&t!("iface.sec_font"), p, cx))
            .child(slider_row(
                &t!("iface.label_font_delta"),
                &i.label_font_delta,
                -4.0..=12.0,
                |v| {
                    fmt::round_to(v as f64, 1).map_or_else(
                        || "+0.0 px".to_string(),
                        |rounded| format!("{rounded:+.1} px"),
                    )
                },
                cx,
            ))
            .child(separator(p, cx))
            // Chart background and grid.
            .child(section(&t!("iface.sec_chart"), p, cx))
            .child(color_row(&t!("iface.bg"), &i.bg, p, cx))
            .child(color_row(&t!("iface.grid"), &i.grid, p, cx))
            .child(slider_row(
                &t!("iface.grid_alpha"),
                &i.grid_alpha,
                0.0..=1.0,
                |v| {
                    fmt::pct((v * 100.0) as f64, 0)
                        .map_or_else(|| "0%".to_string(), |(text, _)| text)
                },
                cx,
            ))
            .child(separator(p, cx))
            // Chart crosshair.
            .child(section(&t!("iface.sec_cross"), p, cx))
            .child(color_row(&t!("iface.cross"), &i.cross, p, cx))
            .child(slider_row(
                &t!("iface.cross_alpha"),
                &i.cross_alpha,
                0.0..=1.0,
                |v| {
                    fmt::pct((v * 100.0) as f64, 0)
                        .map_or_else(|| "0%".to_string(), |(text, _)| text)
                },
                cx,
            ))
            .child(slider_row(
                &t!("iface.cross_thickness"),
                &i.cross_thickness,
                0.5..=4.0,
                |v| format!("{} px", fmt::compact(v as f64, 1)),
                cx,
            ))
            .child(separator(p, cx))
            // Per-theme candle colors shared by all windows; visibility and mode live in the candle popup.
            .child(section(&t!("iface.sec_candles"), p, cx))
            .child(color_row(&t!("iface.candle_up"), &i.candle_up, p, cx))
            .child(color_row(&t!("iface.candle_down"), &i.candle_down, p, cx))
            .child(color_row(
                &t!("iface.candle_neutral"),
                &i.candle_neutral,
                p,
                cx,
            ))
            .child(slider_row(
                &t!("iface.candle_fill_alpha"),
                &i.candle_fill_alpha,
                0.0..=1.0,
                |v| {
                    fmt::pct((v * 100.0) as f64, 0)
                        .map_or_else(|| "0%".to_string(), |(text, _)| text)
                },
                cx,
            ))
            .child(separator(p, cx))
            // Price lines. Colour and width used to be per-backend shader literals.
            .child(section(&t!("iface.sec_price_lines"), p, cx))
            .child(color_row(&t!("iface.price_line"), &i.price_line, p, cx))
            .child(slider_row(
                &t!("iface.price_line_alpha"),
                &i.price_line_alpha,
                0.0..=1.0,
                |v| {
                    fmt::pct((v * 100.0) as f64, 0)
                        .map_or_else(|| "0%".to_string(), |(text, _)| text)
                },
                cx,
            ))
            .child(color_row(&t!("iface.mark_line"), &i.mark_line, p, cx))
            .child(slider_row(
                &t!("iface.mark_line_alpha"),
                &i.mark_line_alpha,
                0.0..=1.0,
                |v| {
                    fmt::pct((v * 100.0) as f64, 0)
                        .map_or_else(|| "0%".to_string(), |(text, _)| text)
                },
                cx,
            ))
            .child(slider_row(
                &t!("iface.price_line_px"),
                &i.price_line_px,
                0.5..=6.0,
                |v| format!("{} px", fmt::compact(v as f64, 1)),
                cx,
            ))
            .child(separator(p, cx))
            // The trade-mark and bottom-volume groups that used to sit here are now per chart TAB,
            // in the chart's palette popup (`chart_tabs::graphics_popup`).
            // Order book.
            .child(section(&t!("iface.sec_book"), p, cx))
            .child(color_row(&t!("iface.book_bg"), &i.book_bg, p, cx))
            .child(color_row(&t!("iface.book_bg_ask"), &i.book_bg_ask, p, cx))
            .child(color_row(&t!("iface.book_bg_bid"), &i.book_bg_bid, p, cx))
            .child(color_row(&t!("iface.book_bid"), &i.book_bid, p, cx))
            .child(color_row(&t!("iface.book_ask"), &i.book_ask, p, cx))
            .child(slider_row(
                &t!("iface.book_level_alpha"),
                &i.book_level_alpha,
                0.0..=1.0,
                |v| {
                    fmt::pct((v * 100.0) as f64, 0)
                        .map_or_else(|| "0%".to_string(), |(text, _)| text)
                },
                cx,
            ))
            .child(separator(p, cx))
            // Panels.
            .child(section(&t!("iface.sec_panels"), p, cx))
            .child(color_row(&t!("iface.panel_bg"), &i.panel_bg, p, cx))
            .child(
                div()
                    .mt_2()
                    .text_color(rgb(p.text_soft))
                    .child(t!("iface.hint").to_string()),
            )
    }
}
