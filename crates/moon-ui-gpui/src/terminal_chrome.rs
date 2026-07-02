//! Terminal-specific chrome composition over MoonPalette primitives.
//!
//! This is an adapter layer, not a reusable MoonPalette control: it knows about
//! Backend actions and MoonTerminal header content, while generic visuals still
//! come from MoonPalette tokens/components.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonPopover, MoonPopoverPlacement, MoonPopupMenu, MoonSelectorPill, MoonSelectorSegment,
    MoonTag, MoonWindowFrame, h_flex,
};
use rust_i18n::t;

use moon_core::feed::ConnStatus;

use crate::shell::Shell;
use crate::{Backend, design, settings, strategies};

pub fn header(
    group: &str,
    backend: Entity<Backend>,
    shell: Entity<Shell>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    // Баланс активного торгового ядра группы (серверные значения в USDT). Нет ядра/данных → нули.
    let (free_usdt, total_usdt) = {
        let b = backend.read(cx);
        b.active_trade_core(group)
            .and_then(|c| b.session.store().core(c))
            .map(|cd| (cd.assets.global.free_usdt, cd.assets.global.total_usdt))
            .unwrap_or((0.0, 0.0))
    };
    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, design::HEADER_TOP_H, 14.0, 9.0))
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .gap(design::ui_px(cx, 12.0))
        .bg(rgb(p.shell_high))
        .child(
            MoonWindowFrame::main("terminal-header-brand-drag", 0.0)
                .brand_cluster(cx)
                .flex_none()
                .h_full(),
        )
        // Селектор активного ядра (на месте бывшего названия монеты) + баланс. Интерактивные →
        // НЕ drag-зона (иначе клик по селектору таскал бы окно). Монету (`group · market`) убрали.
        .child(
            h_flex()
                .flex_none()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .child(core_selector(group, &backend, p, cx))
                .child(core_gear_button(shell, p, cx))
                .child(balance_label(free_usdt, total_usdt, p, cx)),
        )
        // Метрики Session/Real/Unreal/Risk из шапки убраны: сервер (moonproto) не отдаёт
        // session-профита (см. docs-internal/PROTO_REQUEST_PROFIT_AND_STOP_DEFAULTS.md),
        // а остальное решили не показывать. Остался только баланс у селектора ядра.
        .child(
            MoonWindowFrame::main("terminal-header-spacer-drag", 0.0)
                .drag_handle()
                .h_full()
                .flex_1()
                .flex(),
        )
        .child(
            h_flex()
                .flex_none()
                .gap(design::ui_px(cx, 12.0))
                .items_center()
                .child(header_action(
                    "strategies",
                    t!("toolbar.strategies").to_string(),
                    {
                        let backend = backend.clone();
                        move |_, window, cx| {
                            strategies::open(backend.clone(), Some(window.window_handle()), cx)
                        }
                    },
                    p,
                    cx,
                ))
                .child(header_action(
                    "gear",
                    "⚙",
                    {
                        let backend = backend.clone();
                        move |_, window, cx| {
                            settings::open(backend.clone(), Some(window.window_handle()), cx)
                        }
                    },
                    p,
                    cx,
                ))
                .when(design::show_custom_window_controls(), |this| {
                    this.child(
                        MoonWindowFrame::main("terminal-header-controls", 0.0)
                            .show_controls(true)
                            .visual_controls(cx),
                    )
                }),
        )
}

fn positive_text(p: MoonPalette) -> u32 {
    if p.is_light() { p.green_text } else { p.green }
}

fn danger_text(p: MoonPalette) -> u32 {
    if p.is_light() { p.red_text } else { p.red }
}

