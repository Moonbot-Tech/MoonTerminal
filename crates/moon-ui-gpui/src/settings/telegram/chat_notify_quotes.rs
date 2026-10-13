//! Per-currency thresholds of a chat's trade cards and deal charts (LinKvo 13.10): one row per
//! currency, its amounts in that currency's own money. A trade in a listed currency is judged by
//! its row and does not wait for a dollar valuation — the way to hear about a core trading in a
//! currency nothing values, such as RUB.
//!
//! The rows hold their own number fields, created when the editor fills or a currency is added;
//! the stored list is rebuilt from them on save, as the dollar fields are.

use gpui::*;
use moon_core::db::QuoteCurrency;
use moon_core::telegram::notify::{NotifySettings, QuoteThresholds};
use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonInput,
    MoonInputState, MoonMenuItem, MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use super::access::ChatsOf;
use super::chat_notify::parse_amount;
use crate::design;

/// Which rule's list a row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuoteList {
    /// The trade card rule: volume, profit and loss.
    Trades,
    /// The deal chart rule: profit and loss, no volume floor.
    Charts,
}

impl QuoteList {
    /// The locale keys of this list's fields, in [`QuoteRowEd::fields`] order.
    fn labels(self) -> &'static [&'static str] {
        match self {
            Self::Trades => &[
                "telegram.quote_volume",
                "telegram.quote_profit",
                "telegram.quote_loss",
            ],
            Self::Charts => &["telegram.quote_profit", "telegram.quote_loss"],
        }
    }

    /// The stored rows of this list.
    fn stored(self, settings: &NotifySettings) -> &[QuoteThresholds] {
        match self {
            Self::Trades => &settings.trades.by_quote,
            Self::Charts => &settings.charts.by_quote,
        }
    }

    /// A short tag for element ids.
    fn tag(self) -> &'static str {
        match self {
            Self::Trades => "trades",
            Self::Charts => "charts",
        }
    }
}

/// One currency's row as edited: the currency and its number fields.
pub(super) struct QuoteRowEd {
    quote: QuoteCurrency,
    /// [`QuoteList::labels`] order.
    fields: Vec<Entity<MoonInputState>>,
}

/// A list's rows as typed: each currency with its fields' texts. Compared against the snapshot
/// taken when the editor filled, to tell the user's edits from the stored row.
pub(super) type QuoteTexts = Vec<(QuoteCurrency, Vec<String>)>;

/// The texts of `rows` as typed now.
pub(super) fn quote_texts(rows: &[QuoteRowEd], cx: &App) -> QuoteTexts {
    rows.iter()
        .map(|row| {
            let texts = row
                .fields
                .iter()
                .map(|field| field.read(cx).value().to_string())
                .collect();
            (row.quote, texts)
        })
        .collect()
}

/// The texts a stored list fills its fields with: empty for none. A row naming a currency this
/// build does not know is left out — it judges nothing here — and a save keeps it as stored
/// (`chat_notify::settings_with_quotes`).
pub(super) fn stored_texts(list: QuoteList, settings: &NotifySettings) -> QuoteTexts {
    let amount = |value: Option<f64>| value.map(|v| format!("{v}")).unwrap_or_default();
    list.stored(settings)
        .iter()
        .filter_map(|row| {
            let texts = match list {
                QuoteList::Trades => vec![
                    amount(row.min_volume),
                    amount(row.profit_at_least),
                    amount(row.loss_at_least),
                ],
                QuoteList::Charts => vec![amount(row.profit_at_least), amount(row.loss_at_least)],
            };
            Some((row.currency()?, texts))
        })
        .collect()
}

/// The stored list for typed `texts`, or the locale key of the first field that does not read.
pub(super) fn rows_from(
    list: QuoteList,
    texts: &QuoteTexts,
) -> Result<Vec<QuoteThresholds>, &'static str> {
    texts
        .iter()
        .map(|(quote, fields)| {
            let mut amounts = fields
                .iter()
                .map(|text| parse_amount(text).map_err(|_| "telegram.notify_editor.err_amount"));
            let mut next = || amounts.next().transpose().map(Option::flatten);
            let min_volume = match list {
                QuoteList::Trades => next()?,
                QuoteList::Charts => None,
            };
            Ok(QuoteThresholds {
                quote: quote.ticker().to_string(),
                min_volume,
                profit_at_least: next()?,
                loss_at_least: next()?,
            })
        })
        .collect()
}

