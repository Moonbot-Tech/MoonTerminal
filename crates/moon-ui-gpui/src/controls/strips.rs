//! Полосы пресетов размера ордера (F1-F6) и fixed-sell (S1-S6) с overlay-взаимодействием
//! (клик/дабл/колесо) и общий каркас `strip_with_overlay`. Вынесено из `controls.rs` точь-в-точь.

use gpui::*;

use moon_ui::{
    MoonAccent, MoonInput, MoonInputState, MoonPalette, MoonSegmentItem, MoonSegmentedControl,
};

use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

use super::fmt::{fmt_adaptive, fmt_sell_pct, scroll_up, wheel_step};
use crate::{Backend, design};

/// Мелкая тусклая подпись группы (`size`/`sell`/`МАСШТАБ`) — стендовый `.strip-label`.
pub(super) fn strip_label(text: &'static str, p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .text_size(design::t_caption(cx))
        .font_family(design::ui_font())
        .text_color(rgb(p.text_muted))
        .child(text)
}

/// Вертикальный разделитель групп (стендовый `.divider`): тонкая линия высотой 16px.
pub(super) fn divider(p: MoonPalette) -> impl IntoElement {
    design::vline(16.0, p)
}

/// Ширины кнопок размера — единая 62 на все слоты.
const SIZE_W: [f32; 6] = [62.0, 62.0, 62.0, 62.0, 62.0, 62.0];

/// Дефолтный выбранный пресет размера (F3), когда у ядра ещё нет своего выбора.
pub(super) const SIZE_SEL_DEFAULT: usize = 2;

