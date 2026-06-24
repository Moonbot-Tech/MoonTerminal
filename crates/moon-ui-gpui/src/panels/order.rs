//! Панель ордера (right dock): BUY/SELL/Cancel/Panic. Порт egui `dock/order.rs`.
//! Действия пока заглушки-лог; форму ввода/реальные ордера прикрутим позже.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, Panel, PanelEvent, PanelState, v_flex,
};
use rust_i18n::t;

use crate::Backend;

pub struct OrderPanel {
    backend: Entity<Backend>,
    group: String,
    focus: FocusHandle,
}
impl OrderPanel {
    pub fn new(backend: Entity<Backend>, group: String, cx: &mut Context<Self>) -> Self {
        Self {
            backend,
            group,
            focus: cx.focus_handle(),
        }
    }
}
impl EventEmitter<PanelEvent> for OrderPanel {}
impl Focusable for OrderPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for OrderPanel {
    fn panel_name(&self) -> &'static str {
        "Order"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(t!("order.title").to_string())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Order", &self.group)
    }
}
impl Render for OrderPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let backend = self.backend.clone();
        let group = self.group.clone();
        v_flex()
            .id("order-panel")
            .size_full()
            .p_3()
            .gap_2()
            .track_focus(&self.focus)
            .child(action(
                "buy",
                "BUY",
                MoonButtonVariant::Green,
                false,
                |_| log::info!("BUY"),
            ))
            .child(action(
                "sell",
                "SELL",
                MoonButtonVariant::OutlineRed,
                false,
                |_| log::info!("SELL"),
            ))
            .child(action(
                "cancel",
                "Cancel Buy",
                MoonButtonVariant::Amber,
                false,
                move |cx| {
                    backend.update(cx, |b, _| {
                        b.cancel_buy_for_main_chart(&group);
                    });
                },
            ))
            .child(action(
                "panic",
                "PANIC SELL",
                MoonButtonVariant::Danger,
                true,
                |_| log::info!("PANIC"),
            ))
    }
}

fn action(
    id: &'static str,
    label: &'static str,
    variant: MoonButtonVariant,
    strong: bool,
    f: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    MoonButton::new(id)
        .full_width()
        .variant(variant)
        .size(MoonButtonSize::Pill)
        .selected(strong)
        .label(label)
        .on_click(move |_, _, cx| f(cx))
        .render()
}
