//! Appearance page: the interface theme as cards, UI zoom, and the chart colour set in groups
//! beside a still preview, then the order-book width and the dock reset.
//!
//! Edits update the draft for live preview; Save writes `theme.toml`, and the theme, zoom and
//! global order-book width in `settings.toml`. [`Iface`] owns the colour-set controls.
//!
//! The trade-mark sizes and the bottom-volume band are NOT here. They describe a chart tab rather
//! than a colour scheme, so they live on `ChartGraphicsCfg` and are edited from the chart's palette
//! popup (`crate::chart_tabs::graphics_popup`).

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonCheckbox, MoonColorPickerEvent, MoonColorPickerState, MoonPalette, MoonSlider,
    MoonSliderState, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::common::num_input;
use super::{SettingsView, color_row};
use crate::{Backend, design};
use moon_core::config::{AppConfig, ChartTheme, UiThemeMode};
use moon_core::util::fmt;

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
    hvol_bg: Entity<MoonColorPickerState>,
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
        hvol_bg: color_field(backend, window, cx, |t| t.hvol_bg, |t, v| t.hvol_bg = v),
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
    /// The reset is offered only for a picked colour; resetting rebuilds the retained picker from
    /// the wall.
    fn book_level_row(&self, bid: bool, cx: &Context<Self>) -> AnyElement {
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
        // Automatic already: a muted note, not a greyed button that reads as a dead action.
        if automatic {
            let note = div()
                .debug_selector(move || id.to_string())
                .flex_none()
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(t!("iface.book_level_is_auto").to_string())
                .into_any_element();
            return color_line(
                color_row(&label, state, p, cx).into_any_element(),
                Some(note),
                cx,
            );
        }
        let reset = div()
            .debug_selector(move || id.to_string())
            .flex_none()
            .child(
                MoonButton::new(id)
                    .variant(moon_ui::MoonButtonVariant::Neutral)
                    .label(format!("  {}  ", t!("iface.book_level_auto")))
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
            .into_any_element();
        // The reset sits inline at the row's right, like a slider beside its colour.
        color_line(
            color_row(&label, state, p, cx).into_any_element(),
            Some(reset),
            cx,
        )
    }

    /// Switch the interface theme from a theme card.
    ///
    /// The per-mode editors hold a SNAPSHOT of the colour set that was live when their widgets
    /// were built, so a mode change rebuilds them, exactly as the theme-mode select does; the
    /// select is moved along so the two never disagree.
    fn pick_theme_mode(&mut self, mode: UiThemeMode, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let Some(p) = b.preview.as_mut() else {
                return false;
            };
            if !pick_theme(p, mode) {
                return false;
            }
            crate::install_moon_theme_for_config(p, bcx);
            bcx.notify();
            true
        });
        if changed {
            self.theme_mode
                .update(cx, |s, _| s.set_selected_value(&mode));
            self.iface = build(&self.backend, window, cx);
            self.lines = super::lines::build(&self.backend, window, cx);
            self.badges = super::badges::build(&self.backend, window, cx);
            cx.notify();
        }
    }

    /// The theme cards offered now: the three reviewed themes, plus the experimental ones while
    /// their checkbox is on or one of them is the current theme.
    fn theme_cards(&self, current: UiThemeMode, cx: &Context<Self>) -> Vec<AnyElement> {
        visible_modes(self.appearance_ui.show_experimental, current)
            .into_iter()
            .map(|mode| {
                let this = cx.entity().downgrade();
                let on_click: super::common::Action = Rc::new(move |window, cx| {
                    let _ = this.update(cx, |view, cx| view.pick_theme_mode(mode, window, cx));
                });
                theme_card(mode, mode == current, on_click, cx)
            })
            .collect()
    }

    /// The colour groups of the active colour set, each a title and its rows.
    fn color_groups(&self, cx: &Context<Self>) -> Vec<ColorGroup> {
        let i = &self.iface;
        let p = MoonPalette::active(cx);
        let pct = |v: f32| {
            fmt::pct((v * 100.0) as f64, 0).map_or_else(|| "0%".to_string(), |(text, _)| text)
        };
        let px1 = |v: f32| format!("{} px", fmt::compact(v as f64, 1));
        let color = |key: &str, st: &Entity<MoonColorPickerState>| {
            color_line(color_row(&t!(key), st, p, cx).into_any_element(), None, cx)
        };
        let color_alpha =
            |key: &str, st: &Entity<MoonColorPickerState>, alpha: &Entity<MoonSliderState>| {
                color_line(
                    color_row(&t!(key), st, p, cx).into_any_element(),
                    Some(slider_ctl(alpha, &pct, cx)),
                    cx,
                )
            };
        let slider = |key: &str, st: &Entity<MoonSliderState>, f: &dyn Fn(f32) -> String| {
            color_line(
                super::common::prose(
                    format!("iface-slider-label-{key}"),
                    t!(key).to_string(),
                    false,
                    cx,
                )
                .text_color(rgba_from(p.text_soft, 1.0))
                .into_any_element(),
                Some(slider_ctl(st, f, cx)),
                cx,
            )
        };
        vec![
            ColorGroup {
                id: "iface-grp-bg",
                title: t!("iface.grp.bg_grid").to_string(),
                rows: vec![
                    color("iface.bg", &i.bg),
                    color_alpha("iface.grid", &i.grid, &i.grid_alpha),
                    color("iface.panel_bg", &i.panel_bg),
                    color("iface.hvol_bg", &i.hvol_bg),
                ],
            },
            ColorGroup {
                id: "iface-grp-candles",
                title: t!("iface.grp.candles").to_string(),
                rows: vec![
                    color("iface.candle_up", &i.candle_up),
                    color("iface.candle_down", &i.candle_down),
                    color("iface.candle_neutral", &i.candle_neutral),
                    slider("iface.candle_fill_alpha", &i.candle_fill_alpha, &pct),
                ],
            },
            ColorGroup {
                id: "iface-grp-price",
                title: t!("iface.grp.price_cross").to_string(),
                rows: vec![
                    color_alpha("iface.price_line", &i.price_line, &i.price_line_alpha),
                    color_alpha("iface.mark_line", &i.mark_line, &i.mark_line_alpha),
                    // Logical pixels: the device scale is applied where the uniform is built.
                    slider("iface.price_line_px", &i.price_line_px, &px1),
                    color_alpha("iface.cross", &i.cross, &i.cross_alpha),
                    slider("iface.cross_thickness", &i.cross_thickness, &px1),
                ],
            },
            ColorGroup {
                id: "iface-grp-trades",
                title: t!("iface.grp.trades").to_string(),
                rows: vec![
                    color("iface.tick_buy", &i.tick_buy),
                    color("iface.tick_sell", &i.tick_sell),
                    color("iface.tick_liq", &i.tick_liq),
                ],
            },
            ColorGroup {
                id: "iface-grp-book",
                title: t!("iface.grp.book").to_string(),
                rows: vec![
                    color("iface.book_bg_bid", &i.book_bg_bid),
                    color("iface.book_bg_ask", &i.book_bg_ask),
                    color("iface.book_bg", &i.book_bg),
                    color("iface.book_bid", &i.book_bid),
                    color("iface.book_ask", &i.book_ask),
                    self.book_level_row(true, cx),
                    self.book_level_row(false, cx),
                    slider("iface.book_level_alpha", &i.book_level_alpha, &pct),
                    slider("iface.book_level_width", &i.book_level_width, &px1),
                ],
            },
            ColorGroup {
                id: "iface-grp-labels",
                title: t!("iface.grp.labels").to_string(),
                rows: vec![slider(
                    "iface.label_font_delta",
                    &i.label_font_delta,
                    &|v| {
                        fmt::round_to(v as f64, 1).map_or_else(
                            || "+0.0 px".to_string(),
                            |rounded| format!("{rounded:+.1} px"),
                        )
                    },
                )],
            },
        ]
    }

    /// Render the Appearance page body: interface theme and zoom, the chart colour set with its
    /// preview, the order-book width and the dock reset.
    ///
    /// Args:
    ///     cx: Settings context that supplies the active draft and palette.
    ///
    /// Returns:
    ///     The assembled page content.
    pub(super) fn interface_tab(&self, cx: &Context<Self>) -> AnyElement {
        let (mode, theme, show) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.ui_theme_mode,
                d.theme.get(d.ui_theme_mode.is_light()).clone(),
                self.appearance_ui.show_experimental,
            )
        };
        let experimental_label = t!("iface.theme.experimental").to_string();
        let experimental = super::common::floored_checkbox(
            "iface-show-experimental",
            &experimental_label,
            MoonCheckbox::new("iface-show-experimental")
                .checked(show)
                .label(experimental_label.clone())
                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                    this.appearance_ui.show_experimental = *checked;
                    cx.notify();
                })),
            cx,
        );
        let dock_reset = MoonButton::new("iface-dock-reset")
            .variant(moon_ui::MoonButtonVariant::Neutral)
            .size(design::CONTROL_TIER)
            .label(t!("iface.dock_reset_btn").to_string())
            .on_click(cx.listener(|this, _, _window, cx| {
                this.backend.update(cx, |b, bcx| {
                    b.request_dock_layout_reset(bcx);
                });
            }))
            .render()
            .into_any_element();
        let parts = AppearanceParts {
            cards: self.theme_cards(mode, cx),
            experimental,
            zoom: slider_ctl_w(&self.ui_zoom, &super::general::zoom_text, ZOOM_TRACK_W, cx),
            set_note: t!(set_note_key(mode)).to_string(),
            groups: self.color_groups(cx),
            preview: chart_preview(&theme, cx),
            book_width: num_input(
                "iface-book-width",
                &self.appearance_ui.order_book_width_px,
                f64::from(moon_core::config::book_width::MIN)
                    ..=f64::from(moon_core::config::book_width::MAX),
                10.0,
                Some("px".into()),
            ),
            dock_reset,
        };
        appearance_body(parts, &|key| self.more_toggle(key, cx), cx)
    }

    /// The Theme and colours page, Copy and Paste in its header.
    pub(super) fn appearance_page(&self, cx: &Context<Self>) -> super::frame::PageParts {
        let body = self.interface_tab(cx);
        self.page_parts(super::Tab::Appearance, body, self.share_actions(cx), cx)
    }
}

