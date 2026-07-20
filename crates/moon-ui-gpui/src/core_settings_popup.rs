//! Контент попапа «настройки ядра» (кнопка ⚙ рядом с селектором ядра в шапке). Чистый рендер:
//! чекбоксы/кнопки строят замыкания через `backend` сами (как `controls::metric_popup_content`),
//! числовые поля (глобальный TP / трейлинг) — персистентные сущности Shell с коммитом по Blur/Enter.
//! Хостинг (overlay+dismiss, позиция, сид полей, confirm cancel-all) — в `shell/core_settings.rs`.
//!
//! Все правки идут на `active_trade_core(group)` — то же ядро, что селектор/тулбар. Нет ядра/
//! снимка → прочерк-заглушка.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonInput,
    MoonInputState, MoonPalette, MoonSlider, MoonSliderState, MoonTextArea, MoonTooltipView,
    h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use moon_core::feed::{ClientSettingsEdit, LevManageEdit, RuntimeState};
use moon_core::session::CoreId;

use crate::{Backend, design};

/// Ordinal вида стратегии «Alerts» (moonproto `StrategyKindId::ALERTS = 22`).
const ALERTS_KIND: u8 = 22;

/// Ряд «Стратегия алертов по умолчанию» (Def Strategy) активного ядра: выпадашка
/// стратегий вида «Alerts» ЭТОГО ядра. Выбор пишет `Backend::default_alert_strategy[core]`,
/// который применяется к новому алерту при постановке галки Alert.
fn def_alert_strategy_row(
    core: Option<CoreId>,
    filter_input: &Entity<MoonInputState>,
    backend: &Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    let core = core?;
    let b = backend.read(cx);
    let cur = b.alert_def_strategy(core);
    let filter = filter_input.read(cx).value().trim().to_lowercase();
    // Стратегии вида «Alerts» этого ядра, отфильтрованные по поиску. «—» (без стратегии)
    // всегда первым и не фильтруется.
    let mut options: Vec<(u64, String)> = vec![(0u64, "—".to_string())];
    options.extend(
        b.session
            .store()
            .core(core)?
            .strategies
            .iter()
            .filter(|s| s.kind_ordinal == ALERTS_KIND)
            .filter(|s| filter.is_empty() || s.name.to_lowercase().contains(&filter))
            .map(|s| (s.id, s.name.clone())),
    );
    // Инлайн (не MoonDropdown): вложенное меню-оверлей внутри MoonPopover ловится попапом
    // как «клик снаружи» и закрывает его до выбора. Поиск + скролл фикс. высоты держат
    // список компактным даже при сотнях стратегий; клики — часть контента попапа.
    let mut list = v_flex().w_full().gap(design::ui_px(cx, 2.0));
    for (id, name) in options {
        let selected = id == cur;
        let backend2 = backend.clone();
        list = list.child(
            h_flex()
                .id(SharedString::from(format!("def-strat-{id}")))
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .px(design::ui_px(cx, 6.0))
                .py(design::ui_px(cx, 3.0))
                .rounded(design::r_button(cx))
                .cursor_pointer()
                .when(selected, |e| e.bg(rgba_from(p.accent, 0.16)))
                .hover(|e| e.bg(rgba_from(p.text, 0.06)))
                .child(
                    div()
                        .w(design::ui_px(cx, 12.0))
                        .text_color(rgb(p.accent))
                        .child(if selected { "✓" } else { "" }),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(if selected { p.text } else { p.text_soft }))
                        .child(name),
                )
                .on_click(move |_, _w, app| {
                    backend2.update(app, |bk, bcx| {
                        bk.set_alert_def_strategy(core, id);
                        bcx.notify();
                    });
                }),
        );
    }
    Some(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("core_settings.def_strategy").to_string()),
            )
            .child(
                MoonInput::new("core-def-strategy-filter")
                    .state(filter_input)
                    .small(),
            )
            .child(
                div()
                    .id("core-def-strategy-list")
                    .w_full()
                    .max_h(design::ui_px(cx, 150.0))
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element(),
    )
}

