//! Вкладка «Бейджи» — код+цвет бейджа по видам стратегий (типам детектов). Каждая
//! строка: галка «активность» (рисовать ли бейдж), код (long; при галке «различать
//! L/S» рядом появляется код short), цвет (раздельно под тему), и галка «обводка»
//! пер-строка (при включении — цвета обводки long/short). Правки идут в draft (живое
//! превью), «Сохранить» пишет переносимый `badges.json`.
//!
//! Состояние редактора — [`BadgesEd`]; строки пересобираются при add/del (свежие
//! индексы в подписках), как вкладка «Подключения».

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonButton, MoonButtonSize, MoonCheckboxSize,
    MoonColorPicker, MoonColorPickerState, MoonInput, MoonInputEvent, MoonInputState, MoonPalette,
    StyledExt, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::{SettingsView, separator};
use crate::{design, Backend};
use moon_core::config::{BadgeEntry, UiThemeMode};

/// Редактор одной строки бейджа: поля ввода + пикеры цвета активной темы.
pub(super) struct BadgeRowEd {
    /// Индекс записи в `badges.entries` (draft). Может НЕ совпадать с позицией строки в
    /// списке: служебный вид `Unknown` (ordinal 0) в редакторе скрыт, но остаётся в данных.
    idx: usize,
    ordinal: Entity<MoonInputState>,
    name: Entity<MoonInputState>,
    code: Entity<MoonInputState>,
    code_short: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
    outline_long: Entity<MoonColorPickerState>,
    outline_short: Entity<MoonColorPickerState>,
}