/// Селектор «активного торгового ядра» группы. Список ядер группы; текущий выбор =
/// `Backend::active_trade_core` (авто-следование за фуллскрин-чартом + sticky-override
/// при ручном выборе). Все торговые контролы тулбара/шапки читают это же ядро.
fn core_selector(group: &str, backend: &Entity<Backend>, p: MoonPalette, cx: &App) -> AnyElement {
    // Геометрия пилюли: высота фикс., полное скругление = ½ высоты (ширина — по контенту, как в
    // каноне MoonSelectorPill). `SEL_NAME_MAX` — лимит символов имени (обрезка справа, у пилюли
    // нет overflow-клипа).
    const SEL_H: f32 = 26.0;
    const SEL_NAME_MAX: usize = 13;

    let b = backend.read(cx);
    let cores = b.group_cores(group);
    let active = b.active_trade_core(group);
    let store = b.session.store();

    // Нет ядер в группе — статичная заглушка вместо пустого дропдауна.
    if cores.is_empty() {
        return MoonTag::new()
            .outline()
            .rounded_full()
            .child(design::status_dot(p.text_muted, cx))
            .label(t!("header.no_cores").to_string())
            .into_any_element();
    }

    let active_ready = active
        .and_then(|id| store.core(id))
        .map(|c| c.status == ConnStatus::Ready)
        .unwrap_or(false);
    let dot_color = if active_ready {
        positive_text(p)
    } else {
        danger_text(p)
    };
    let active_name = active
        .and_then(|id| cores.iter().find(|(cid, _)| *cid == id))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| "—".to_string());
    // У пилюли нет overflow-клипа → длинное имя обрезаем САМИ по символам, оставляя ЛЕВУЮ часть
    // (как просили: «обрезать справа», левый край на месте). Короткие имена остаются как есть;
    // многоточие не добавляем (имя — лишь подпись активного ядра, точный список — в попапе).
    let active_name: String = active_name.chars().take(SEL_NAME_MAX).collect();

    let mut items = Vec::with_capacity(cores.len());
    for (id, name) in &cores {
        let id = *id;
        let backend = backend.clone();
        let group = group.to_string();
        items.push(
            MoonMenuItem::with_key(format!("core-{id}"), name.clone())
                .selected(active == Some(id))
                .checked(active == Some(id))
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        b.set_trade_core_override(&group, id);
                        bcx.notify();
                    });
                }),
        );
    }

    // Каноничный визуал `MoonSelectorPill` (точка статуса со свечением + каретка-иконка) как
    // триггер `MoonPopover`, контент — `MoonPopupMenu` со списком ядер. Всё напрямую из moonui:
    // ни ручной стилизации триггера, ни хака размеров. Popover сам держит open-стейт (внутренний
    // `use_keyed_state`) и тогглит по клику; `close_on_content_click` закрывает после выбора ядра.
    //
    // Фон пилюли = `p.panel`; у `MoonSelectorPill` есть явный бордер `p.border` → «таблетка»
    // читается даже на фоне шапки `shell_high` (в отличие от старого Panel-кейса без рамки).
    MoonPopover::new("header-core-selector")
        .placement(MoonPopoverPlacement::BottomStart)
        .width(192.0) // 180 ширина меню + 2×6 паддинг попапа
        .close_on_content_click(true)
        .trigger(
            MoonSelectorPill::new("header-core-pill")
                .height(SEL_H)
                .radius(SEL_H / 2.0)
                .leading_dot(dot_color)
                .segment(
                    MoonSelectorSegment::new(active_name)
                        .color(p.text)
                        .weight(500.0),
                )
                .render(),
        )
        .content(
            MoonPopupMenu::new("header-core-menu")
                .width(180.0)
                .size(MoonMenuSize::Compact)
                .items(items)
                .render(),
        )
        .into_any_element()
}

/// Кнопка ⚙ настроек ядра (тоггл попапа). Тот же визуал, что правая шестерёнка (Panel/Action),
/// но открывает overlay-попап Shell, а не окно настроек терминала.
fn core_gear_button(shell: Entity<Shell>, _p: MoonPalette, _cx: &App) -> impl IntoElement {
    MoonButton::new("core-gear")
        .label("⚙")
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Panel)
        .on_click(move |_, window, cx| {
            shell.update(cx, |s, cx| s.toggle_core_settings_popup(window, cx));
        })
        .render()
}

fn balance_label(free_usdt: f64, total_usdt: f64, p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .gap(px(0.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .text_color(rgb(p.text_soft))
        .child("Balance: ")
        .child(
            div()
                .text_color(rgb(p.text))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("{free_usdt:.2}")),
        )
        .child(
            div()
                .text_color(rgb(p.text_muted))
                .child(format!(" /{total_usdt:.0} USDT")),
        )
}

fn header_action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    _p: MoonPalette,
    _cx: &App,
) -> impl IntoElement {
    let id: SharedString = id.into();
    MoonButton::new(id)
        .label(label)
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Panel)
        .on_click(on_click)
        .render()
}