/// Unscaled content width shared with the popover host.
///
/// [`core_settings_content`] applies the font scale to this value; the terminal chrome uses the
/// same scaled width when sizing the surrounding `MoonPopover`.
/// 268: вмещает шапку «заголовок + Запущен/Автодетект» в EN/RU без обрезания (ES режет truncate).
pub const CONTENT_W: f32 = 268.0;

/// Границы слайдеров параметров «галка + слайдер + поле» (min, max, шаг).
/// ТП-глоб = g_take_profit (плюс), трейлинг = trailing_drop (минус). Стоп-лосс вынесен в тулбар.
pub const CORE_GTP_BOUNDS: (f32, f32, f32) = (0.5, 10.0, 0.1);
pub const CORE_TRAILING_BOUNDS: (f32, f32, f32) = (-10.0, -0.1, 0.1);
/// V-Stop (vol_drop_level, целое %): уровень падения объёма BID, отрицательный.
pub const CORE_VSTOP_BOUNDS: (f32, f32, f32) = (-50.0, 0.0, 1.0);

/// Чекбокс правки `ClientSettings` активного ядра. `edit` — конструктор варианта `Variant(bool)`.
fn cs_checkbox(
    id: &str,
    label: String,
    checked: bool,
    backend: &Entity<Backend>,
    group: &str,
    edit: fn(bool) -> ClientSettingsEdit,
) -> impl IntoElement {
    let backend = backend.clone();
    let group = group.to_string();
    MoonCheckbox::new(SharedString::from(id.to_string()))
        .label(label)
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            let b = backend.read(app);
            if let Some(core) = b.active_trade_core(&group) {
                if let Err(e) = b.session.edit_client_settings(core, edit(on)) {
                    log::warn!("core settings edit failed: {e:#}");
                }
            }
        })
}

/// Чекбокс правки `LevManage` активного ядра. `edit` — конструктор варианта `Variant(bool)`.
fn lev_checkbox(
    id: &str,
    label: String,
    checked: bool,
    backend: &Entity<Backend>,
    group: &str,
    edit: fn(bool) -> LevManageEdit,
) -> impl IntoElement {
    let backend = backend.clone();
    let group = group.to_string();
    MoonCheckbox::new(SharedString::from(id.to_string()))
        .label(label)
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            let b = backend.read(app);
            if let Some(core) = b.active_trade_core(&group) {
                if let Err(e) = b.session.edit_lev_manage(core, edit(on)) {
                    log::warn!("core lev edit failed: {e:#}");
                }
            }
        })
}

/// Рамка-группа: тонкая граница + капшен-заголовок сверху (как в `chart_tabs/layout_popup`).
fn framed(title: String, p: MoonPalette, cx: &App, body: AnyElement) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .px(design::ui_px(cx, 6.0))
        .py(design::ui_px(cx, 4.0))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_button(cx))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(title),
        )
        .child(body)
}

/// Галка-иконка (без подписи) с всплывающей подсказкой «вкл/выкл». Обёртка `div.id.tooltip`
/// над `MoonCheckbox` без label (у самого чекбокса тултипа нет).
fn icon_checkbox(
    id: &str,
    tooltip: String,
    checked: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("{id}-tip")))
        .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tooltip.clone())).into())
        .child(
            MoonCheckbox::new(SharedString::from(id.to_string()))
                .checked(checked)
                .size(MoonCheckboxSize::Compact)
                .on_change(on_change),
        )
        .into_any_element()
}

/// Параметр «галка + слайдер + поле»: заголовок сверху, ниже строка [галка-иконка][слайдер][поле].
/// Коммит слайдера/поля держит Shell (подписки); галку (вкл/выкл) задаёт `checkbox`.
#[allow(clippy::too_many_arguments)]
fn param_row(
    title: String,
    checkbox: AnyElement,
    slider: &Entity<MoonSliderState>,
    slider_id: &str,
    input: &Entity<MoonInputState>,
    input_id: &str,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 3.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(title),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(checkbox)
                .child(
                    div().flex_1().child(
                        MoonSlider::new(slider)
                            .id(slider_id.to_string())
                            .height(18.0),
                    ),
                )
                .child(
                    div().w(design::font_w_px(cx, 56.0)).child(
                        MoonInput::new(SharedString::from(input_id.to_string()))
                            .state(input)
                            .small(),
                    ),
                ),
        )
}

