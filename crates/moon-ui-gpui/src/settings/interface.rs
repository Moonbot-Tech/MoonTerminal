//! Interface tab for chart-theme editing, ported from egui's `settings/interface.rs`.
//!
//! It exposes chart, crosshair, trade, order-book, panel, and candle colors plus numeric controls. Edits
//! update the draft for live preview; Save writes `theme.toml`. [`Iface`] owns editor controls.
//!
//! The trade-mark sizes and the bottom-volume band are NOT here. They describe a chart tab rather
//! than a colour scheme, so they live on `ChartGraphicsCfg` and are edited from the chart's palette
//! popup (`crate::chart_tabs::graphics_popup`).

use gpui::*;
use moon_ui::{
    MoonButton, MoonColorPickerEvent, MoonColorPickerState, MoonPalette, MoonSliderState, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::{SettingsView, color_row, section, separator, slider_row};
use crate::{Backend, design};
use moon_core::{config::ChartTheme, util::fmt};

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
    tick_buy: Entity<MoonColorPickerState>,
    tick_sell: Entity<MoonColorPickerState>,
    tick_liq: Entity<MoonColorPickerState>,
    book_bg: Entity<MoonColorPickerState>,
    book_bg_ask: Entity<MoonColorPickerState>,
    book_bg_bid: Entity<MoonColorPickerState>,
    book_bid: Entity<MoonColorPickerState>,
    book_ask: Entity<MoonColorPickerState>,
    book_level_bid: Entity<MoonColorPickerState>,
    book_level_ask: Entity<MoonColorPickerState>,
    book_level_alpha: Entity<MoonSliderState>,
    book_level_width: Entity<MoonSliderState>,
    panel_bg: Entity<MoonColorPickerState>,
}

/// Bind a colour picker to one field of the colour set active when its editor was built.
///
/// Initialize from the current config or draft and write changes to the matching
/// `Backend.preview.theme` entry for live application and backend notification, as in Lines.
fn color_field(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    get: fn(&ChartTheme) -> [u8; 3],
    set: fn(&mut ChartTheme, [u8; 3]),
) -> Entity<MoonColorPickerState> {
    let cur = {
        let b = backend.read(cx);
        let is_light = b
            .preview
            .as_ref()
            .unwrap_or(&b.config)
            .ui_theme_mode
            .is_light();
        get(b.preview.as_ref().unwrap_or(&b.config).theme.get(is_light))
    };
    super::draft_color(window, cx, cur, move |p, c| {
        let is_light = p.ui_theme_mode.is_light();
        if get(p.theme.get(is_light)) != c {
            set(p.theme.get_mut(is_light), c);
            true
        } else {
            false
        }
    })
}

/// Bind a level picker without turning an automatic colour into a rounded stored default.
fn book_level_field(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    bid: bool,
) -> Entity<MoonColorPickerState> {
    let init = {
        let b = backend.read(cx);
        let p = b.preview.as_ref().unwrap_or(&b.config);
        let rgba = p
            .theme
            .get(p.ui_theme_mode.is_light())
            .resolved_book_level(bid);
        [rgba[0], rgba[1], rgba[2]].map(|v| (v * 255.0).round() as u8)
    };
    super::draft_color(window, cx, init, move |p, color| {
        let theme = p.theme.get_mut(p.ui_theme_mode.is_light());
        let slot = if bid {
            &mut theme.book_level_bid
        } else {
            &mut theme.book_level_ask
        };
        let changed = *slot != Some(color);
        *slot = Some(color);
        changed
    })
}

/// Bind a wall picker and refresh its automatic level swatch after the draft changes.
fn book_wall_field(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    bid: bool,
) -> Entity<MoonColorPickerState> {
    let init = {
        let b = backend.read(cx);
        let p = b.preview.as_ref().unwrap_or(&b.config);
        let theme = p.theme.get(p.ui_theme_mode.is_light());
        if bid { theme.book_bid } else { theme.book_ask }
    };
    let st = crate::controls::color_picker::shared_color_state(init, window, cx);
    cx.subscribe_in(&st, window, move |this, _, event, window, cx| {
        let MoonColorPickerEvent::Change(color) = event else {
            return;
        };
        let refresh = this.backend.update(cx, |b, bcx| {
            let Some(p) = b.preview.as_mut() else {
                return false;
            };
            let theme = p.theme.get_mut(p.ui_theme_mode.is_light());
            let color = super::common::hsla_u8(*color);
            let (wall, level) = if bid {
                (&mut theme.book_bid, theme.book_level_bid)
            } else {
                (&mut theme.book_ask, theme.book_level_ask)
            };
            if *wall == color {
                return false;
            }
            *wall = color;
            bcx.notify();
            level.is_none()
        });
        if refresh {
            let state = book_level_field(&this.backend, window, cx, bid);
            if bid {
                this.iface.book_level_bid = state;
            } else {
                this.iface.book_level_ask = state;
            }
            cx.notify();
        }
    })
    .detach();
    st
}