/// Set the interface theme in the draft.
///
/// Returns:
///     Whether the draft changed.
fn pick_theme(p: &mut AppConfig, mode: UiThemeMode) -> bool {
    if p.ui_theme_mode == mode {
        return false;
    }
    p.ui_theme_mode = mode;
    true
}

/// The theme cards to show: the reviewed three always, the experimental ones when asked for or
/// when one of them is already in use (hiding the current theme would leave no card selected).
fn visible_modes(show_experimental: bool, current: UiThemeMode) -> Vec<UiThemeMode> {
    let experimental = [
        UiThemeMode::LightExperimental,
        UiThemeMode::DarkExperimental,
    ];
    let mut modes = vec![UiThemeMode::Light, UiThemeMode::Graphite, UiThemeMode::Dark];
    if show_experimental || experimental.contains(&current) {
        modes.extend(experimental);
    }
    modes
}

/// Locale key naming which chart colour set the page edits for `mode`.
fn set_note_key(mode: UiThemeMode) -> &'static str {
    if mode.is_light() {
        "iface.set_note.light"
    } else {
        "iface.set_note.dark"
    }
}

/// Element id and caption of a theme card.
fn theme_card_meta(mode: UiThemeMode) -> (&'static str, &'static str) {
    match mode {
        UiThemeMode::Light => ("theme-card-light", "iface.theme.light"),
        UiThemeMode::Graphite => ("theme-card-graphite", "iface.theme.graphite"),
        UiThemeMode::Dark => ("theme-card-dark", "iface.theme.dark"),
        UiThemeMode::LightExperimental => {
            ("theme-card-light-exp", "iface.light_experimental_theme")
        }
        UiThemeMode::DarkExperimental => ("theme-card-dark-exp", "iface.dark_experimental_theme"),
    }
}