/// Контент попапа настроек ядра. `gtp_input`/`trailing_input` — Shell-сущности (коммит держит
/// Shell). `cancel_confirm` — стадия подтверждения «Отменить все ордера». `on_cancel_all` —
/// колбэк Shell: первый клик ставит confirm, второй (когда confirm=true) шлёт команду.
#[allow(clippy::too_many_arguments)]
pub fn core_settings_content(
    gtp_slider: &Entity<MoonSliderState>,
    trailing_slider: &Entity<MoonSliderState>,
    vstop_slider: &Entity<MoonSliderState>,
    gtp_input: &Entity<MoonInputState>,
    trailing_input: &Entity<MoonInputState>,
    vstop_input: &Entity<MoonInputState>,
    blacklist_input: &Entity<MoonInputState>,
    blacklist_area: &Entity<MoonInputState>,
    def_strategy_input: &Entity<MoonInputState>,
    blacklist_expanded: bool,
    cancel_confirm: bool,
    backend: &Entity<Backend>,
    group: &str,
    p: MoonPalette,
    cx: &App,
    on_cancel_all: impl Fn(&mut App) + 'static,
    on_toggle_blacklist: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let b = backend.read(cx);
    let core = b.active_trade_core(group);
    let cd = core.and_then(|c| b.session.store().core(c));
    let cs = cd.and_then(|d| d.client_settings.clone());
    let lm = cd.and_then(|d| d.lev_manage.clone());
    // Состояние рантайма активного ядра — 2 точки (запущен / авто-детект), перенесены сюда из
    // шапки (рядом с кнопкой ⚙ было тесно и без подписей). Здесь подписаны.
    let rt = cd.and_then(|d| d.runtime_state);

    // Фон/рамку/внешний паддинг даёт MoonPopover (хостится у кнопки ⚙ в шапке) — здесь
    // только контент фикс. ширины.
    let root = v_flex()
        .id("core-settings-popup")
        .w(design::font_w_px(cx, CONTENT_W))
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    // flex_1+min_w_0+truncate: заголовок ужимается, иначе строка шире CONTENT_W
                    // и подписи статуса вылезают за правый край попапа.
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("core_settings.title").to_string()),
                )
                .child(runtime_status(rt, p, cx)),
        );

    // Нет ядра/снимка настроек — заглушка.
    let Some(cs) = cs else {
        return root
            .child(
                div()
                    .text_color(rgb(p.text_muted))
                    .child(t!("core_settings.no_core").to_string()),
            )
            .into_any_element();
    };

    // ── Шапка: Старт/Рестарт + эмулятор ──────────────────────────────────
    let restart_btn = {
        let backend = backend.clone();
        let group = group.to_string();
        // Action-кнопка (Size::Small) идёт с pad_x=0 — фон ровно по тексту. Пробелы в label —
        // единственный способ дать паре пикселей полей без правки форка (у MoonButton нет pad_x).
        MoonButton::new("core-restart")
            .label(format!(" {} ", t!("core_settings.restart")))
            .size(MoonButtonSize::Action)
            .variant(MoonButtonVariant::Blue)
            .on_click(move |_, _w, app| {
                let b = backend.read(app);
                if let Some(core) = b.active_trade_core(&group) {
                    if let Err(e) = b.session.restart_now(core) {
                        log::warn!("restart_now failed: {e:#}");
                    }
                }
            })
            .render()
    };
    let emu_check = cs_checkbox(
        "core-emu",
        t!("core_settings.emu").to_string(),
        cs.emu_mode,
        backend,
        group,
        ClientSettingsEdit::EmuMode,
    );
    let header_row = v_flex()
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(restart_btn)
                .child(div().flex_1())
                .child(emu_check),
        )
        // Заметная плашка-предупреждение, когда включён режим эмулятора.
        .when(cs.emu_mode, |this| {
            this.child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 6.0))
                    .py(design::ui_px(cx, 3.0))
                    .rounded(design::r_button(cx))
                    .bg(rgba_from(p.amber, 0.18))
                    .border_1()
                    .border_color(rgb(p.amber))
                    .text_color(rgb(p.amber))
                    .text_size(design::t_caption(cx))
                    .child(t!("core_settings.emu_on").to_string()),
            )
        });

    // ── Рамка «Дефолты поведения» ────────────────────────────────────────
    // Стоп-лосс/паника вынесены в тулбар (тогл рядом с кнопкой SL) — здесь их нет.
    // Глобальный TP: галка `use_g_take_profit` + значение `g_take_profit`.
    let gtp_cb = {
        let backend = backend.clone();
        let group = group.to_string();
        let pct = cs.global_take_profit_pct;
        icon_checkbox(
            "core-gtp-cb",
            t!("core_settings.toggle_tip").to_string(),
            cs.use_global_take_profit,
            move |ch, _w, app| {
                let on = *ch;
                let b = backend.read(app);
                if let Some(core) = b.active_trade_core(&group) {
                    if let Err(e) = b.session.edit_client_settings(
                        core,
                        ClientSettingsEdit::GlobalTakeProfit { on, pct },
                    ) {
                        log::warn!("global tp toggle failed: {e:#}");
                    }
                }
            },
        )
    };
    // Трейлинг: флага на проводе нет → галка = «значение ≠ 0». Снятие шлёт 0; включение берёт
    // текущее значение слайдера (или дефолт −1.0). Само значение правит слайдер/поле.
    let trailing_cb = {
        let backend = backend.clone();
        let group = group.to_string();
        let cur = cs.trailing_drop_pct;
        let slider = trailing_slider.clone();
        icon_checkbox(
            "core-trailing-cb",
            t!("core_settings.toggle_tip").to_string(),
            cur.abs() > 1e-6,
            move |ch, _w, app| {
                let on = *ch;
                let val = if on {
                    if cur.abs() > 1e-6 {
                        cur
                    } else {
                        let s = slider.read(app).value().end();
                        if s.abs() > 1e-6 { s } else { -1.0 }
                    }
                } else {
                    0.0
                };
                let b = backend.read(app);
                if let Some(core) = b.active_trade_core(&group) {
                    if let Err(e) = b
                        .session
                        .edit_client_settings(core, ClientSettingsEdit::TrailingDrop(val))
                    {
                        log::warn!("trailing toggle failed: {e:#}");
                    }
                }
            },
        )
    };
    // V-Stop: флага на проводе нет → галка = «значение ≠ 0». Значение правит слайдер/поле
    // (целое %). Включение из 0 берёт значение слайдера или дефолт −2.
    let vstop_cb = {
        let backend = backend.clone();
        let group = group.to_string();
        let cur = cs.vol_drop_level;
        let slider = vstop_slider.clone();
        icon_checkbox(
            "core-vstop-cb",
            t!("core_settings.toggle_tip").to_string(),
            cur != 0,
            move |ch, _w, app| {
                let on = *ch;
                let n = if on {
                    if cur != 0 {
                        cur
                    } else {
                        let s = slider.read(app).value().end().round() as i32;
                        if s != 0 { s } else { -2 }
                    }
                } else {
                    0
                };
                let b = backend.read(app);
                if let Some(core) = b.active_trade_core(&group) {
                    if let Err(e) = b
                        .session
                        .edit_client_settings(core, ClientSettingsEdit::VolDropLevel(n))
                    {
                        log::warn!("vstop toggle failed: {e:#}");
                    }
                }
            },
        )
    };
    let defaults = framed(
        t!("core_settings.frame_defaults").to_string(),
        p,
        cx,
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .child(param_row(
                t!("core_settings.global_tp").to_string(),
                gtp_cb,
                gtp_slider,
                "core-gtp-slider",
                gtp_input,
                "core-gtp-input",
                p,
                cx,
            ))
            .child(param_row(
                t!("core_settings.trailing").to_string(),
                trailing_cb,
                trailing_slider,
                "core-trailing-slider",
                trailing_input,
                "core-trailing-input",
                p,
                cx,
            ))
            .child(param_row(
                t!("core_settings.vstop").to_string(),
                vstop_cb,
                vstop_slider,
                "core-vstop-slider",
                vstop_input,
                "core-vstop-input",
                p,
                cx,
            ))
            .child(cs_checkbox(
                "core-buy-iceberg",
                t!("core_settings.buy_iceberg").to_string(),
                cs.buy_iceberg,
                backend,
                group,
                ClientSettingsEdit::BuyIceberg,
            ))
            .child(cs_checkbox(
                "core-sell-iceberg",
                t!("core_settings.sell_iceberg").to_string(),
                cs.sell_iceberg,
                backend,
                group,
                ClientSettingsEdit::SellIceberg,
            ))
            .into_any_element(),
    );

    // ── Рамка «Ограничение рисков»: чёрный список монет ───────────────────
    // Галка `use_coins_black_list` + текст `coins_black_list_text` (общий CoreCmd::SetBlacklist) +
    // локальная галка «исключить из дельт» (нет read-back от ядра → состояние в Backend).
    let bl_check = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonCheckbox::new("core-bl")
            .label(t!("core_settings.blacklist").to_string())
            .checked(cs.use_blacklist)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let on = *ch;
                let b = backend.read(app);
                if let Some(core) = b.active_trade_core(&group) {
                    let text = b
                        .session
                        .store()
                        .core(core)
                        .and_then(|d| d.client_settings.as_ref())
                        .map(|s| s.blacklist_text.clone())
                        .unwrap_or_default();
                    if let Err(e) = b.session.set_blacklist(core, on, text) {
                        log::warn!("blacklist toggle failed: {e:#}");
                    }
                }
            })
    };
    let exclude_on = core.map(|c| b.exclude_bl_delta(c)).unwrap_or(false);
    let exclude_check = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonCheckbox::new("core-bl-exclude")
            .label(t!("core_settings.exclude_delta").to_string())
            .checked(exclude_on)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let on = *ch;
                let core = backend.read(app).active_trade_core(&group);
                if let Some(core) = core {
                    backend.update(app, |bk, _| bk.set_exclude_bl_delta(core, on));
                    if let Err(e) = backend
                        .read(app)
                        .session
                        .set_exclude_blacklisted_delta(core, on)
                    {
                        log::warn!("exclude delta failed: {e:#}");
                    }
                }
            })
    };
    // Кнопка «…» — развернуть/свернуть поле списка монет. Свёрнуто поле в одну строку
    // (длинный список прячется), развёрнуто — многострочный редактор фикс. высоты со
    // скроллом (не растягивает попап).
    let bl_expand_btn = MoonButton::new("core-bl-expand")
        .label("…")
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Soft)
        .selected(blacklist_expanded)
        .on_click(move |_, w, app| on_toggle_blacklist(w, app))
        .render();
    // Свёрнуто — однострочный MoonInput (длинный хвост списка скрыт, поле НЕ растягивается).
    // Развёрнуто — MoonTextArea на ОТДЕЛЬНОМ multi-line стейте `blacklist_area` (общий стейт
    // нельзя: textarea необратимо переводит его в multi-line, и однострочное поле после
    // сворачивания рендерится узкой полоской). Текст между стейтами синкает Shell в тогле «…»;
    // коммит Blur/Enter подписан на оба. `submit_on_enter` — Enter коммитит, а не вставляет
    // перенос (список монет — одна строка).
    // NB: высота textarea только дефолтная Normal (~3 строки со скроллом) —
    // `MoonTextAreaSize::Custom` не реэкспортирован из moonui (см. FORK_BUGS).
    let bl_field: AnyElement = if blacklist_expanded {
        MoonTextArea::new("core-bl-area")
            .state(blacklist_area)
            .submit_on_enter(true)
            .mono(true)
            .into_any_element()
    } else {
        MoonInput::new("core-bl-text")
            .state(blacklist_input)
            .small()
            .into_any_element()
    };
    let risks = framed(
        t!("core_settings.frame_risks").to_string(),
        p,
        cx,
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(bl_check)
                    .child(div().flex_1())
                    .child(bl_expand_btn),
            )
            .child(div().w_full().child(bl_field))
            .child(exclude_check)
            .into_any_element(),
    );

    // ── Рамка «Плечо / маржа» (LevManage) ────────────────────────────────
    let (lev_max, lev_up, lev_iso, lev_cross, lev_tlg) = lm
        .as_ref()
        .map(|l| {
            (
                l.auto_max_order,
                l.auto_lev_up,
                l.auto_isolated,
                l.auto_cross,
                l.tlg_report,
            )
        })
        .unwrap_or((false, false, false, false, false));
    let leverage = framed(
        t!("core_settings.frame_leverage").to_string(),
        p,
        cx,
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .child(lev_checkbox(
                "core-auto-max",
                t!("core_settings.auto_max_order").to_string(),
                lev_max,
                backend,
                group,
                LevManageEdit::AutoMaxOrder,
            ))
            .child(lev_checkbox(
                "core-auto-levup",
                t!("core_settings.auto_lev_up").to_string(),
                lev_up,
                backend,
                group,
                LevManageEdit::AutoLevUp,
            ))
            .child(lev_checkbox(
                "core-isolated",
                t!("core_settings.isolated").to_string(),
                lev_iso,
                backend,
                group,
                LevManageEdit::AutoIsolated,
            ))
            .child(lev_checkbox(
                "core-cross",
                t!("core_settings.cross").to_string(),
                lev_cross,
                backend,
                group,
                LevManageEdit::AutoCross,
            ))
            .child(lev_checkbox(
                "core-tlg",
                t!("core_settings.tlg_report").to_string(),
                lev_tlg,
                backend,
                group,
                LevManageEdit::TlgReport,
            ))
            .into_any_element(),
    );

    // ── Рамка «Действия» ─────────────────────────────────────────────────
    // Кнопки «Сброс сессии»/«Сброс всего» убраны: TResetProfitCommand сбрасывает серверные
    // счётчики RepForm, которые клиенту не транслируются — эффект в терминале не виден
    // (трансляция счётчиков запрошена у авторов moonproto). Вернуть, когда протокол
    // начнёт их отдавать.
    let cancel_all = MoonButton::new("core-cancel-all")
        .label(if cancel_confirm {
            t!("core_settings.cancel_all_confirm").to_string()
        } else {
            t!("core_settings.cancel_all").to_string()
        })
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Danger)
        .selected(cancel_confirm)
        .full_width()
        .on_click(move |_, _w, app| on_cancel_all(app))
        .render();
    let actions = framed(
        t!("core_settings.frame_actions").to_string(),
        p,
        cx,
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .children(def_alert_strategy_row(
                core,
                def_strategy_input,
                backend,
                p,
                cx,
            ))
            .child(cancel_all)
            .into_any_element(),
    );

    root.child(header_row)
        .child(defaults)
        .child(risks)
        .child(leverage)
        .child(actions)
        .into_any_element()
}

/// Две подписанные точки состояния рантайма активного ядра: «Запущен» (`is_started`) и
/// «Автодетект» (`auto_detect_active`). Зелёный = вкл; запущен-но-passive автодетект → янтарный;
/// иначе серый. Перенесено из шапки (были без подписей рядом с ⚙).
fn runtime_status(rt: Option<RuntimeState>, p: MoonPalette, cx: &App) -> impl IntoElement {
    let ok = if p.is_light() { p.green_text } else { p.green };
    let started = rt.map(|r| r.is_started).unwrap_or(false);
    let auto = rt.map(|r| r.auto_detect_active).unwrap_or(false);
    let started_color = if started { ok } else { p.text_muted };
    let auto_color = if auto {
        ok
    } else if started {
        p.amber
    } else {
        p.text_muted
    };
    let labeled = |color: u32, label: String, cx: &App| {
        h_flex()
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .child(design::status_dot(color, cx))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_soft))
                    .child(label),
            )
    };
    h_flex()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .child(labeled(
            started_color,
            t!("core_settings.runtime_started").to_string(),
            cx,
        ))
        .child(labeled(
            auto_color,
            t!("core_settings.runtime_auto").to_string(),
            cx,
        ))
}
