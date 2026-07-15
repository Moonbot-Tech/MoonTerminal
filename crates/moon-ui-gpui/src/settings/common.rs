//! Общие UI-хелперы окна настроек (`slider_row`/`section`/`color_row`/`separator`)
//! и draft-байндеры (`draft_color`/`draft_slider`) — общие для вкладок
//! Интерфейс/Линии/Подключения (re-export в `settings/mod.rs`).

use std::collections::HashSet;

use gpui::*;
use moon_ui::{
    MoonAccordion, MoonColorPicker, MoonColorPickerEvent, MoonColorPickerState, MoonPalette,
    MoonSlider, MoonSliderEvent, MoonSliderState, h_flex, rgba_from, v_flex,
};

use super::SettingsView;
use crate::{Backend, design};
use moon_core::config::AppConfig;

/// Сворачиваемый блок на компоненте MoonUI `MoonAccordion` (один item на ключ): заголовок
/// с шевроном + тело. Общий для вкладок Линии/Хоткеи: раскрытость живёт в per-вкладочном
/// `HashSet` (`open` — текущее значение, `set` — доступ к набору для тоггла по клику).
pub(super) fn collapse_block(
    cx: &Context<SettingsView>,
    id: SharedString,
    key: &'static str,
    title: SharedString,
    open: bool,
    body: AnyElement,
    set: fn(&mut SettingsView) -> &mut HashSet<&'static str>,
) -> impl IntoElement + use<> {
    let entity = cx.entity();
    MoonAccordion::new(id)
        .item(move |item| item.title(title).open(open).child(body))
        .on_toggle_click(move |open_ixs, _window, cx| {
            let now_open = !open_ixs.is_empty();
            entity.update(cx, |this, c| {
                let s = set(this);
                let changed = if now_open { s.insert(key) } else { s.remove(key) };
                if changed {
                    c.notify();
                }
            });
        })
}

/// Hsla (из color-picker) → sRGB [u8;3] для ChartTheme/OrdersStyle.
pub(super) fn hsla_u8(h: Hsla) -> [u8; 3] {
    let c: Rgba = h.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

/// Строка слайдера (порт egui `Slider::new(..).text(label)`): подпись СВЕРХУ
/// (полная ширина, не обрезается при крупном кегле), под ней — слайдер и
/// текущее значение. Значение нормализуем от IEEE `-0.0` (квантование слайдера
/// на отрицательном поддиапазоне даёт минус-ноль → печаталось «-0.00»).
pub(super) fn slider_row(label: &str, st: &Entity<MoonSliderState>, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let val = st.read(cx).value().end();
    let val = if val == 0.0 { 0.0 } else { val };
    v_flex()
        .w_full()
        .child(
            div()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
        .child(
            h_flex()
                .w_full()
                .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .child(
                    div()
                        .w(px(180.0))
                        .child(MoonSlider::new(st).height(design::ui_value(cx, 22.0))),
                )
                .child(
                    div()
                        .w(design::font_w_px(cx, 58.0))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(format!("{val:.2}")),
                ),
        )
}

/// Разделитель секций (порт egui `ui.separator()`).
pub(super) fn separator(p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .my(design::ui_px(cx, 8.0))
        .h(px(1.0))
        .bg(rgba_from(p.border, 1.0))
}

/// Секционный заголовок (порт egui `section()`): жирная подпись с отступом сверху.
pub(super) fn section(title: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .mt(design::ui_px(cx, 10.0))
        .mb(design::ui_px(cx, 4.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgba_from(p.text, 1.0))
        .child(title.to_string())
}

/// Строка цвета (порт egui `color_row`): свотч-пикер, затем подпись справа.
pub(super) fn color_row(
    label: &str,
    st: &Entity<MoonColorPickerState>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(MoonColorPicker::new(st))
        .child(
            div()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
}

/// Общий color-picker draft-настроек: init = переданное значение, на `Change` — пишет в живой
/// `Backend.preview` через `apply` (он же делает проверку «изменилось ли» и возвращает результат) и
/// нотифаит бэкенд. `apply` — замыкание (может захватывать индекс сервера и т.п.). Общий для вкладок
/// Интерфейс/Линии/Подключения (тонкие обёртки делегируют сюда).
pub(super) fn draft_color(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    init: [u8; 3],
    apply: impl Fn(&mut AppConfig, [u8; 3]) -> bool + 'static,
) -> Entity<MoonColorPickerState> {
    let st = cx.new(|cx| {
        MoonColorPickerState::new(window, cx).default_value(rgb(design::rgb_to_u32(init)).into())
    });
    cx.subscribe(&st, move |this, _emitter, ev: &MoonColorPickerEvent, cx| {
        let MoonColorPickerEvent::Change(h) = ev;
        let c = hsla_u8(*h);
        this.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if apply(p, c) {
                    bcx.notify();
                }
            }
        });
    })
    .detach();
    st
}

/// Общий слайдер f32 draft-настроек: init = переданное значение, на `Change` — пишет в живой
/// `Backend.preview` через `apply` (проверка изменения + сам сеттер; `&mut Context<Backend>` нужен
/// тем полям, что переустанавливают тему). Нотифаит бэкенд, если `apply` вернул true.
pub(super) fn draft_slider(
    cx: &mut Context<SettingsView>,
    min: f32,
    max: f32,
    step: f32,
    init: f32,
    apply: impl Fn(&mut AppConfig, f32, &mut Context<Backend>) -> bool + 'static,
) -> Entity<MoonSliderState> {
    let st = cx.new(|_| {
        MoonSliderState::new()
            .min(min)
            .max(max)
            .step(step)
            .default_value(init)
    });
    cx.subscribe(&st, move |this, _emitter, ev: &MoonSliderEvent, cx| {
        let MoonSliderEvent::Change(f) = ev else {
            return;
        };
        // Квантование слайдера на отрицательном поддиапазоне даёт IEEE -0.0 —
        // нормализуем, чтобы в конфиг/на диск не уезжал «минус-ноль».
        let f = f.end();
        let f = if f == 0.0 { 0.0 } else { f };
        this.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if apply(p, f, bcx) {
                    bcx.notify();
                }
            }
        });
    })
    .detach();
    st
}