/// The interface palette a theme card previews: the theme's own shell, not the active one. The
/// dark card takes MoonUI's default, the dark shell, since it previews a theme not in use.
fn card_palette(mode: UiThemeMode) -> MoonPalette {
    match mode {
        UiThemeMode::Graphite => MoonPalette::GRAPHITE,
        mode if mode.is_light() => MoonPalette::LIGHT,
        _ => MoonPalette::default(),
    }
}

/// One theme card: a miniature of the theme's panel and chart, and its name.
fn theme_card(
    mode: UiThemeMode,
    selected: bool,
    on_click: super::common::Action,
    cx: &App,
) -> AnyElement {
    let (id, label) = theme_card_meta(mode);
    let pal = card_palette(mode);
    let candle = |up: bool, top: f32, h: f32| {
        div()
            .absolute()
            .top(relative(top))
            .h(relative(h))
            .w(design::ui_px(cx, 4.0))
            .bg(rgba_from(if up { pal.green } else { pal.red }, 1.0))
    };
    let mini = h_flex()
        .w_full()
        .h(design::ui_px(cx, 44.0))
        .rounded(design::ui_px(cx, 4.0))
        .overflow_hidden()
        .border_1()
        .border_color(rgba_from(pal.border, 1.0))
        .child(
            div()
                .w(relative(0.22))
                .h_full()
                .bg(rgba_from(pal.shell_high, 1.0)),
        )
        .child(
            h_flex()
                .flex_1()
                .h_full()
                .relative()
                .bg(rgba_from(pal.shell, 1.0))
                .gap(design::ui_px(cx, 6.0))
                .px(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .relative()
                        .w(design::ui_px(cx, 4.0))
                        .h_full()
                        .child(candle(true, 0.45, 0.35)),
                )
                .child(
                    div()
                        .relative()
                        .w(design::ui_px(cx, 4.0))
                        .h_full()
                        .child(candle(false, 0.3, 0.3)),
                )
                .child(
                    div()
                        .relative()
                        .w(design::ui_px(cx, 4.0))
                        .h_full()
                        .child(candle(true, 0.2, 0.4)),
                )
                .child(
                    div()
                        .relative()
                        .w(design::ui_px(cx, 4.0))
                        .h_full()
                        .child(candle(true, 0.1, 0.3)),
                ),
        );
    div()
        .w(design::ui_px(cx, 150.0))
        .debug_selector(move || id.to_string())
        .child(super::common::select_card(
            id,
            selected,
            on_click,
            vec![
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .child(mini)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!(label).to_string()),
                    )
                    .into_any_element(),
            ],
        ))
        .into_any_element()
}

