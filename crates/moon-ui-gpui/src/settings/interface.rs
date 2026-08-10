//! Interface tab for chart-theme editing, ported from egui's `settings/interface.rs`.
//!
//! It exposes chart, crosshair, order-book, panel, and candle colors plus numeric controls. Edits
//! update the draft for live preview; Save writes `theme.toml`. [`Iface`] owns editor controls.

use gpui::*;
use moon_ui::{
    IndexPath, MoonButtonSize, MoonColorPickerState, MoonMenuSize, MoonPalette, MoonSelect,
    MoonSelectEvent, MoonSelectItem, MoonSelectState, MoonSliderState, h_flex, v_flex,
};
use rust_i18n::t;

use super::{SettingsView, color_row, section, separator, slider_row};
use crate::{Backend, design};
use moon_core::config::{ChartTheme, TradeVolumeStyle, UiThemeMode};

const VOLUME_STYLE_LABELS: [(&str, TradeVolumeStyle); 3] = [
    ("iface.trade_volume_style.thin", TradeVolumeStyle::ThinBars),
    ("iface.trade_volume_style.wide", TradeVolumeStyle::WideBars),
    (
        "iface.trade_volume_style.smooth",
        TradeVolumeStyle::SmoothArea,
    ),
];

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
    trade_marker_scale: Entity<MoonSliderState>,
    trade_volume_style: Entity<MoonSelectState<TradeVolumeStyle>>,
    trade_volume_alpha: Entity<MoonSliderState>,
    trade_volume_height: Entity<MoonSliderState>,
    price_line: Entity<MoonColorPickerState>,
    mark_price_line: Entity<MoonColorPickerState>,
    price_line_width: Entity<MoonSliderState>,
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
    is_light: bool,
    get: fn(&ChartTheme) -> [u8; 3],
    set: fn(&mut ChartTheme, [u8; 3]),
) -> Entity<MoonColorPickerState> {
    let cur = {
        let b = backend.read(cx);
        get(b.preview.as_ref().unwrap_or(&b.config).theme.get(is_light))
    };
    super::draft_color(window, cx, cur, move |p, c| {
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
    is_light: bool,
    get: fn(&ChartTheme) -> f32,
    set: fn(&mut ChartTheme, f32),
    min: f32,
    max: f32,
    step: f32,
) -> Entity<MoonSliderState> {
    let cur = {
        let b = backend.read(cx);
        get(b.preview.as_ref().unwrap_or(&b.config).theme.get(is_light))
    };
    super::draft_slider(cx, min, max, step, cur, move |p, f, _bcx| {
        if get(p.theme.get(is_light)) != f {
            set(p.theme.get_mut(is_light), f);
            true
        } else {
            false
        }
    })
}

fn volume_style_field(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    is_light: bool,
) -> Entity<MoonSelectState<TradeVolumeStyle>> {
    let cur = {
        let b = backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .theme
            .get(is_light)
            .trade_volume_style
    };
    let items = VOLUME_STYLE_LABELS
        .iter()
        .map(|(key, style)| MoonSelectItem::new(*style, t!(*key).to_string()))
        .collect::<Vec<_>>();
    let idx = VOLUME_STYLE_LABELS
        .iter()
        .position(|(_, style)| *style == cur)
        .unwrap_or(1);
    let state = cx.new(|cx| MoonSelectState::new(items, Some(IndexPath::new(idx)), window, cx));
    cx.subscribe(
        &state,
        move |this, _e, ev: &MoonSelectEvent<TradeVolumeStyle>, cx| {
            if let MoonSelectEvent::Confirm(Some(style)) = ev {
                let style = *style;
                let changed = this.backend.update(cx, |b, bcx| {
                    let mut changed = false;
                    if let Some(p) = b.preview.as_mut() {
                        let theme = p.theme.get_mut(is_light);
                        if theme.trade_volume_style != style {
                            theme.trade_volume_style = style;
                            bcx.notify();
                            changed = true;
                        }
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            }
        },
    )
    .detach();
    state
}

fn select_row<T: Clone + PartialEq + 'static>(
    label: &str,
    state: &Entity<MoonSelectState<T>>,
    cx: &Context<SettingsView>,
) -> impl IntoElement {
    h_flex()
        .gap(px(10.0))
        .items_center()
        .child(div().w(px(145.0)).child(label.to_string()))
        .child(
            div().w(px(220.0)).child(
                MoonSelect::new(state)
                    .trigger_size(MoonButtonSize::Action)
                    .menu_width(design::font_w(cx, 220.0))
                    .menu_size(MoonMenuSize::Compact),
            ),
        )
}

/// Build the theme editor from the current draft, called by `SettingsView::new`.
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Iface {
    // Bind controls to the saved UI mode that was active when Settings opened, as in Lines. Saving
    // a mode change and reopening Settings builds controls for the other variant.
    let is_light = backend.read(cx).config.ui_theme_mode == UiThemeMode::Light;
    Iface {
        label_font_delta: num_field(
            backend,
            cx,
            is_light,
            |t| t.label_font_delta,
            |t, v| t.label_font_delta = v,
            -4.0,
            12.0,
            0.5,
        ),
        bg: color_field(backend, window, cx, is_light, |t| t.bg, |t, v| t.bg = v),
        grid: color_field(backend, window, cx, is_light, |t| t.grid, |t, v| t.grid = v),
        grid_alpha: num_field(
            backend,
            cx,
            is_light,
            |t| t.grid_alpha,
            |t, v| t.grid_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        cross: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.cross,
            |t, v| t.cross = v,
        ),
        cross_alpha: num_field(
            backend,
            cx,
            is_light,
            |t| t.cross_alpha,
            |t, v| t.cross_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        cross_thickness: num_field(
            backend,
            cx,
            is_light,
            |t| t.cross_thickness,
            |t, v| t.cross_thickness = v,
            0.5,
            4.0,
            0.1,
        ),
        candle_up: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.candle_up,
            |t, v| t.candle_up = v,
        ),
        candle_down: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.candle_down,
            |t, v| t.candle_down = v,
        ),
        candle_neutral: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.candle_neutral,
            |t, v| t.candle_neutral = v,
        ),
        candle_fill_alpha: num_field(
            backend,
            cx,
            is_light,
            |t| t.candle_fill_alpha,
            |t, v| t.candle_fill_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        trade_marker_scale: num_field(
            backend,
            cx,
            is_light,
            |t| t.trade_marker_scale,
            |t, v| t.trade_marker_scale = v,
            0.5,
            3.0,
            0.05,
        ),
        trade_volume_style: volume_style_field(backend, window, cx, is_light),
        trade_volume_alpha: num_field(
            backend,
            cx,
            is_light,
            |t| t.trade_volume_alpha,
            |t, v| t.trade_volume_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        trade_volume_height: num_field(
            backend,
            cx,
            is_light,
            |t| t.trade_volume_height,
            |t, v| t.trade_volume_height = v,
            0.02,
            0.45,
            0.01,
        ),
        price_line: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.price_line,
            |t, v| t.price_line = v,
        ),
        mark_price_line: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.mark_price_line,
            |t, v| t.mark_price_line = v,
        ),
        price_line_width: num_field(
            backend,
            cx,
            is_light,
            |t| t.price_line_width,
            |t, v| t.price_line_width = v,
            0.5,
            6.0,
            0.1,
        ),
        book_bg: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.book_bg,
            |t, v| t.book_bg = v,
        ),
        book_bg_ask: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.book_bg_ask,
            |t, v| t.book_bg_ask = v,
        ),
        book_bg_bid: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.book_bg_bid,
            |t, v| t.book_bg_bid = v,
        ),
        book_bid: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.book_bid,
            |t, v| t.book_bid = v,
        ),
        book_ask: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.book_ask,
            |t, v| t.book_ask = v,
        ),
        book_level_alpha: num_field(
            backend,
            cx,
            is_light,
            |t| t.book_level_alpha,
            |t, v| t.book_level_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        panel_bg: color_field(
            backend,
            window,
            cx,
            is_light,
            |t| t.panel_bg,
            |t, v| t.panel_bg = v,
        ),
    }
}