/// Полоса пресетов размера ордера (значения, без подписей F1-F6). Значения — из конфига ядра
/// (или дефолт по базе BTC/USDT), выбор хранится per-core в `Backend::order_size_sel`.
/// Взаимодействие — прозрачным overlay поверх каждой кнопки (MoonSegmentedControl сам колесо
/// не умеет): одиночный клик = выбор; дабл-клик = инлайн-правка (`order_size_edit_req`); КОЛЕСО
/// = ±значение с шагом по порядку величины (наведи и крути, не нажимая). `core=None` → без
/// взаимодействия.
pub(super) fn size_strip(
    values: [f64; 6],
    sel: usize,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    core: Option<CoreId>,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = (0..6)
        .map(|i| {
            let mut it = MoonSegmentItem::new("", fmt_adaptive(values[i])).width(SIZE_W[i]);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new("toolbar-size-presets")
        .accent(MoonAccent::Amber)
        .items(items)
        .render();

    let backend_click = backend.clone();
    strip_with_overlay(
        seg,
        "size",
        &SIZE_W,
        edit_ix,
        "toolbar-size-edit",
        input,
        core.is_some(),
        // Одиночный клик = выбор пресета; дабл = инлайн-правка (`order_size_edit_req`).
        move |i, dbl, cx| {
            let Some(core) = core else { return };
            backend_click.update(cx, |b, bcx| {
                if dbl {
                    b.order_size_edit_req = Some((core, i));
                } else {
                    // Выбор + персист в конфиг (восстановление после перезапуска).
                    b.set_order_size_sel(core, i);
                }
                b.order_size_rev = b.order_size_rev.wrapping_add(1);
                bcx.notify();
            });
        },
        // Колесо = ±значение с шагом по порядку величины (frac 1.0).
        move |i, up, cx| {
            let Some(core) = core else { return };
            backend.update(cx, |b, bcx| {
                let cur = b.order_size_value(core, i);
                let next = wheel_step(cur, up, 1.0);
                if next != cur {
                    b.set_order_size_value(core, i, next);
                    b.order_size_rev = b.order_size_rev.wrapping_add(1);
                    bcx.notify();
                }
            });
        },
    )
}

/// Ширины кнопок продажи — единая 33.0 на все слоты (44 −25%; исходно 62 базовых).
const SELL_W: [f32; 6] = [40.0, 40.0, 40.0, 40.0, 40.0, 40.0];

/// Полоса fixed-sell пресетов (S1-S6) рядом с кнопкой TP (без подписи). Значения — из
/// `ClientSettings` активного ядра (видимые проценты). Слот подсвечен ТОЛЬКО когда задействован
/// (`sel_slot = Some` лишь при `fixed_sell_mode`); по умолчанию все S погашены, горит TP.
/// Клик — задействовать слот (гасит TP); повторный клик по активному — вернуть TP. Нет ядра — прочерки.
pub(super) fn sell_strip(
    pcts: Option<[f64; 6]>,
    sel_slot: Option<usize>,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    core: Option<CoreId>,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = (0..6)
        .map(|i| {
            let value = match pcts {
                Some(p) => format!("{}%", fmt_sell_pct(p[i])),
                None => "—".to_string(),
            };
            let mut it = MoonSegmentItem::new("", value).width(SELL_W[i]);
            if sel_slot == Some(i + 1) {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new("toolbar-sell-presets")
        .accent(MoonAccent::Blue)
        .items(items)
        .render();

    let backend_click = backend.clone();
    strip_with_overlay(
        seg,
        "sell",
        &SELL_W,
        edit_ix,
        "toolbar-sell-edit",
        input,
        core.is_some(),
        // Одиночный клик = задействовать слот (гасит TP); повторный клик по активному слоту
        // = вернуть TP (гасит S, не трогая значение TP); дабл = инлайн-правка %.
        move |i, dbl, cx| {
            let Some(core) = core else { return };
            backend_click.update(cx, |b, bcx| {
                if dbl {
                    b.sell_edit_req = Some((core, i));
                } else {
                    // Повторный клик по уже горящему слоту → возврат к главному TP.
                    let (edit, local_slot) = if sel_slot == Some(i + 1) {
                        (ClientSettingsEdit::EngageMainTakeProfit, None)
                    } else {
                        (ClientSettingsEdit::SelectFixedSellSlot(i + 1), Some(i + 1))
                    };
                    b.set_fixed_sell_slot_local(core, local_slot);
                    b.order_size_rev = b.order_size_rev.wrapping_add(1);
                    if let Err(error) = b.session.edit_client_settings(core, edit) {
                        log::warn!("toggle fixed-sell slot failed: {error}");
                    }
                }
                bcx.notify();
            });
        },
        // Колесо = ±% полразрядом (frac 0.5). Значение % — на ядре, читаем из снимка ClientSettings.
        move |i, up, cx| {
            let Some(core) = core else { return };
            backend.update(cx, |b, bcx| {
                let cur = b.fixed_sell_pct(core, i);
                let next = wheel_step(cur, up, 0.5);
                if next != cur {
                    // Оптимистично: локальный кэш + перерисовка СРАЗУ; в ядро — тоже.
                    b.set_fixed_sell_pct_local(core, i, next);
                    b.order_size_rev = b.order_size_rev.wrapping_add(1);
                    bcx.notify();
                    if let Err(error) = b.session.edit_client_settings(
                        core,
                        ClientSettingsEdit::SetFixedSellPct {
                            slot: i + 1,
                            pct: next,
                        },
                    ) {
                        log::warn!("set fixed-sell pct (wheel) failed: {error}");
                    }
                }
            });
        },
    )
}

/// Общий каркас полос пресетов (size/sell): сегментированный контрол + прозрачные overlay
/// поверх каждой кнопки (MoonSegmentedControl сам клик/дабл/колесо не различает) + инлайн-инпут
/// поверх редактируемой кнопки. Отличия size/sell — только в `on_click`/`on_wheel` (наведи и
/// крути, не нажимая). `overlay=false` (нет ядра) → без взаимодействия.
#[allow(clippy::too_many_arguments)]
fn strip_with_overlay(
    seg: impl IntoElement,
    id_prefix: &'static str,
    widths: &[f32; 6],
    edit_ix: Option<usize>,
    edit_input_id: &'static str,
    input: &Entity<MoonInputState>,
    overlay: bool,
    on_click: impl Fn(usize, bool, &mut App) + Clone + 'static,
    on_wheel: impl Fn(usize, bool, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let mut root = div().relative().flex().items_center().child(seg);

    if overlay {
        for i in 0..6 {
            let left: f32 = widths.iter().take(i).sum();
            let on_click = on_click.clone();
            let on_wheel = on_wheel.clone();
            root = root.child(
                div()
                    .id(SharedString::from(format!("{id_prefix}-hit-{i}")))
                    .absolute()
                    .left(px(left))
                    .top(px(0.0))
                    .w(px(widths[i]))
                    .h_full()
                    .on_mouse_down(MouseButton::Left, move |ev, _w, cx| {
                        on_click(i, ev.click_count >= 2, cx);
                    })
                    .on_scroll_wheel(move |ev, _w, cx| {
                        on_wheel(i, scroll_up(ev), cx);
                    }),
            );
        }
    }

    // Инпут поверх редактируемой кнопки (absolute по сумме ширин предыдущих), на самом верху.
    if let Some(ix) = edit_ix.filter(|i| *i < 6) {
        let left: f32 = widths.iter().take(ix).sum();
        root = root.child(
            div()
                .absolute()
                .left(px(left))
                .top(px(0.0))
                .w(px(widths[ix]))
                .h_full()
                .child(MoonInput::new(edit_input_id).state(input).small()),
        );
    }
    root
}