/// Track width of a colour-group slider, unscaled.
const SLIDER_TRACK_W: f32 = 110.0;
/// Track width of the interface-scale slider, unscaled: a page-level row has room for more.
const ZOOM_TRACK_W: f32 = 220.0;

/// A compact slider for a colour-group row: the track and its value in the mono face.
pub(super) fn slider_ctl(
    st: &Entity<MoonSliderState>,
    format: &dyn Fn(f32) -> String,
    cx: &App,
) -> AnyElement {
    slider_ctl_w(st, format, SLIDER_TRACK_W, cx)
}

/// [`slider_ctl`] with a `track_w` (unscaled) track. The value is left-aligned in its fixed
/// box, right beside the track, so values of stacked rows line up and none drifts away.
pub(super) fn slider_ctl_w(
    st: &Entity<MoonSliderState>,
    format: &dyn Fn(f32) -> String,
    track_w: f32,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let value = st.read(cx).value().end();
    let value = if value == 0.0 { 0.0 } else { value };
    h_flex()
        .flex_none()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .child(
            div()
                .w(design::ui_px(cx, track_w))
                .child(MoonSlider::new(st).height(design::ui_value(cx, 22.0))),
        )
        .child(
            div()
                .w(design::font_w_px(cx, 56.0))
                .font_family(design::mono())
                .text_align(TextAlign::Left)
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(format(value)),
        )
        .into_any_element()
}

/// One colour-group row: what it edits in a label column, an optional slider or action right
/// after it; the control drops under the label when the group is narrow.
///
/// The label column stops at [`COLOR_LABEL_MAX_W`], so in a wide group the control stays beside
/// its label rather than pinned to the far edge, and the controls of a group start on one line.
pub(super) fn color_line(left: AnyElement, right: Option<AnyElement>, cx: &App) -> AnyElement {
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 2.0))
        // The label side takes what the control leaves and wraps under it below that floor.
        .child(
            div()
                .flex_1()
                .min_w(design::font_w_px(cx, 140.0))
                .max_w(design::font_w_px(cx, COLOR_LABEL_MAX_W))
                .child(left),
        )
        .children(right)
        .into_any_element()
}

/// The widest a [`color_line`] label column grows, unscaled.
const COLOR_LABEL_MAX_W: f32 = 260.0;

/// A colour group of the Appearance page.
pub(super) struct ColorGroup {
    id: &'static str,
    title: String,
    rows: Vec<AnyElement>,
}

/// The parts of the Appearance page, built by the view and laid out by [`appearance_body`].
pub(super) struct AppearanceParts {
    cards: Vec<AnyElement>,
    experimental: AnyElement,
    zoom: AnyElement,
    /// Which chart colour set is edited, from [`set_note_key`].
    set_note: String,
    groups: Vec<ColorGroup>,
    preview: AnyElement,
    book_width: AnyElement,
    dock_reset: AnyElement,
}

