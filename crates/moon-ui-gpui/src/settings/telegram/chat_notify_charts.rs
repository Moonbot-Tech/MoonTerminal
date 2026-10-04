//! The deal chart switch and its two thresholds in a chat's Notifications column: a picture of the
//! trade's tape, sent apart from the cards (LinKvo 04.10 — cards may be off, pictures on).
//!
//! Shown for the station's bot only: the picture is drawn from the station's tape recorder, which
//! a terminal runs only as a measuring instrument. Saved with the rest of the chat's
//! notifications.

use gpui::*;
use moon_ui::{MoonCheckbox, MoonInput, MoonInputState, MoonPalette, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::SettingsView;
use super::access::ChatsOf;
use crate::design;

impl SettingsView {
    /// `chat`'s deal chart rule on `side`, as the editor holds it.
    pub(super) fn chart_notify_block(
        &self,
        side: ChatsOf,
        chat: i64,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let id = |what: &str| -> SharedString {
            match side {
                ChatsOf::Terminal => format!("tgn-chart-{what}-{chat}").into(),
                ChatsOf::Station => format!("tgns-chart-{what}-{chat}").into(),
            }
        };
        let ed = self.notify_ed(side);
        let field = |state: &Entity<MoonInputState>, what: &str, label: String| {
            h_flex()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .child(div().text_color(muted).child(label))
                .child(
                    div().w(design::ui_px(cx, 90.0)).child(
                        MoonInput::new(id(what))
                            .state(state)
                            .size(design::INPUT_SIZE),
                    ),
                )
        };
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(
                MoonCheckbox::new(id("on"))
                    .checked(ed.draft.charts.on)
                    .label(t!("telegram.notify_editor.charts").to_string())
                    .on_change(cx.listener(move |this, v: &bool, _, cx| {
                        let v = *v;
                        this.notify_edit(side, cx, |s| s.charts.on = v);
                    })),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 16.0))
                    .child(field(
                        &ed.chart_profit,
                        "profit",
                        t!("telegram.mini_settings_profit_at_least").to_string(),
                    ))
                    .child(field(
                        &ed.chart_loss,
                        "loss",
                        t!("telegram.mini_settings_loss_at_least").to_string(),
                    )),
            )
            .child(
                div().text_color(muted).child(
                    t!(
                        "telegram.notify_editor.charts_hint",
                        minutes = moon_tg::TRADE_HOLD_MINUTES
                    )
                    .to_string(),
                ),
            )
            .into_any_element()
    }
}
