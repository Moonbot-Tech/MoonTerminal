//! Общее для док-панелей: кнопка открепления (⧉), гейт перерисовки `RenderGate`
//! (повторялся в Orders/Assets) и числовой форматтер `num`. Каждая панель живёт в
//! своей папке (`orders/`, `assets/`, `report/`, `log/`); сюда вынесено лишь ОБЩЕЕ.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{DockArea, MoonButton, MoonButtonSize, MoonPalette};

use crate::Backend;
use crate::design;
use crate::detached::DetachedSpec;

/// Адаптивный числовой формат (кол-во/цена) — общий для таблиц Orders/Assets.
pub(crate) fn num(v: f64) -> String {
    moon_core::util::fmt::adaptive(v)
}

/// Хост таблицы данных док-панели (общий для Orders/Assets): контейнер на всю высоту с фоном
/// `table_body`, сама `MoonDataTable` (строит вызывающий — у каждой панели свои колонки/строки) и
/// оверлей-заглушка «пусто» при `empty` (как egui-плейсхолдер, поверх шапки). `empty_msg` —
/// готовая локализованная строка.
pub(crate) fn data_table_host(
    host_id: impl Into<SharedString>,
    empty: bool,
    empty_msg: String,
    p: MoonPalette,
    cx: &App,
    table: impl IntoElement,
) -> impl IntoElement {
    div()
        .id(host_id.into())
        .relative()
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .overflow_hidden()
        .bg(rgb(p.table_body))
        .child(table)
        .when(empty, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(10.0))
                    .top(px(design::TABLE_HEAD_H))
                    .h(px(design::TABLE_ROW_H))
                    .flex()
                    .items_center()
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(empty_msg),
            )
        })
}

/// Гейт перерисовки док-панели по сигнатуре данных. Повторялся в Orders/Assets:
/// перерисовываем при смене сигнатуры данных ИЛИ раз в секунду (живые цены/P&L),
/// но НЕ ЧАЩЕ 4 Гц (пол 250мс) — частые ивенты коалесцируются (глаз не различит).
#[derive(Default)]
pub(crate) struct RenderGate {
    last_sig: u64,
    last_sec: u64,
    last_notify_ms: f64,
}

impl RenderGate {
    /// True, если пора перерисовать (сигнатура сменилась ИЛИ новое секундное ведро),
    /// с полом 250мс. На true обновляет внутреннее состояние.
    pub(crate) fn should_notify(&mut self, sig: u64, now_ms: f64) -> bool {
        let sec = (now_ms as u64) / 1000;
        let changed = sig != self.last_sig || sec != self.last_sec;
        if changed && now_ms - self.last_notify_ms >= 250.0 {
            self.last_sig = sig;
            self.last_sec = sec;
            self.last_notify_ms = now_ms;
            true
        } else {
            false
        }
    }
}

/// Кнопка тулбара «открепить в окно» (⧉): убирает панель из своего дока и открывает
/// её отдельным окном, записывая спеку в `backend.detached`. `name` — стабильное имя
/// панели (как у `panel_name`/`remove_panel_by_name`/`DetachedSpec`).
pub fn detach_button(
    name: &'static str,
    group: String,
    backend: Entity<Backend>,
    dock: Option<WeakEntity<DockArea>>,
) -> AnyElement {
    MoonButton::new(SharedString::from(format!("detach-{name}")))
        .ghost()
        .size(MoonButtonSize::Action)
        .label("⧉")
        .on_click(move |_, window, app| {
            let spec = DetachedSpec::new(group.clone(), name.to_string());
            if let Err(err) =
                crate::detached::spawn(app, &backend, &spec, Some(window.window_handle()))
            {
                log::warn!("detach panel failed group={} panel={name}: {err:#}", group);
                return;
            }
            // Убрать себя из дока только после успешного открытия окна.
            if let Some(dock) = dock.as_ref().and_then(|d| d.upgrade()) {
                dock.update(app, |area, cx| {
                    area.remove_panel_by_name(name, window, cx);
                });
            }
            // Записать спеку после успешного открытия + удаления из дока.
            backend.update(app, |b, _| {
                if !b
                    .detached
                    .iter()
                    .any(|s| s.group == spec.group && s.panel == spec.panel)
                {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                }
            });
        })
        .render()
        .into_any_element()
}
