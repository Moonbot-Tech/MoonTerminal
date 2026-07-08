//! Вкладка «Бейджи» — код+цвет бейджа по видам стратегий (типам детектов) + опция
//! «помечать направление (isShort) обводкой». Правки идут в draft (живое превью в
//! строке), «Сохранить» пишет отдельный переносимый `badges.json`. Цвета — РАЗДЕЛЬНО
//! под активную тему (как «Линии»/«Ордера»); список растёт кнопкой «Добавить тип».
//!
//! Состояние редактора — [`BadgesEd`]; строки пересобираются при add/del (свежие
//! индексы в подписках), как вкладка «Подключения».

use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonButton, MoonButtonSize, MoonCheckboxSize,
    MoonColorPicker, MoonColorPickerState, MoonInput, MoonInputEvent, MoonInputState, MoonPalette,
    StyledExt, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::{SettingsView, color_row, separator};
use crate::{design, Backend};
use moon_core::config::{BadgeEntry, UiThemeMode};

/// Редактор одной строки бейджа: поля ввода + пикер цвета активной темы.
pub(super) struct BadgeRowEd {
    ordinal: Entity<MoonInputState>,
    name: Entity<MoonInputState>,
    code: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
}

/// Состояние редактора вкладки «Бейджи».
pub(super) struct BadgesEd {
    /// Тема, набор цветов которой сейчас редактируется (по активной теме приложения).
    is_light: bool,
    rows: Vec<BadgeRowEd>,
    short_color: Entity<MoonColorPickerState>,
    long_color: Entity<MoonColorPickerState>,
}

/// TextInput, привязанный к полю записи `badges.entries[idx]` (пишет в draft).
fn badge_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    idx: usize,
    init: String,
    apply: impl Fn(&mut BadgeEntry, String) + 'static,
) -> Entity<MoonInputState> {
    let st = cx.new(|cx| MoonInputState::new(window, cx).default_value(init));
    cx.subscribe(&st, move |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut() {
                    if let Some(e) = p.badges.entries.get_mut(idx) {
                        apply(e, val);
                        bcx.notify();
                    }
                }
            });
        }
    })
    .detach();
    st
}

/// Color-picker цвета бейджа `badges.entries[idx]` под активную тему (пишет в draft).
fn badge_color(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    idx: usize,
    is_light: bool,
) -> Entity<MoonColorPickerState> {
    let init = {
        let b = backend.read(cx);
        let bcfg = &b.preview.as_ref().unwrap_or(&b.config).badges;
        bcfg.entries
            .get(idx)
            .map(|e| e.color(is_light))
            .unwrap_or([0x97, 0x92, 0x8A])
    };
    super::draft_color(window, cx, init, move |p, c| {
        if let Some(e) = p.badges.entries.get_mut(idx) {
            if e.color(is_light) != c {
                if is_light {
                    e.color_light = c;
                } else {
                    e.color_dark = c;
                }
                return true;
            }
        }
        false
    })
}

/// Собрать редактор бейджей из текущего draft (зовётся из `SettingsView::new` и после add/del).
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> BadgesEd {
    // Редактируем набор цветов АКТИВНОЙ темы приложения (по `ui_theme_mode`), как «Линии».
    let is_light = backend.read(cx).config.ui_theme_mode == UiThemeMode::Light;
    let entries = {
        let b = backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .badges
            .entries
            .clone()
    };
    let rows = entries
        .iter()
        .enumerate()
        .map(|(idx, e)| BadgeRowEd {
            ordinal: badge_input(window, cx, idx, e.ordinal.to_string(), |e, v| {
                if let Ok(n) = v.trim().parse::<u8>() {
                    e.ordinal = n;
                }
            }),
            name: badge_input(window, cx, idx, e.name.clone(), |e, v| e.name = v),
            // Код — максимум 3 символа (не обязательно буквы): обрезаем по символам.
            code: badge_input(window, cx, idx, e.code.clone(), |e, v| {
                e.code = v.chars().take(3).collect();
            }),
            color: badge_color(backend, window, cx, idx, is_light),
        })
        .collect();

    let (short_init, long_init) = {
        let b = backend.read(cx);
        let bcfg = &b.preview.as_ref().unwrap_or(&b.config).badges;
        (bcfg.outline(true, is_light), bcfg.outline(false, is_light))
    };
    let short_color = super::draft_color(window, cx, short_init, move |p, c| {
        if p.badges.outline(true, is_light) != c {
            if is_light {
                p.badges.short_outline_light = c;
            } else {
                p.badges.short_outline_dark = c;
            }
            true
        } else {
            false
        }
    });
    let long_color = super::draft_color(window, cx, long_init, move |p, c| {
        if p.badges.outline(false, is_light) != c {
            if is_light {
                p.badges.long_outline_light = c;
            } else {
                p.badges.long_outline_dark = c;
            }
            true
        } else {
            false
        }
    });

    BadgesEd {
        is_light,
        rows,
        short_color,
        long_color,
    }
}

