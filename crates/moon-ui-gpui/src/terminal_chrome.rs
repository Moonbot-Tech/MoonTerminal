//! Terminal-specific chrome composition over MoonPalette primitives.
//!
//! This is an adapter layer, not a reusable MoonPalette control: it knows about
//! Backend actions and MoonTerminal header content, while generic visuals still
//! come from MoonPalette tokens/components.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem,
    MoonMenuSize, MoonPalette, MoonProgress, MoonTag, MoonWindowFrame, h_flex,
};
use rust_i18n::t;

use moon_core::feed::ConnStatus;

use crate::{Backend, design, settings, strategies};

pub fn header(group: &str, backend: Entity<Backend>, p: MoonPalette, cx: &App) -> impl IntoElement {
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
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .child(core_selector(group, &backend, p, cx))
                .child(balance_label(p, cx)),
        )
        .child(
            MoonWindowFrame::main("terminal-header-metrics-drag", 0.0)
                .drag_handle()
                .flex()
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .min_w_0()
                .overflow_hidden()
                .child(metric("Session", "+$24.30", p.green, p, cx))
                .child(metric("Real", "+$104.20", p.green, p, cx))
                .child(metric("Unreal", "−$8.10", p.orange, p, cx))
                .child(risk_meter(p, cx)),
        )
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

fn metric(
    label: &'static str,
    value: &'static str,
    color: u32,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
        .gap(design::ui_px(cx, 5.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .font_family(design::ui_font())
                .text_color(rgb(p.text_muted))
                .child(label),
        )
        .child(
            div()
                .text_color(rgb(color))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
}

fn risk_meter(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
        .gap(design::ui_px(cx, 8.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .font_family(design::ui_font())
                .text_color(rgb(p.text_muted))
                .child("Risk"),
        )
        .child(
            div().w(px(64.0)).child(
                MoonProgress::new("risk-meter")
                    .value(18.0)
                    .color(p.green)
                    .height(4.0)
                    .radius(2.0)
                    .render(),
            ),
        )
        .child(div().text_color(rgb(p.green)).child("18%"))
}

/// Селектор «активного торгового ядра» группы. Список ядер группы; текущий выбор =
/// `Backend::active_trade_core` (авто-следование за фуллскрин-чартом + sticky-override
/// при ручном выборе). Все торговые контролы тулбара/шапки читают это же ядро.
fn core_selector(group: &str, backend: &Entity<Backend>, p: MoonPalette, cx: &App) -> AnyElement {
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
            .child(t!("header.no_cores").to_string())
            .into_any_element();
    }

    let active_ready = active
        .and_then(|id| store.core(id))
        .map(|c| c.status == ConnStatus::Ready)
        .unwrap_or(false);
    let dot_color = if active_ready { p.green } else { p.red };
    let active_name = active
        .and_then(|id| cores.iter().find(|(cid, _)| *cid == id))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| "—".to_string());

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

    MoonDropdown::new("header-core-selector")
        .trigger_variant(MoonButtonVariant::Panel)
        .trigger_size(MoonButtonSize::Action)
        .menu_width(180.0)
        .menu_size(MoonMenuSize::Compact)
        .segment(MoonButtonSegment::new("●").color(dot_color).weight(400.0))
        .segment(
            MoonButtonSegment::new(active_name)
                .color(p.text)
                .weight(500.0),
        )
        .segment(
            MoonButtonSegment::new("▾")
                .color(p.text_muted)
                .weight(400.0),
        )
        .items(items)
        .into_any_element()
}

fn balance_label(p: MoonPalette, cx: &App) -> impl IntoElement {
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
                .child("50.00"),
        )
        .child(div().text_color(rgb(p.text_muted)).child(" /50 USDT"))
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