/// Fresh rows for `texts` on `side`, each field created in `window`, filled with its text and
/// saved the way the dollar fields are (`SettingsView::wire_notify`): once it is left or
/// confirmed with Enter.
pub(super) fn quote_rows(
    side: ChatsOf,
    texts: &QuoteTexts,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Vec<QuoteRowEd> {
    texts
        .iter()
        .map(|(quote, values)| QuoteRowEd {
            quote: *quote,
            fields: values
                .iter()
                .map(|value| {
                    let value = value.clone();
                    let field = cx.new(|cx| {
                        let mut state = MoonInputState::new(window, cx);
                        state.set_value(value, window, cx);
                        state
                    });
                    cx.subscribe(&field, move |this, _, ev: &moon_ui::MoonInputEvent, cx| {
                        if matches!(
                            ev,
                            moon_ui::MoonInputEvent::Blur
                                | moon_ui::MoonInputEvent::PressEnter { .. }
                        ) {
                            this.notify_commit_open(side, cx);
                        }
                    })
                    .detach();
                    field
                })
                .collect(),
        })
        .collect()
}

/// The rows for `texts`, reusing the field entities of a currency `old` already has — only its
/// values are set again — and creating fields for a currency it does not. A refill after every
/// autosave must not destroy the field the user just tabbed into: a fresh entity has no focus.
pub(super) fn refill_rows(
    side: ChatsOf,
    mut old: Vec<QuoteRowEd>,
    texts: &QuoteTexts,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Vec<QuoteRowEd> {
    texts
        .iter()
        .flat_map(|(quote, values)| {
            let reused = old
                .iter()
                .position(|row| row.quote == *quote && row.fields.len() == values.len())
                .map(|at| old.swap_remove(at));
            match reused {
                Some(row) => {
                    for (field, value) in row.fields.iter().zip(values) {
                        let value = value.clone();
                        field.update(cx, |state, cx| state.set_value(value, window, cx));
                    }
                    vec![row]
                }
                None => quote_rows(side, &vec![(*quote, values.clone())], window, cx),
            }
        })
        .collect()
}

impl SettingsView {
    /// `side`'s rows of `list`.
    fn quote_rows_of(&self, side: ChatsOf, list: QuoteList) -> &Vec<QuoteRowEd> {
        let ed = self.notify_ed(side);
        match list {
            QuoteList::Trades => &ed.trade_quotes,
            QuoteList::Charts => &ed.chart_quotes,
        }
    }

    /// `side`'s rows of `list`, to change.
    fn quote_rows_mut(&mut self, side: ChatsOf, list: QuoteList) -> &mut Vec<QuoteRowEd> {
        let ed = self.notify_ed_mut(side);
        match list {
            QuoteList::Trades => &mut ed.trade_quotes,
            QuoteList::Charts => &mut ed.chart_quotes,
        }
    }

    /// Add an empty row for `quote` to `list` and apply it at once, as a switch is: every trade in
    /// that currency passes until a threshold is typed.
    fn quote_add(
        &mut self,
        side: ChatsOf,
        list: QuoteList,
        quote: QuoteCurrency,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .quote_rows_of(side, list)
            .iter()
            .any(|r| r.quote == quote)
        {
            return;
        }
        let empty = vec![(quote, vec![String::new(); list.labels().len()])];
        let mut row = quote_rows(side, &empty, window, cx);
        self.quote_rows_mut(side, list).append(&mut row);
        self.notify_ed_mut(side).clear_status();
        self.notify_commit_open(side, cx);
        cx.notify();
    }

    /// Drop `quote`'s row from `list` and apply it at once.
    fn quote_remove(
        &mut self,
        side: ChatsOf,
        list: QuoteList,
        quote: QuoteCurrency,
        cx: &mut Context<Self>,
    ) {
        self.quote_rows_mut(side, list).retain(|r| r.quote != quote);
        self.notify_ed_mut(side).clear_status();
        self.notify_commit_open(side, cx);
        cx.notify();
    }

    /// The rows of `list` for `chat` on `side`, then the picker adding a currency. Its hint is
    /// [`quote_hint`], for the rule's explanation.
    pub(super) fn quote_block(
        &self,
        side: ChatsOf,
        chat: i64,
        list: QuoteList,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let prefix = match side {
            ChatsOf::Terminal => "tgn",
            ChatsOf::Station => "tgns",
        };
        let id = |what: &str| -> SharedString {
            format!("{prefix}-q-{}-{what}-{chat}", list.tag()).into()
        };
        let rows = self.quote_rows_of(side, list);
        let row_el = |row: &QuoteRowEd| {
            let quote = row.quote;
            let ticker = quote.ticker();
            let fields = list
                .labels()
                .iter()
                .zip(&row.fields)
                .enumerate()
                .map(|(n, (key, state))| {
                    (
                        t!(*key).to_string(),
                        MoonInput::new(id(&format!("{ticker}-{n}")))
                            .state(state)
                            .size(design::INPUT_SIZE)
                            .into_any_element(),
                    )
                })
                .collect();
            let remove = MoonButton::new(id(&format!("{ticker}-remove")))
                .ghost()
                .padding_x(REMOVE_PAD_X)
                .width(remove_w(cx))
                .label(t!("telegram.notify_editor.quote_remove").to_string())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.quote_remove(side, list, quote, cx);
                }))
                .render()
                .into_any_element();
            quote_row_line(ticker, fields, remove, cx)
        };
        let weak = cx.entity().downgrade();
        let addable: Vec<MoonMenuItem> = QuoteCurrency::all()
            .filter(|quote| rows.iter().all(|r| r.quote != *quote))
            .map(|quote| {
                let weak = weak.clone();
                MoonMenuItem::with_key(id(quote.ticker()), quote.ticker()).on_click(
                    move |_, window, app| {
                        let _ = weak.update(app, |this, cx| {
                            this.quote_add(side, list, quote, window, cx);
                        });
                    },
                )
            })
            .collect();
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .children(rows.iter().map(row_el))
            .child(
                h_flex().child(
                    MoonDropdown::new(id("add"))
                        // The plus is a glyph, not text: kept out of the dictionary.
                        .segment(MoonButtonSegment::new("+").color(p.text_muted))
                        .segment(
                            MoonButtonSegment::new(t!("telegram.notify_editor.quote_add"))
                                .color(p.text),
                        )
                        .disabled(addable.is_empty())
                        .trigger_caret(true)
                        .trigger_variant(MoonButtonVariant::Neutral)
                        .trigger_size(MoonButtonSize::density(cx))
                        .menu_width_scaled(160.0)
                        .items(addable),
                ),
            )
            .into_any_element()
    }
}