/// The Appearance page body.
///
/// The preview sits right of the colour groups while the body has room for both, and drops
/// below them when it does not: the two wrap rather than squeeze.
fn appearance_body(
    parts: AppearanceParts,
    more: &dyn Fn(&'static str) -> (bool, super::common::Action),
    cx: &App,
) -> AnyElement {
    let pal = MoonPalette::active(cx);
    let muted = rgba_from(pal.text_muted, 1.0);
    let row = |id: &'static str,
               label: String,
               hint: String,
               long: Option<String>,
               control: AnyElement| {
        let (open, toggle) = more(id);
        div()
            .debug_selector(move || id.to_string())
            .child(super::common::hint_row(
                id,
                label,
                Some(hint),
                long,
                control,
                open,
                toggle,
                cx,
            ))
            .into_any_element()
    };
    // The groups keep 420 before the preview drops below them: with the sidebar and padding that
    // is the mockup's 980-pixel window breakpoint.
    let groups = h_flex()
        .flex_1()
        .min_w(design::ui_px(cx, 420.0))
        .flex_wrap()
        .items_start()
        .gap(design::ui_px(cx, 12.0))
        .children(parts.groups.into_iter().map(|g| {
            div()
                .flex_1()
                .min_w(design::ui_px(cx, 260.0))
                .child(super::general::page_section(g.id, g.title, g.rows, cx))
        }));
    let preview = v_flex()
        .flex_none()
        .w(design::ui_px(cx, 300.0))
        .max_w_full()
        .gap(design::ui_px(cx, 6.0))
        .debug_selector(|| "iface-preview".into())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("iface.preview").to_string()),
        )
        .child(parts.preview);
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 16.0))
        .child(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("iface.theme.title").to_string()),
                )
                .child(
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .gap(design::ui_px(cx, 10.0))
                        .debug_selector(|| "iface-themes".into())
                        .children(parts.cards),
                )
                .child(parts.experimental),
        )
        .child(super::general::page_box(
            vec![row(
                "iface-zoom",
                t!("iface.ui_zoom").to_string(),
                t!("iface.ui_zoom.hint").to_string(),
                Some(t!("iface.ui_zoom_hint").to_string()),
                parts.zoom,
            )],
            cx,
        ))
        .child(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .debug_selector(|| "iface-set-note".into())
                        .text_color(muted)
                        .child(parts.set_note),
                )
                .child(
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .items_start()
                        .gap(design::ui_px(cx, 14.0))
                        .debug_selector(|| "iface-colors".into())
                        .child(groups)
                        .child(preview),
                ),
        )
        .child(super::general::page_box(
            vec![
                row(
                    "iface-book-width",
                    t!("iface.order_book_width").to_string(),
                    t!("iface.order_book_width.hint").to_string(),
                    None,
                    parts.book_width,
                ),
                row(
                    "iface-dock-reset",
                    t!("iface.dock_reset").to_string(),
                    t!("iface.dock_reset.hint").to_string(),
                    Some(t!("iface.dock_reset_tip").to_string()),
                    parts.dock_reset,
                ),
            ],
            cx,
        ))
        // `theme.toml` is the shareable file the colour set lands in.
        .child(
            div()
                .debug_selector(|| "iface-footnote".into())
                .text_color(muted)
                .child(t!("iface.hint").to_string()),
        )
        .into_any_element()
}