impl SettingsView {
    /// Добавить новый вид в draft (ordinal = max+1) и пересобрать редактор.
    fn add_badge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_color = design::u32_to_rgb(MoonPalette::active(cx).accent);
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                let next = p
                    .badges
                    .entries
                    .iter()
                    .map(|e| e.ordinal)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1);
                p.badges.entries.push(BadgeEntry {
                    ordinal: next,
                    name: String::new(),
                    code: "???".to_string(),
                    color_dark: default_color,
                    color_light: default_color,
                });
                bcx.notify();
            }
        });
        self.badges = build(&self.backend, window, cx);
        cx.notify();
    }

    /// Удалить вид `idx` из draft и пересобрать редактор.
    fn delete_badge(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if idx < p.badges.entries.len() {
                    p.badges.entries.remove(idx);
                    bcx.notify();
                }
            }
        });
        self.badges = build(&self.backend, window, cx);
        cx.notify();
    }

    /// Строка редактора бейджа: ordinal · имя · код · цвет · превью · удалить.
    fn badge_row(&self, cx: &Context<Self>, idx: usize, row: &BadgeRowEd) -> impl IntoElement {
        let is_light = self.badges.is_light;
        // Превью-бейдж из живого draft (код+цвет активной темы).
        let (code, color) = {
            let b = self.backend.read(cx);
            let bcfg = &b.preview.as_ref().unwrap_or(&b.config).badges;
            bcfg.entries
                .get(idx)
                .map(|e| (e.code.clone(), e.color(is_light)))
                .unwrap_or_else(|| ("UNK".to_string(), [0x97, 0x92, 0x8A]))
        };
        let color = design::rgb_to_u32(color);
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .py_0p5()
            .child(
                div().w(px(58.0)).child(
                    MoonInput::new(SharedString::from(format!("badge-ord-{idx}")))
                        .state(&row.ordinal)
                        .small(),
                ),
            )
            .child(
                div().w(px(160.0)).child(
                    MoonInput::new(SharedString::from(format!("badge-name-{idx}")))
                        .state(&row.name)
                        .small(),
                ),
            )
            .child(
                div().w(px(64.0)).child(
                    MoonInput::new(SharedString::from(format!("badge-code-{idx}")))
                        .state(&row.code)
                        .small(),
                ),
            )
            .child(div().w(px(110.0)).child(MoonColorPicker::new(&row.color)))
            .child(
                div().w(px(56.0)).flex().justify_center().child(
                    MoonBadge::new(code)
                        .variant(MoonBadgeVariant::Soft)
                        .size(MoonBadgeSize::Status)
                        .bg_color(color)
                        .text_color(color)
                        .mono(true),
                ),
            )
            .child(
                MoonButton::new(SharedString::from(format!("badge-del-{idx}")))
                    .danger()
                    .size(MoonButtonSize::Micro)
                    .width(24.0)
                    .label("x")
                    .on_click(cx.listener(move |this, _, w, cx| this.delete_badge(idx, w, cx)))
                    .render(),
            )
    }

    /// Вкладка «Бейджи»: список видов (ordinal·имя·код·цвет·превью) + «Добавить», затем
    /// блок «Направление» (галка обводки + цвета short/long активной темы).
    pub(super) fn badges_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let mark_init = {
            let b = self.backend.read(cx);
            b.preview
                .as_ref()
                .unwrap_or(&b.config)
                .badges
                .mark_direction
        };
        // Шапка колонок.
        let header = h_flex()
            .w_full()
            .gap_1()
            .text_size(design::t_caption(cx))
            .text_color(rgba_from(p.text_soft, 1.0))
            .child(div().w(px(58.0)).child(t!("badges.col_type").to_string()))
            .child(div().w(px(160.0)).child(t!("badges.col_name").to_string()))
            .child(div().w(px(64.0)).child(t!("badges.col_code").to_string()))
            .child(div().w(px(110.0)).child(t!("badges.col_color").to_string()));

        let mut col = v_flex()
            .w_full()
            .gap_1()
            .child(div().font_bold().child(t!("badges.title").to_string()))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(t!("badges.theme_hint").to_string()),
            )
            .child(header);
        for (idx, row) in self.badges.rows.iter().enumerate() {
            col = col.child(self.badge_row(cx, idx, row));
        }
        col.child(
            div().mt_1().child(
                MoonButton::new("badge-add")
                    .small()
                    .width(140.0)
                    .label(t!("badges.add").to_string())
                    .on_click(cx.listener(|this, _, w, cx| this.add_badge(w, cx)))
                    .render(),
            ),
        )
        .child(separator(p, cx))
        .child(
            div()
                .mt_1()
                .font_bold()
                .child(t!("badges.direction_group").to_string()),
        )
        .child(
            self.draft_checkbox(cx, "badge-mark-dir", mark_init, |p, v| {
                if p.badges.mark_direction != v {
                    p.badges.mark_direction = v;
                    true
                } else {
                    false
                }
            })
            .label(t!("badges.mark_direction").to_string())
            .size(MoonCheckboxSize::Compact),
        )
        .child(color_row(
            &t!("badges.short_outline"),
            &self.badges.short_color,
            p,
            cx,
        ))
        .child(color_row(
            &t!("badges.long_outline"),
            &self.badges.long_color,
            p,
            cx,
        ))
    }
}
