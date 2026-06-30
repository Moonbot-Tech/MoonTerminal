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
    MoonInputState, MoonPalette, MoonSlider, MoonSliderState, MoonTooltipView, h_flex, rgba_from,
    v_flex,
};
use rust_i18n::t;

use moon_core::feed::{ClientSettingsEdit, LevManageEdit, ResetProfitKind};

use crate::{Backend, design};

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
        .rounded(design::ui_px(cx, 4.0))
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
                    div()
                        .flex_1()
                        .child(MoonSlider::new(slider).id(slider_id.to_string()).height(18.0)),
                )
                .child(
                    div().w(px(56.0)).child(
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
    cancel_confirm: bool,
    backend: &Entity<Backend>,
    group: &str,
    p: MoonPalette,
    cx: &App,
    on_cancel_all: impl Fn(&mut App) + 'static,
) -> AnyElement {
    let b = backend.read(cx);
    let core = b.active_trade_core(group);
    let cd = core.and_then(|c| b.session.store().core(c));
    let cs = cd.and_then(|d| d.client_settings.clone());
    let lm = cd.and_then(|d| d.lev_manage.clone());

    let root = v_flex()
        .id("core-settings-popup")
        .w(px(248.0))
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("core_settings.title").to_string()),
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
        MoonButton::new("core-restart")
            .label(t!("core_settings.restart").to_string())
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
        .child(h_flex().w_full().items_center().gap(design::ui_px(cx, 8.0)).child(restart_btn).child(emu_check))
        // Заметная плашка-предупреждение, когда включён режим эмулятора.
        .when(cs.emu_mode, |this| {
            this.child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 6.0))
                    .py(design::ui_px(cx, 3.0))
                    .rounded(design::ui_px(cx, 4.0))
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
                    if let Err(e) = b
                        .session
                        .edit_client_settings(core, ClientSettingsEdit::GlobalTakeProfit { on, pct })
                    {
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
                        if s.abs() > 1e-6 {
                            s
                        } else {
                            -1.0
                        }
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
                        if s != 0 {
                            s
                        } else {
                            -2
                        }
                    }
                } else {
                    0
                };
                let b = backend.read(app);
                if let Some(core) = b.active_trade_core(&group) {
                    if let Err(e) =
                        b.session.edit_client_settings(core, ClientSettingsEdit::VolDropLevel(n))
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
                    if let Err(e) = backend.read(app).session.set_exclude_blacklisted_delta(core, on)
                    {
                        log::warn!("exclude delta failed: {e:#}");
                    }
                }
            })
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
                    .child(exclude_check),
            )
            .child(
                div().w_full().child(
                    MoonInput::new("core-bl-text")
                        .state(blacklist_input)
                        .small(),
                ),
            )
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
    let reset_session = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonButton::new("core-reset-session")
            .label(t!("core_settings.reset_session").to_string())
            .size(MoonButtonSize::Action)
            .variant(MoonButtonVariant::Soft)
            .on_click(move |_, _w, app| reset_profit(&backend, &group, ResetProfitKind::Session, app))
            .render()
    };
    let reset_all = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonButton::new("core-reset-all")
            .label(t!("core_settings.reset_all").to_string())
            .size(MoonButtonSize::Action)
            .variant(MoonButtonVariant::Soft)
            .on_click(move |_, _w, app| reset_profit(&backend, &group, ResetProfitKind::All, app))
            .render()
    };
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
            .child(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .child(div().flex_1().child(reset_session))
                    .child(div().flex_1().child(reset_all)),
            )
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

/// Сброс прибыли активного ядра (без подтверждения).
fn reset_profit(backend: &Entity<Backend>, group: &str, kind: ResetProfitKind, app: &mut App) {
    let b = backend.read(app);
    if let Some(core) = b.active_trade_core(group) {
        if let Err(e) = b.session.reset_profit(core, kind) {
            log::warn!("reset_profit failed: {e:#}");
        }
    }
}