/// A still picture of a chart in the edited colour set: grid, candles, the price and mark lines,
/// and the order-book zones on the right.
///
/// Drawn from plain elements rather than the chart renderer: the renderer reads candles only
/// from live market rings, and a synthetic series would need a market-data source the settings
/// window does not have.
fn chart_preview(theme: &ChartTheme, cx: &App) -> AnyElement {
    let c = |rgb: [u8; 3], alpha: f32| {
        Hsla::from(Rgba {
            r: f32::from(rgb[0]) / 255.0,
            g: f32::from(rgb[1]) / 255.0,
            b: f32::from(rgb[2]) / 255.0,
            a: alpha.clamp(0.0, 1.0),
        })
    };
    let level = |bid: bool| {
        let v = theme.resolved_book_level(bid);
        Hsla::from(Rgba {
            r: v[0],
            g: v[1],
            b: v[2],
            a: theme.book_level_alpha.clamp(0.0, 1.0),
        })
    };
    // (open, close, high, low) as fractions of the chart height from the top.
    const CANDLES: [(f32, f32, f32, f32); 10] = [
        (0.62, 0.55, 0.50, 0.66),
        (0.55, 0.58, 0.52, 0.62),
        (0.58, 0.47, 0.44, 0.60),
        (0.47, 0.42, 0.38, 0.50),
        (0.42, 0.46, 0.40, 0.49),
        (0.46, 0.36, 0.33, 0.48),
        (0.36, 0.40, 0.34, 0.43),
        (0.40, 0.30, 0.27, 0.42),
        (0.30, 0.33, 0.28, 0.36),
        (0.33, 0.26, 0.22, 0.35),
    ];
    let chart_w = 0.72;
    let step = chart_w / CANDLES.len() as f32;
    let mut chart = div()
        .relative()
        .w_full()
        .h(design::ui_px(cx, 220.0))
        .overflow_hidden()
        .rounded(design::r_container(cx))
        .bg(c(theme.bg, 1.0));
    for k in 1..4 {
        let y = k as f32 / 4.0;
        chart = chart.child(
            div()
                .absolute()
                .left_0()
                .w(relative(chart_w))
                .top(relative(y))
                .h(px(1.0))
                .bg(c(theme.grid, theme.grid_alpha)),
        );
    }
    for (n, (open, close, high, low)) in CANDLES.into_iter().enumerate() {
        let up = close < open;
        let color = if up {
            theme.candle_up
        } else {
            theme.candle_down
        };
        let x = step * n as f32 + step * 0.2;
        let (top, bottom) = if up { (close, open) } else { (open, close) };
        chart = chart
            .child(
                div()
                    .absolute()
                    .left(relative(x + step * 0.28))
                    .w(px(1.0))
                    .top(relative(high))
                    .h(relative(low - high))
                    .bg(c(color, 1.0)),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(x))
                    .w(relative(step * 0.6))
                    .top(relative(top))
                    .h(relative((bottom - top).max(0.01)))
                    .border_1()
                    .border_color(c(color, 1.0))
                    .bg(c(color, theme.candle_fill_alpha)),
            );
    }
    let last = CANDLES[CANDLES.len() - 1].1;
    chart = chart
        .child(
            div()
                .absolute()
                .left_0()
                .w(relative(chart_w))
                .top(relative(last))
                .h(px(theme.price_line_px.max(0.5)))
                .bg(c(theme.price_line, theme.price_line_alpha)),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .w(relative(chart_w))
                .top(relative(last + 0.04))
                .h(px(1.0))
                .bg(c(theme.mark_line, theme.mark_line_alpha)),
        );
    // The book: ask zone above the spread, bid zone below, each with one wall and one level.
    let book = v_flex()
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(chart_w))
        .right_0()
        .child(
            div()
                .relative()
                .flex_1()
                .bg(c(theme.book_bg_ask, 1.0))
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom(relative(0.3))
                        .h(px(4.0))
                        .w(relative(0.8))
                        .bg(c(theme.book_ask, 1.0)),
                )
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom(relative(0.6))
                        .h(px(3.0))
                        .w(relative(0.4))
                        .bg(level(false)),
                ),
        )
        .child(div().h(relative(0.06)).bg(c(theme.book_bg, 1.0)))
        .child(
            div()
                .relative()
                .flex_1()
                .bg(c(theme.book_bg_bid, 1.0))
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .top(relative(0.3))
                        .h(px(4.0))
                        .w(relative(0.7))
                        .bg(c(theme.book_bid, 1.0)),
                )
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .top(relative(0.6))
                        .h(px(3.0))
                        .w(relative(0.35))
                        .bg(level(true)),
                ),
        );
    chart.child(book).into_any_element()
}

/// UI state of the Appearance page that the draft does not hold. Branch-owned: fields are added
/// here, never on `SettingsView`.
pub(super) struct AppearanceUi {
    /// Number field for `order_book_width_px`.
    pub(super) order_book_width_px: Entity<moon_ui::MoonInputState>,
    /// Whether the experimental theme cards are shown. Per Settings session; not a setting.
    pub(super) show_experimental: bool,
}

impl AppearanceUi {
    /// Fields seeded from the draft `config` and bound to the draft.
    pub(super) fn init(
        config: &AppConfig,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
    ) -> Self {
        use moon_core::config::book_width;
        Self {
            order_book_width_px: super::general::draft_number(
                f64::from(book_width::normalize(config.order_book_width_px).round()),
                window,
                cx,
                |p| {
                    Some(f64::from(
                        book_width::normalize(p.order_book_width_px).round(),
                    ))
                },
                |p, v| {
                    let width = book_width::normalize(v as f32);
                    let changed = p.order_book_width_px != width;
                    p.order_book_width_px = width;
                    changed
                },
            ),
            show_experimental: false,
        }
    }
}

#[cfg(test)]
mod tests;
