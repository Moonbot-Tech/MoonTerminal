//! Вкладка «Общие» — личные/машинные настройки (settings.toml): тёмная/светлая тема и
//! шрифт UI, язык интерфейса (выпадающий список), отдельная чарт-вкладка на ядро, лог в
//! файлы + срок хранения. Правки идут в draft; тема/шрифт применяются живьём, остальное —
//! после «Сохранить» (язык/чарты — на перезапуске/пересборке окон).

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonCheckboxSize, MoonInput, MoonInputEvent, MoonInputState,
    MoonMenuSize, MoonPalette, MoonSelect, MoonSlider, MoonSliderEvent, MoonSliderState, MoonToggle,
    StyledExt, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{Backend, design};
use moon_core::config::UiThemeMode;

impl SettingsView {
    /// Изменить срок хранения логов (клампим 0..=365), правит draft.
    fn adjust_ret(&mut self, delta: i32, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                let v = (p.log_retention_days as i32 + delta).clamp(0, 365) as u32;
                if p.log_retention_days != v {
                    p.log_retention_days = v;
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

    /// Дефолт секунд при ВКЛючении авто-закрытия Main по неактивности (чекбокс 0↔дефолт).
    const IDLE_DEFAULT_SECS: u32 = 120;

    /// Изменить таймаут авто-закрытия Main (клампим 5..=3600), правит draft. Активно только
    /// когда фича включена (значение > 0).
    fn adjust_idle(&mut self, delta: i32, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                if p.main_idle_close_secs > 0 {
                    let v = (p.main_idle_close_secs as i32 + delta).clamp(5, 3600) as u32;
                    if p.main_idle_close_secs != v {
                        p.main_idle_close_secs = v;
                        bcx.notify();
                        changed = true;
                    }
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// Ряд степпера «<<  <  значение  >  >>»: одинарные стрелки — малый шаг,
    /// двойные — крупный (быстрый набор без «тыкать в плюсик до отсыхания»).
    /// Общий для счётчиков секунд/дней (+ лимит версий «Хранилища»); клампы —
    /// внутри `adjust`.
    pub(super) fn stepper_controls(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        enabled: bool,
        value_text: String,
        small: i32,
        large: i32,
        adjust: fn(&mut Self, i32, &mut Context<Self>),
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let color = if enabled {
            rgba_from(p.text, 1.0)
        } else {
            rgba_from(p.text_muted, 1.0)
        };
        let btn = |suffix: &'static str, label: &'static str, delta: i32| {
            MoonButton::new(SharedString::from(format!("{id}{suffix}")))
                .ghost()
                .size(MoonButtonSize::Micro)
                .width(28.0)
                .label(label)
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| adjust(this, delta, cx)))
                .render()
        };
        h_flex()
            .gap(design::ui_px(cx, 4.0))
            .items_center()
            .child(btn("-large", "<<", -large))
            .child(btn("-small", "<", -small))
            .child(
                div()
                    .w(design::font_w_px(cx, 72.0))
                    .text_center()
                    .text_color(color)
                    .child(value_text),
            )
            .child(btn("+small", ">", small))
            .child(btn("+large", ">>", large))
    }

    /// Записать `ui_font_delta` в draft + переустановить MoonUI-тему живьём (масштаб шрифтов
    /// всего UI). Возвращает, изменилось ли значение — по этому вызывающий решает, обновлять ли
    /// парный контрол (слайдер ↔ поле), не порождая лишний notify/перерисовку на no-op.
    pub(super) fn set_ui_font_delta(&mut self, v: f32, cx: &mut Context<Self>) -> bool {
        let changed = self.backend.update(cx, |b, bcx| {
            let Some(p) = b.preview.as_mut() else {
                return false;
            };
            if p.ui_font_delta == v {
                return false;
            }
            p.ui_font_delta = v;
            crate::install_moon_theme_for_config(p, bcx);
            bcx.notify();
            true
        });
        if changed {
            cx.notify();
        }
        changed
    }

    /// Контрол размера шрифта UI (вкладка «Общие»): ряд «колонка ползунок+линейка целочисленных
    /// меток» и числовое поле точного ввода, БЕЗ отдельной подписи — параметр описывает хинт
    /// снизу (единообразно с остальными настройками: контрол, под ним пояснение; ползунок стоит у
    /// левого края, как прочие контролы). Ползунок и метки живут в одной колонке ширины `track_w`,
    /// поэтому засечки совпадают с центром бегунка.
    pub(super) fn font_delta_control(&self, cx: &Context<Self>) -> impl IntoElement {
        let track_w = design::ui_value(cx, 210.0);
        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                v_flex()
                    .w(px(track_w))
                    .gap(design::ui_px(cx, 2.0))
                    .child(
                        div().w(px(track_w)).child(
                            MoonSlider::new(&self.ui_font).height(design::ui_value(cx, 22.0)),
                        ),
                    )
                    .child(font_delta_marks(cx, track_w)),
            )
            .child(
                div().w(design::font_w_px(cx, 56.0)).child(
                    MoonInput::new("ui-font-delta")
                        .state(&self.ui_font_input)
                        .small(),
                ),
            )
    }

    /// Вкладка «Общие» — порт egui `settings/general.rs` точь-в-точь: язык (выпадающий
    /// список) + хинт; разделитель; чекбокс «чарт-вкладка на ядро» + хинт; разделитель;
    /// чекбокс «писать лог в файлы» + хинт; срок хранения (число) + хинт.
    pub(super) fn general_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let (ui_theme_mode, split, scz, idle_secs, logf, ret) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.ui_theme_mode,
                d.charts_split_by_core,
                d.separate_control_zones,
                d.main_idle_close_secs,
                d.log_to_file,
                d.log_retention_days,
            )
        };
        let hint = |s: &str| div().text_color(muted).child(s.to_string());

        v_flex()
            .w_full()
            .gap_1()
            // Тёмная/светлая тема UI + шрифт UI — личные настройки (settings.toml, не
            // переносимая тема чарта — та на вкладке «Интерфейс» = theme.toml).
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
            .child(hint(&t!("iface.light_theme_hint")))
            .child(self.font_delta_control(cx))
            .child(hint(&t!("iface.font_delta_hint")))
            .child(super::separator(p, cx))
            // Язык интерфейса — выпадающий список.
            .child(
                h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(div().font_bold().child(t!("general.language").to_string()))
                    .child(
                        div().w(px(220.0)).child(
                            MoonSelect::new(&self.lang)
                                .trigger_size(MoonButtonSize::Action)
                                .menu_width(design::font_w(cx, 220.0))
                                .menu_size(MoonMenuSize::Compact),
                        ),
                    ),
            )
            .child(hint(&t!("general.language_hint")))
            .child(super::separator(p, cx))
            // Отдельная чарт-вкладка на каждое ядро.
            .child(
                self.draft_checkbox(cx, "split", split, |p, v| {
                    if p.charts_split_by_core != v {
                        p.charts_split_by_core = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.charts_split_by_core").to_string())
                .size(MoonCheckboxSize::Normal),
            )
            .child(hint(&t!("general.charts_split_by_core_hint")))
            .child(super::separator(p, cx))
            // Раздельные зоны управления: ордера/линии только в зоне стакана.
            .child(
                self.draft_checkbox(cx, "separate-zones", scz, |p, v| {
                    if p.separate_control_zones != v {
                        p.separate_control_zones = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.separate_control_zones").to_string())
                .size(MoonCheckboxSize::Normal),
            )
            .child(hint(&t!("general.separate_control_zones_hint")))
            .child(super::separator(p, cx))
            // Авто-закрытие графиков Main при неактивности окна (чекбокс 0↔дефолт + счётчик сек).
            .child(
                self.draft_checkbox(cx, "idle-close", idle_secs > 0, |p, v| {
                    let want = if v { Self::IDLE_DEFAULT_SECS } else { 0 };
                    // Вкл при уже выставленном значении не сбрасываем; выкл = 0.
                    let target = if v && p.main_idle_close_secs > 0 {
                        p.main_idle_close_secs
                    } else {
                        want
                    };
                    if p.main_idle_close_secs != target {
                        p.main_idle_close_secs = target;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.main_idle_close").to_string())
                .size(MoonCheckboxSize::Normal),
            )
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(if idle_secs > 0 {
                                rgba_from(p.text, 1.0)
                            } else {
                                muted
                            })
                            .child(t!("general.main_idle_close_secs").to_string()),
                    )
                    .child(self.stepper_controls(
                        cx,
                        "idle",
                        idle_secs > 0,
                        format!("{idle_secs} {}", t!("general.seconds")),
                        10,
                        100,
                        Self::adjust_idle,
                    )),
            )
            .child(hint(&t!("general.main_idle_close_hint")))
            .child(super::separator(p, cx))
            // Раскладка стека (FIT/SCROLL/COMPRESS + высота) теперь per-вкладка — кнопка ⚙
            // в полоске вкладок / шапке выносного окна (см. chart_tabs::layout_popup).
            // Логи в файлы + срок хранения.
            .child(
                self.draft_checkbox(cx, "logf", logf, |p, v| {
                    if p.log_to_file != v {
                        p.log_to_file = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.log_to_file").to_string())
                .size(MoonCheckboxSize::Normal),
            )
            .child(hint(&t!("general.log_to_file_hint")))
            // Срок хранения активен только при включённой записи лога (порт
            // egui `add_enabled_ui(cfg.log_to_file, ...)`): кнопки −/+ задизейблены,
            // значение/подписи тусклые, пока «Писать лог в файлы» выключено.
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(if logf { rgba_from(p.text, 1.0) } else { muted })
                            .child(t!("general.log_retention").to_string()),
                    )
                    .child(self.stepper_controls(
                        cx,
                        "ret",
                        logf,
                        format!("{ret} {}", t!("general.days")),
                        1,
                        10,
                        Self::adjust_ret,
                    )),
            )
            .child(hint(&t!("general.log_retention_hint")))
    }
}

/// Диапазон прибавки к размеру шрифта UI (logical px, целый шаг 1.0). Единый источник и для
/// слайдера, и для клампа ввода, и для меток линейки.
const FONT_DELTA_MIN: i32 = -2;
const FONT_DELTA_MAX: i32 = 6;

/// Каноничный текст значения прибавки (целый шаг): "-2", "0", "3". `round` перед `as i32` —
/// на случай накопленной погрешности f32; заодно даёт "0" вместо "-0".
fn font_delta_text(v: f32) -> String {
    (v.round() as i32).to_string()
}

/// Разбор ввода поля: запятая как точка, пробелы обрезаем. `None` — для пустого/незавершённого
/// ("", "-") или нечисла; NaN/Inf тоже отвергаем ДО округления (иначе пролезли бы в конфиг).
/// Валидное округляем к шагу, клампим в [MIN, MAX] и нормализуем IEEE `-0.0` → `0.0`.
fn parse_font_delta(s: &str) -> Option<f32> {
    let v: f32 = s.trim().replace(',', ".").parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    let v = v.round().clamp(FONT_DELTA_MIN as f32, FONT_DELTA_MAX as f32);
    Some(if v == 0.0 { 0.0 } else { v })
}

/// Линейка меток под ползунком размера шрифта: тонкая засечка на каждом целом [MIN..MAX],
/// числовая подпись на чётных. Позиция метки — доля `(m-MIN)/span` от `track_w`, чтобы совпадать
/// с центром бегунка (бар слайдера — во всю ширину той же колонки). Крайние подписи (MIN и MAX)
/// прижаты к краям трека, а не центрированы: иначе половина цифры вылезала бы за трек — левая
/// обрезалась бы, правая налезала на поле ввода.
fn font_delta_marks(cx: &App, track_w: f32) -> impl IntoElement {
    let span = (FONT_DELTA_MAX - FONT_DELTA_MIN) as f32;
    let p = MoonPalette::active(cx);
    let tick = rgba_from(p.border, 1.0);
    let label = rgba_from(p.text_muted, 1.0);
    let tick_h = design::ui_value(cx, 4.0);
    let label_w = design::font_w(cx, 20.0);
    let mut row = div()
        .relative()
        .w(px(track_w))
        .h(design::fit_h_px(cx, 16.0, 11.0, 1.0));
    for m in FONT_DELTA_MIN..=FONT_DELTA_MAX {
        let x = (m - FONT_DELTA_MIN) as f32 / span * track_w;
        row = row.child(
            div()
                .absolute()
                .left(px((x - 0.5).max(0.0)))
                .top(px(0.0))
                .w(px(1.0))
                .h(px(tick_h))
                .bg(tick),
        );
        if m % 2 == 0 {
            let left = if m == FONT_DELTA_MIN {
                0.0
            } else if m == FONT_DELTA_MAX {
                track_w - label_w
            } else {
                x - label_w / 2.0
            };
            row = row.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(tick_h + design::ui_value(cx, 1.0)))
                    .w(px(label_w))
                    .text_center()
                    .text_size(design::t_caption(cx))
                    .text_color(label)
                    .child(m.to_string()),
            );
        }
    }
    row
}

/// Собрать контрол размера шрифта UI: ползунок + числовое поле, двусторонне синхронные.
/// Диапазон — от −2 до 6 logical px с целым шагом 1.0; дефолт настройки — +2.
/// Зовётся из [`SettingsView::new`]. Обе подписки — через `subscribe_in` (нужен `&mut Window`
/// для `set_value` парного контрола). Петли нет: `set_value` слайдера только нотифаит, у поля —
/// c `emit_events = false`, так что ни один не порождает встречное событие.
pub(super) fn build_font(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> (Entity<MoonSliderState>, Entity<MoonInputState>) {
    let cur = {
        let b = backend.read(cx);
        b.preview.as_ref().unwrap_or(&b.config).ui_font_delta
    };
    let slider = cx.new(|_| {
        MoonSliderState::new()
            .min(FONT_DELTA_MIN as f32)
            .max(FONT_DELTA_MAX as f32)
            .step(1.0)
            .default_value(cur)
    });
    let input = cx.new(|cx| MoonInputState::new(window, cx).default_value(font_delta_text(cur)));

    // Ползунок → конфиг, зеркалим число в поле. Замыкание намеренно не захватывает ничего
    // внешнего: парное поле берёт через `this.ui_font_input`. Детач-подписка удерживается своим
    // эмиттером, поэтому сильный захват input из подписки slider создал бы взаимный цикл удержания.
    cx.subscribe_in(
        &slider,
        window,
        move |this, _slider, ev: &MoonSliderEvent, window, cx| {
            let MoonSliderEvent::Change(v) = ev else {
                return;
            };
            // Квантование на отрицательном поддиапазоне даёт IEEE -0.0 — нормализуем.
            let v = v.end();
            let v = if v == 0.0 { 0.0 } else { v };
            if this.set_ui_font_delta(v, cx) {
                this.ui_font_input
                    .update(cx, |st, c| st.set_value(font_delta_text(v), window, c));
            }
        },
    )
    .detach();

    // Поле → конфиг, двигаем бегунок. На `Change` текст НЕ переписываем (не мешаем набору);
    // на `Blur`/`Enter` нормализуем текст к канону (или к текущему значению, если ввод — мусор).
    // Это замыкание тоже не захватывает ничего внешнего: своё поле получает параметром-эмиттером
    // `field`, а парный бегунок — через `this.ui_font`; сильный захват slider создал бы тот же цикл.
    cx.subscribe_in(
        &input,
        window,
        move |this, field, ev: &MoonInputEvent, window, cx| match ev {
            MoonInputEvent::Change => {
                // Отдельным `let` — чтобы immutable-borrow `cx` полем закрылся до `set_ui_font_delta`.
                let parsed = parse_font_delta(&field.read(cx).value());
                if let Some(v) = parsed {
                    if this.set_ui_font_delta(v, cx) {
                        this.ui_font
                            .update(cx, |st, c| st.set_value(v, window, c));
                    }
                }
            }
            MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. } => {
                let cur = {
                    let b = this.backend.read(cx);
                    b.preview.as_ref().unwrap_or(&b.config).ui_font_delta
                };
                field.update(cx, |st, c| st.set_value(font_delta_text(cur), window, c));
            }
            _ => {}
        },
    )
    .detach();

    (slider, input)
}

#[cfg(test)]
mod tests {
    use super::{font_delta_text, parse_font_delta};

    #[test]
    fn text_is_integer_canonical() {
        assert_eq!(font_delta_text(2.0), "2");
        assert_eq!(font_delta_text(-2.0), "-2");
        assert_eq!(font_delta_text(0.0), "0");
        assert_eq!(font_delta_text(-0.0), "0"); // без «минус-нуля»
        assert_eq!(font_delta_text(2.6), "3"); // округление к целому
        assert_eq!(font_delta_text(2.4), "2");
    }

    #[test]
    fn parse_rounds_and_clamps() {
        assert_eq!(parse_font_delta("3"), Some(3.0));
        assert_eq!(parse_font_delta("2.6"), Some(3.0)); // округление к шагу
        assert_eq!(parse_font_delta("2.4"), Some(2.0));
        assert_eq!(parse_font_delta("2,4"), Some(2.0)); // запятая как точка
        assert_eq!(parse_font_delta("+4"), Some(4.0));
        assert_eq!(parse_font_delta("  5  "), Some(5.0)); // trim
        assert_eq!(parse_font_delta("10"), Some(6.0)); // кламп сверху
        assert_eq!(parse_font_delta("-9"), Some(-2.0)); // кламп снизу
    }

    #[test]
    fn parse_rejects_garbage_and_nonfinite() {
        assert_eq!(parse_font_delta(""), None);
        assert_eq!(parse_font_delta("-"), None); // незавершённый ввод
        assert_eq!(parse_font_delta("abc"), None);
        assert_eq!(parse_font_delta("nan"), None); // NaN не должен уехать в конфиг
        assert_eq!(parse_font_delta("inf"), None); // Inf тоже
    }

    #[test]
    fn parse_normalizes_negative_zero() {
        // «-0» → канонический +0.0 (иначе IEEE «минус-ноль» уехал бы в settings.toml).
        let v = parse_font_delta("-0").unwrap();
        assert_eq!(v, 0.0);
        assert!(v.is_sign_positive());
    }
}
