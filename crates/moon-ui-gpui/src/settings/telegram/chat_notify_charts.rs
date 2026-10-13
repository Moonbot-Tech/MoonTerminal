//! The deal chart switch and its two thresholds in a chat's Notifications column: a picture of the
//! trade's tape, sent apart from the cards (LinKvo 04.10 — cards may be off, pictures on).
//!
//! Shown for the station's bot only: the picture is drawn from the station's tape recorder, which
//! a terminal runs only as a measuring instrument. Saved with the rest of the chat's
//! notifications.

use gpui::*;
use moon_ui::MoonCheckbox;
use rust_i18n::t;

use super::SettingsView;
use super::access::ChatsOf;
use super::chat_notify::amounts_line;
use super::chat_notify_quotes::QuoteList;

impl SettingsView {
    /// `chat`'s deal chart rule on `side`, as the editor holds it: the controls, their
    /// explanation and the controls' one-line width, for the rule's `aside_row`.
    pub(super) fn chart_notify_parts(
        &self,
        side: ChatsOf,
        chat: i64,
        cx: &Context<Self>,
    ) -> (Vec<AnyElement>, String, f32) {
        let id = |what: &str| -> SharedString {
            match side {
                ChatsOf::Terminal => format!("tgn-chart-{what}-{chat}").into(),
                ChatsOf::Station => format!("tgns-chart-{what}-{chat}").into(),
            }
        };
        let ed = self.notify_ed(side);
        let (amounts, amounts_w) = amounts_line(
            vec![
                (
                    &ed.chart_profit,
                    "profit",
                    t!("telegram.mini_settings_profit_at_least").to_string(),
                ),
                (
                    &ed.chart_loss,
                    "loss",
                    t!("telegram.mini_settings_loss_at_least").to_string(),
                ),
            ],
            id,
            cx,
        );
        let switch = MoonCheckbox::new(id("on"))
            .checked(ed.draft.charts.on)
            .label(t!("telegram.notify_editor.charts").to_string())
            .on_change(cx.listener(move |this, v: &bool, _, cx| {
                let v = *v;
                this.notify_edit(side, cx, |s| s.charts.on = v);
            }))
            .into_any_element();
        let hint = t!(
            "telegram.notify_editor.charts_hint",
            minutes = moon_tg::TRADE_HOLD_MINUTES
        )
        .to_string();
        let quotes = self.quote_block(side, chat, QuoteList::Charts, cx);
        (vec![switch, amounts, quotes], hint, amounts_w)
    }
}