/// Состояние редактора вкладки «Бейджи».
pub(super) struct BadgesEd {
    /// Тема, набор цветов которой сейчас редактируется (по активной теме приложения).
    is_light: bool,
    rows: Vec<BadgeRowEd>,
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

/// Color-picker поля записи `badges.entries[idx]` (get/set над `BadgeEntry`, пишет в draft).
fn entry_color(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    idx: usize,
    get: impl Fn(&BadgeEntry) -> [u8; 3] + Copy + 'static,
    set: impl Fn(&mut BadgeEntry, [u8; 3]) + 'static,
) -> Entity<MoonColorPickerState> {
    let init = {
        let b = backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .badges
            .entries
            .get(idx)
            .map(get)
            .unwrap_or([0x97, 0x92, 0x8A])
    };
    super::draft_color(window, cx, init, move |p, cc| {
        if let Some(e) = p.badges.entries.get_mut(idx) {
            if get(e) != cc {
                set(e, cc);
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
        // Служебный `Unknown` (ordinal 0) — фолбэк-бакет для нераспознанного типа детекта;
        // в редакторе не показываем (просьба пользователя), в данных/`badges.json` оставляем.
        .filter(|(_, e)| e.ordinal != 0)
        .map(|(idx, e)| BadgeRowEd {
            idx,
            ordinal: badge_input(window, cx, idx, e.ordinal.to_string(), |e, v| {
                if let Ok(n) = v.trim().parse::<u8>() {
                    e.ordinal = n;
                }
            }),
            name: badge_input(window, cx, idx, e.name.clone(), |e, v| e.name = v),
            code: badge_input(window, cx, idx, e.code.clone(), |e, v| {
                e.code = v.chars().take(3).collect();
            }),
            code_short: badge_input(window, cx, idx, e.code_short.clone(), |e, v| {
                e.code_short = v.chars().take(3).collect();
            }),
            color: entry_color(
                backend,
                window,
                cx,
                idx,
                move |e| e.color(is_light),
                move |e, cc| {
                    if is_light {
                        e.color_light = cc;
                    } else {
                        e.color_dark = cc;
                    }
                },
            ),
            outline_long: entry_color(
                backend,
                window,
                cx,
                idx,
                move |e| {
                    if is_light {
                        e.outline_long_light
                    } else {
                        e.outline_long_dark
                    }
                },
                move |e, cc| {
                    if is_light {
                        e.outline_long_light = cc;
                    } else {
                        e.outline_long_dark = cc;
                    }
                },
            ),
            outline_short: entry_color(
                backend,
                window,
                cx,
                idx,
                move |e| {
                    if is_light {
                        e.outline_short_light
                    } else {
                        e.outline_short_dark
                    }
                },
                move |e, cc| {
                    if is_light {
                        e.outline_short_light = cc;
                    } else {
                        e.outline_short_dark = cc;
                    }
                },
            ),
        })
        .collect();

    BadgesEd { is_light, rows }
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
                    ..BadgeEntry::default()
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

    /// Строка редактора бейджа (карточка): line1 = ordinal·имя·актив·код(·L/S·код-short)·
    /// цвет·превью·удалить; line2 = обводка (галка + цвета long/short при включении).
    fn badge_row(&self, cx: &Context<Self>, idx: usize, row: &BadgeRowEd) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let is_light = self.badges.is_light;
        let (active, distinguish, outline, code, color) = {
            let b = self.backend.read(cx);
            let bcfg = &b.preview.as_ref().unwrap_or(&b.config).badges;
            bcfg.entries
                .get(idx)
                .map(|e| {
                    (
                        e.active,
                        e.distinguish_dir,
                        e.outline,
                        e.code.clone(),
                        e.color(is_light),
                    )
                })
                .unwrap_or((true, false, false, "UNK".to_string(), [0x97, 0x92, 0x8A]))
        };
        let bcol = design::rgb_to_u32(color);

        let active_chk = self
            .draft_checkbox(
                cx,
                SharedString::from(format!("badge-active-{idx}")),
                active,
                move |p, v| {
                    if let Some(e) = p.badges.entries.get_mut(idx) {
                        if e.active != v {
                            e.active = v;
                            return true;
                        }
                    }
                    false
                },
            )
            .label(t!("badges.col_active").to_string())
            .size(MoonCheckboxSize::Compact);

        let distinguish_chk = self
            .draft_checkbox(
                cx,
                SharedString::from(format!("badge-dist-{idx}")),
                distinguish,
                move |p, v| {
                    if let Some(e) = p.badges.entries.get_mut(idx) {
                        if e.distinguish_dir != v {
                            e.distinguish_dir = v;
                            return true;
                        }
                    }
                    false
                },
            )
            .label(t!("badges.distinguish").to_string())
            .size(MoonCheckboxSize::Compact);

        let outline_chk = self
            .draft_checkbox(
                cx,
                SharedString::from(format!("badge-outline-{idx}")),
                outline,
                move |p, v| {
                    if let Some(e) = p.badges.entries.get_mut(idx) {
                        if e.outline != v {
                            e.outline = v;
                            return true;
                        }
                    }
                    false
                },
            )
            .label(t!("badges.use_outline").to_string())
            .size(MoonCheckboxSize::Compact);

        // Всё в ОДНУ строку. Пикеры цвета — `flex_none` в натуральную ширину (128px,
        // фикс форка), поэтому больше не налезают на соседний бейдж. Цвета обводки
        // (L=long, S=short) появляются справа при включённой галке «Обводка».
        let cap = |t: &str| {
            div()
                .flex_none()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(t.to_string())
        };
        let row_el = h_flex()
            .gap(px(6.0))
            .items_center()
            .child(
                div().flex_none().w(px(42.0)).child(
                    MoonInput::new(SharedString::from(format!("badge-ord-{idx}")))
                        .state(&row.ordinal)
                        .small(),
                ),
            )
            .child(
                div().flex_none().w(px(120.0)).child(
                    MoonInput::new(SharedString::from(format!("badge-name-{idx}")))
                        .state(&row.name)
                        .small(),
                ),
            )
            .child(active_chk)
            .child(
                div().flex_none().w(px(44.0)).child(
                    MoonInput::new(SharedString::from(format!("badge-code-{idx}")))
                        .state(&row.code)
                        .small(),
                ),
            )
            .child(distinguish_chk)
            .when(distinguish, |el| {
                el.child(
                    div().flex_none().w(px(44.0)).child(
                        MoonInput::new(SharedString::from(format!("badge-codeshort-{idx}")))
                            .state(&row.code_short)
                            .small(),
                    ),
                )
            })
            .child(div().flex_none().child(MoonColorPicker::new(&row.color)))
            .child(
                div().flex_none().w(px(42.0)).flex().justify_center().child(
                    MoonBadge::new(code)
                        .variant(MoonBadgeVariant::Soft)
                        .size(MoonBadgeSize::Status)
                        .bg_color(bcol)
                        .text_color(bcol)
                        .mono(true),
                ),
            )
            .child(outline_chk)
            .when(outline, |el| {
                el.child(cap("L"))
                    .child(div().flex_none().child(MoonColorPicker::new(&row.outline_long)))
                    .child(cap("S"))
                    .child(div().flex_none().child(MoonColorPicker::new(&row.outline_short)))
            })
            .child(
                MoonButton::new(SharedString::from(format!("badge-del-{idx}")))
                    .danger()
                    .size(MoonButtonSize::Micro)
                    .width(24.0)
                    .label("x")
                    .on_click(cx.listener(move |this, _, w, cx| this.delete_badge(idx, w, cx)))
                    .render(),
            );

        // Без `w_full` — карточка обжимает содержимое (ширина рамки зависит от строки:
        // с включённой обводкой строка шире). Список выше выравнивает по левому краю.
        div()
            .px_1()
            .py_0p5()
            .rounded(design::ui_px(cx, 4.0))
            .border_1()
            .border_color(rgba_from(p.border, 1.0))
            .child(row_el)
    }

    /// Вкладка «Бейджи»: список видов (карточка на вид) + кнопка «Добавить тип».
    pub(super) fn badges_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let mut col = v_flex()
            .w_full()
            .items_start()
            .gap_1()
            .child(div().font_bold().child(t!("badges.title").to_string()))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(t!("badges.theme_hint").to_string()),
            );
        // Индекс берём из строки (row.idx = позиция в `entries`), НЕ из enumerate: список
        // строк может быть короче entries (скрыт служебный Unknown), иначе правки уехали бы.
        for row in self.badges.rows.iter() {
            col = col.child(self.badge_row(cx, row.idx, row));
        }
        col.child(separator(p, cx)).child(
            div().mt_1().child(
                MoonButton::new("badge-add")
                    .small()
                    .width(150.0)
                    .label(t!("badges.add").to_string())
                    .on_click(cx.listener(|this, _, w, cx| this.add_badge(w, cx)))
                    .render(),
            ),
        )
    }
}