/// Bind an `f32` slider to a field of the active colour-set theme for live preview.
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
        let is_light = b
            .preview
            .as_ref()
            .unwrap_or(&b.config)
            .ui_theme_mode
            .is_light();
        get(b.preview.as_ref().unwrap_or(&b.config).theme.get(is_light))
    };
    super::draft_slider(cx, min, max, step, cur, move |p, f, _bcx| {
        let is_light = p.ui_theme_mode.is_light();
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
    // Bind controls to the draft's active colour set. Changing the mode rebuilds these controls so
    // their retained widget state cannot keep editing the previous set.
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
        tick_buy: color_field(backend, window, cx, |t| t.tick_buy, |t, v| t.tick_buy = v),
        tick_sell: color_field(backend, window, cx, |t| t.tick_sell, |t, v| t.tick_sell = v),
        tick_liq: color_field(backend, window, cx, |t| t.tick_liq, |t, v| t.tick_liq = v),
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
        book_bid: book_wall_field(backend, window, cx, true),
        book_ask: book_wall_field(backend, window, cx, false),
        book_level_bid: book_level_field(backend, window, cx, true),
        book_level_ask: book_level_field(backend, window, cx, false),
        book_level_alpha: num_field(
            backend,
            cx,
            |t| t.book_level_alpha,
            |t, v| t.book_level_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        book_level_width: num_field(
            backend,
            cx,
            |t| t.book_level_width,
            |t, v| t.book_level_width = v,
            1.0,
            4.0,
            0.1,
        ),
        panel_bg: color_field(backend, window, cx, |t| t.panel_bg, |t, v| t.panel_bg = v),
    }
}

impl SettingsView {
    /// Render a level-colour picker with a per-side reset because MoonUI has no clear event.
    /// The controls wrap at narrow widths; resetting rebuilds the retained picker from the wall.
    fn book_level_row(&self, bid: bool, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let (label, state, id) = if bid {
            (
                t!("iface.book_level_bid"),
                &self.iface.book_level_bid,
                "book-level-bid-auto",
            )
        } else {
            (
                t!("iface.book_level_ask"),
                &self.iface.book_level_ask,
                "book-level-ask-auto",
            )
        };
        let automatic = {
            let b = self.backend.read(cx);
            let draft = b.preview.as_ref().unwrap_or(&b.config);
            let theme = draft.theme.get(draft.ui_theme_mode.is_light());
            if bid {
                theme.book_level_bid.is_none()
            } else {
                theme.book_level_ask.is_none()
            }
        };
        h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 10.0))
            .child(color_row(&label, state, p, cx))
            .child(
                MoonButton::new(id)
                    .outline()
                    .label(t!("iface.book_level_auto"))
                    .disabled(automatic)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.backend.update(cx, |b, bcx| {
                            if let Some(draft) = b.preview.as_mut() {
                                let theme = draft.theme.get_mut(draft.ui_theme_mode.is_light());
                                if bid {
                                    theme.book_level_bid = None;
                                } else {
                                    theme.book_level_ask = None;
                                }
                                bcx.notify();
                            }
                        });
                        let state = book_level_field(&this.backend, window, cx, bid);
                        if bid {
                            this.iface.book_level_bid = state;
                        } else {
                            this.iface.book_level_ask = state;
                        }
                        cx.notify();
                    }))
                    .render(),
            )
    }

    /// Render the Interface tab for the portable `theme.toml` chart theme variant.
    ///
    /// Sections cover chart-label font, chart background/grid, crosshair, candles, price lines,
    /// trades, order book, and panels. Personal interface mode and UI font settings belong to
    /// General in `settings.toml`.
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
            // Trade colors are shared by all chart tabs using this theme.
            .child(section(&t!("iface.sec_trades"), p, cx))
            .child(color_row(&t!("iface.tick_buy"), &i.tick_buy, p, cx))
            .child(color_row(&t!("iface.tick_sell"), &i.tick_sell, p, cx))
            .child(color_row(&t!("iface.tick_liq"), &i.tick_liq, p, cx))
            .child(separator(p, cx))
            // Order book.
            .child(section(&t!("iface.sec_book"), p, cx))
            .child(color_row(&t!("iface.book_bg"), &i.book_bg, p, cx))
            .child(color_row(&t!("iface.book_bg_ask"), &i.book_bg_ask, p, cx))
            .child(color_row(&t!("iface.book_bg_bid"), &i.book_bg_bid, p, cx))
            .child(color_row(&t!("iface.book_bid"), &i.book_bid, p, cx))
            .child(color_row(&t!("iface.book_ask"), &i.book_ask, p, cx))
            .child(self.book_level_row(true, cx))
            .child(self.book_level_row(false, cx))
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
            .child(slider_row(
                &t!("iface.book_level_width"),
                &i.book_level_width,
                1.0..=4.0,
                |v| format!("{} px", fmt::compact(v as f64, 1)),
                cx,
            ))
            .child(separator(p, cx))
            // Panels.
            .child(section(&t!("iface.sec_panels"), p, cx))
            .child(color_row(&t!("iface.panel_bg"), &i.panel_bg, p, cx))
            .child(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(
                        div()
                            .text_color(rgb(p.text_soft))
                            .child(t!("iface.dock_reset").to_string()),
                    )
                    .child(
                        MoonButton::new("iface-dock-reset")
                            .outline()
                            .label(format!("  {}  ", t!("iface.dock_reset_btn")))
                            .tooltip(t!("iface.dock_reset_tip").to_string())
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.backend.update(cx, |b, bcx| {
                                    b.request_dock_layout_reset(bcx);
                                });
                            }))
                            .render(),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .text_color(rgb(p.text_soft))
                    .child(t!("iface.hint").to_string()),
            )
    }
}