impl SettingsView {
    /// Render the Interface tab for the portable `theme.toml` chart theme variant.
    ///
    /// Sections cover chart-label font, chart background/grid, crosshair, candles, order book, and
    /// panels. Personal light/dark mode and UI font settings belong to General in `settings.toml`.
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
                cx,
            ))
            .child(separator(p, cx))
            // Chart background and grid.
            .child(section(&t!("iface.sec_chart"), p, cx))
            .child(color_row(&t!("iface.bg"), &i.bg, p, cx))
            .child(color_row(&t!("iface.grid"), &i.grid, p, cx))
            .child(slider_row(&t!("iface.grid_alpha"), &i.grid_alpha, cx))
            .child(separator(p, cx))
            // Chart crosshair.
            .child(section(&t!("iface.sec_cross"), p, cx))
            .child(color_row(&t!("iface.cross"), &i.cross, p, cx))
            .child(slider_row(&t!("iface.cross_alpha"), &i.cross_alpha, cx))
            .child(slider_row(
                &t!("iface.cross_thickness"),
                &i.cross_thickness,
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
                cx,
            ))
            .child(separator(p, cx))
            // Trade crosses and price-line appearance.
            .child(section(&t!("iface.sec_trade_lines"), p, cx))
            .child(slider_row(
                &t!("iface.trade_marker_scale"),
                &i.trade_marker_scale,
                cx,
            ))
            .child(select_row(
                &t!("iface.trade_volume_style"),
                &i.trade_volume_style,
                cx,
            ))
            .child(slider_row(
                &t!("iface.trade_volume_alpha"),
                &i.trade_volume_alpha,
                cx,
            ))
            .child(slider_row(
                &t!("iface.trade_volume_height"),
                &i.trade_volume_height,
                cx,
            ))
            .child(color_row(&t!("iface.price_line"), &i.price_line, p, cx))
            .child(color_row(
                &t!("iface.mark_price_line"),
                &i.mark_price_line,
                p,
                cx,
            ))
            .child(slider_row(
                &t!("iface.price_line_width"),
                &i.price_line_width,
                cx,
            ))
            .child(separator(p, cx))
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