/// Width of a currency's ticker cell, unscaled.
const TICKER_W: f32 = 48.0;
/// Width of a per-currency amount box, unscaled.
const FIELD_W: f32 = 90.0;
/// Gap between the cells of a currency's row, unscaled.
const ROW_GAP: f32 = 16.0;
/// Gap between a field's caption and its box, unscaled.
const LABEL_GAP: f32 = 8.0;
/// Horizontal padding of a row's remove button, unscaled.
const REMOVE_PAD_X: f32 = 12.0;

/// What a list's per-currency thresholds mean, for the explanation beside its rule: it is not
/// drawn under the picker, where it would widen the controls column.
pub(super) fn quote_hint() -> String {
    t!("telegram.notify_editor.quote_hint").to_string()
}

/// One currency's line: its ticker, each field after its caption, then its remove button.
///
/// Args:
///     ticker: The currency's ticker.
///     fields: Each field's caption and its input.
///     remove: The remove button.
///     cx: Application context for theme and scale.
pub(super) fn quote_row_line(
    ticker: &str,
    fields: Vec<(String, AnyElement)>,
    remove: AnyElement,
    cx: &App,
) -> Div {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    h_flex()
        .flex_wrap()
        .gap(design::ui_px(cx, ROW_GAP))
        .items_center()
        .font_family(design::ui_font())
        .child(
            div()
                .min_w(design::ui_px(cx, TICKER_W))
                .text_color(rgba_from(p.text, 1.0))
                .child(ticker.to_string()),
        )
        .children(fields.into_iter().map(|(label, input)| {
            h_flex()
                .flex_none()
                .gap(design::ui_px(cx, LABEL_GAP))
                .items_center()
                .child(div().text_color(muted).child(label))
                .child(div().w(design::ui_px(cx, FIELD_W)).child(input))
        }))
        .child(remove)
}

/// The remove button's width: its mono caption at the control size, padding and border. Set on
/// the button, so [`quote_row_w`] adds what is drawn.
pub(super) fn remove_w(cx: &App) -> f32 {
    let metrics = design::CONTROL_TIER.control_metrics();
    (design::ui_text_width_zoomed(
        cx,
        t!("telegram.notify_editor.quote_remove").as_ref(),
        metrics.font_size,
        400.0,
        true,
    ) + design::ui_value(cx, 2.0 * (REMOVE_PAD_X + 1.0))
        + 2.0)
        .ceil()
}

/// The width a currency's line of `list` needs to stay on one line: the controls column of its
/// rule's `aside_row` never gets narrower than that, so adding a currency never moves the
/// explanation.
pub(super) fn quote_row_w(list: QuoteList, cx: &App) -> f32 {
    let text = |key: &str| design::ui_body_text_width(cx, t!(key).as_ref(), 400.0) + 2.0;
    let fields: f32 = list
        .labels()
        .iter()
        .map(|key| text(key) + design::ui_value(cx, LABEL_GAP + FIELD_W))
        .sum();
    let n = list.labels().len() as f32;
    let remove = remove_w(cx);
    (design::ui_value(cx, TICKER_W) + fields + remove + (n + 1.0) * design::ui_value(cx, ROW_GAP))
        .ceil()
}

#[cfg(test)]
mod tests;
