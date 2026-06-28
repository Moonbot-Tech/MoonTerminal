//! Вкладка «Интерфейс» — тема оформления (порт egui `settings/interface.rs`): цвета
//! графика/перекрестия/стакана/панелей + слайдеры. Правки идут в draft (живое превью),
//! «Сохранить» пишет theme.toml. Состояние редактора — [`Iface`].

use gpui::*;
use moon_ui::{MoonColorPickerState, MoonPalette, MoonSliderState, MoonToggle, v_flex};
use rust_i18n::t;

use super::{SettingsView, color_row, section, separator, slider_row};
use crate::Backend;
use moon_core::config::{AppConfig, ChartTheme, UiThemeMode};

/// Состояние редактора темы: по entity на каждое поле.
pub(super) struct Iface {
    ui_font_delta: Entity<MoonSliderState>,
    label_font_delta: Entity<MoonSliderState>,
    bg: Entity<MoonColorPickerState>,
    grid: Entity<MoonColorPickerState>,
    grid_alpha: Entity<MoonSliderState>,
    cross: Entity<MoonColorPickerState>,
    cross_alpha: Entity<MoonSliderState>,
    cross_thickness: Entity<MoonSliderState>,
    book_bg: Entity<MoonColorPickerState>,
    book_bid: Entity<MoonColorPickerState>,
    book_ask: Entity<MoonColorPickerState>,
    book_level_alpha: Entity<MoonSliderState>,
    panel_bg: Entity<MoonColorPickerState>,
}

/// Слайдер f32, привязанный к общему AppConfig, а не к теме чарта (переустанавливает тему).
fn app_num_field(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
    get: fn(&AppConfig) -> f32,
    set: fn(&mut AppConfig, f32),
    min: f32,
    max: f32,
    step: f32,
) -> Entity<MoonSliderState> {
    let cur = {
        let b = backend.read(cx);
        get(b.preview.as_ref().unwrap_or(&b.config))
    };
    super::draft_slider(cx, min, max, step, cur, move |p, f, bcx| {
        if get(p) != f {
            set(p, f);
            crate::install_moon_theme_for_config(p, bcx);
            true
        } else {
            false
        }
    })
}

/// Color-picker, привязанный к полю темы: init из текущего config, на изменение —
/// пишет в `Backend.preview.theme` (живое применение + notify групп-окон).
fn color_field(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    get: fn(&ChartTheme) -> [u8; 3],
    set: fn(&mut ChartTheme, [u8; 3]),
) -> Entity<MoonColorPickerState> {
    let cur = get(&backend.read(cx).config.theme);
    super::draft_color(window, cx, cur, move |p, c| {
        if get(&p.theme) != c {
            set(&mut p.theme, c);
            true
        } else {
            false
        }
    })
}

/// Слайдер f32, привязанный к полю темы (живое применение).
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
    let cur = get(&backend.read(cx).config.theme);
    super::draft_slider(cx, min, max, step, cur, move |p, f, _bcx| {
        if get(&p.theme) != f {
            set(&mut p.theme, f);
            true
        } else {
            false
        }
    })
}

/// Собрать редактор темы из текущего draft (зовётся из `SettingsView::new`).
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Iface {
    Iface {
        ui_font_delta: app_num_field(
            backend,
            cx,
            |c| c.ui_font_delta,
            |c, v| c.ui_font_delta = v,
            -2.0,
            6.0,
            1.0,
        ),
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
        book_bg: color_field(backend, window, cx, |t| t.book_bg, |t, v| t.book_bg = v),
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
    /// Вкладка «Интерфейс»: секции График(фон/сетка) · Перекрестие · Стакан · Панели,
    /// цветовые ряды (свотч+подпись) и слайдеры; разделители между секциями; хинт внизу.
    pub(super) fn interface_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let i = &self.iface;
        let p = MoonPalette::active(cx);
        let ui_theme_mode = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).ui_theme_mode
        };
        v_flex()
            .w_full()
            .gap_1()
            .child(section(&t!("iface.sec_theme"), p, cx))
            .child(
                MoonToggle::new("ui-theme-mode")
                    .checked(ui_theme_mode == UiThemeMode::Light)
                    .label(t!("iface.light_theme").to_string())
                    .on_change(cx.listener(|this, checked: &bool, _window, cx| {
                        let mode = if *checked {
                            UiThemeMode::Light
                        } else {
                            UiThemeMode::Dark
                        };
                        let changed = this.backend.update(cx, |b, bcx| {
                            let Some(p) = b.preview.as_mut() else {
                                return false;
                            };
                            if p.ui_theme_mode == mode {
                                return false;
                            }
                            p.ui_theme_mode = mode;
                            crate::install_moon_theme_for_config(p, bcx);
                            bcx.notify();
                            true
                        });
                        if changed {
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .text_color(rgb(p.text_soft))
                    .child(t!("iface.light_theme_hint").to_string()),
            )
            .child(separator(p, cx))
            // UI: шрифты и масштаб
            .child(section(&t!("iface.sec_font"), p, cx))
            .child(slider_row(&t!("iface.font_delta"), &i.ui_font_delta, cx))
            .child(slider_row(
                &t!("iface.label_font_delta"),
                &i.label_font_delta,
                cx,
            ))
            .child(separator(p, cx))
            // График: фон и сетка
            .child(section(&t!("iface.sec_chart"), p, cx))
            .child(color_row(&t!("iface.bg"), &i.bg, p, cx))
            .child(color_row(&t!("iface.grid"), &i.grid, p, cx))
            .child(slider_row(&t!("iface.grid_alpha"), &i.grid_alpha, cx))
            .child(separator(p, cx))
            // График: перекрестие
            .child(section(&t!("iface.sec_cross"), p, cx))
            .child(color_row(&t!("iface.cross"), &i.cross, p, cx))
            .child(slider_row(&t!("iface.cross_alpha"), &i.cross_alpha, cx))
            .child(slider_row(
                &t!("iface.cross_thickness"),
                &i.cross_thickness,
                cx,
            ))
            .child(separator(p, cx))
            // Стакан
            .child(section(&t!("iface.sec_book"), p, cx))
            .child(color_row(&t!("iface.book_bg"), &i.book_bg, p, cx))
            .child(color_row(&t!("iface.book_bid"), &i.book_bid, p, cx))
            .child(color_row(&t!("iface.book_ask"), &i.book_ask, p, cx))
            .child(slider_row(
                &t!("iface.book_level_alpha"),
                &i.book_level_alpha,
                cx,
            ))
            .child(separator(p, cx))
            // Панели
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
