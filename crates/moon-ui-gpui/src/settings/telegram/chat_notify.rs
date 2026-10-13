//! The notifications of one chat, picked in the Notifications column of the bot box beside the
//! menu tree — what the Mini App's Settings tab edits: closed-trade cards (cores, volume and result
//! thresholds) and core down/back notices — and what only the bot's chat and this column edit:
//! the cards' dollar follow-up, their per-currency thresholds (`chat_notify_quotes`), the
//! automatic reports and the cores' own events.
//!
//! The terminal's bot saves them at once into its notifications file; the station's sends them in
//! a change of their own, carrying only this chat's row — not the zone, not a draft of the chats.
//! Either way the save goes through the bot's own checks against the revision the row was read
//! at, so a change made meanwhile from the Mini App or the chat is refused rather than
//! overwritten. A row changed elsewhere while the editor holds unsaved edits is not refilled over
//! them: the editor says so and offers to read it again.
//!
//! The stored row is read once per window render pass ([`SettingsView::chat_notify_sync`]) and
//! kept here; drawing the card reads nothing.

use std::collections::BTreeMap;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::config::telegram_menu::ReportBasis;
use moon_core::station_api::{Access, ChatNotifyRow};
use moon_core::telegram::notify::{AutoReport, CoreScope, NotifySettings};
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonInput,
    MoonInputState, MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use crate::settings::common;

use super::SettingsView;
use super::access::ChatsOf;
use super::chat_notify_quotes::{
    QuoteList, QuoteRowEd, QuoteTexts, quote_texts, refill_rows, rows_from, stored_texts,
};
use super::server_bot::known_server;
use crate::backend::station::job::Job;
use crate::design;

/// Width of a threshold's amount box, unscaled.
const AMOUNT_W: f32 = 72.0;
/// Gap between a threshold's caption and its box, unscaled.
const AMOUNT_LABEL_GAP: f32 = 6.0;
/// Gap between two thresholds of one line, unscaled.
const AMOUNT_GAP: f32 = 12.0;

/// One threshold field: its input state, its id suffix and its caption.
pub(super) type Amount<'a> = (&'a Entity<MoonInputState>, &'a str, String);

/// A line of threshold fields, caption before each box, and the width it needs to stay on one
/// line: the controls column of its [`common::aside_row`] never gets narrower than that.
///
/// Args:
///     amounts: The fields, in order.
///     id: Builds an element id from a field's suffix.
///     cx: Application context for theme, measure and scale.
///
/// Returns:
///     The line and its one-line width in layout pixels.
pub(super) fn amounts_line(
    amounts: Vec<Amount<'_>>,
    id: impl Fn(&str) -> SharedString,
    cx: &App,
) -> (AnyElement, f32) {
    let muted = rgba_from(MoonPalette::active(cx).text_muted, 1.0);
    let n = amounts.len() as f32;
    // A pixel of slack per caption: the estimate rounds glyph advances.
    let captions: f32 = amounts
        .iter()
        .map(|(_, _, label)| design::ui_body_text_width(cx, label, 400.0) + 2.0)
        .sum();
    let need = captions
        + n * design::ui_value(cx, AMOUNT_W + AMOUNT_LABEL_GAP)
        + (n - 1.0).max(0.0) * design::ui_value(cx, AMOUNT_GAP);
    let line = h_flex()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, AMOUNT_GAP))
        .children(amounts.into_iter().map(|(state, what, label)| {
            let field = format!("{}-field", id(what));
            h_flex()
                .debug_selector(move || field.clone())
                .flex_none()
                .gap(design::ui_px(cx, AMOUNT_LABEL_GAP))
                .items_center()
                .child(
                    div()
                        .font_family(design::ui_font())
                        .text_color(muted)
                        .child(label),
                )
                .child(
                    div().w(design::ui_px(cx, AMOUNT_W)).child(
                        MoonInput::new(id(what))
                            .state(state)
                            .size(design::INPUT_SIZE),
                    ),
                )
        }));
    (line.into_any_element(), need)
}

/// Why the picked chat has no notifications to edit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unavailable {
    /// The terminal's bot is not running: its notifications file is not open.
    BotOff,
    /// A chat of the draft, not saved (or applied) yet.
    NotSaved,
    /// The station's answer carries no notifications: it predates them, or is not read now.
    StationSilent,
}

/// The picked chat's notifications as edited: switches and cores here, numbers in the fields.
pub(in crate::settings) struct NotifyEd {
    /// The chat picked in the column. `None`, or a chat no longer in the draft, falls back to the
    /// owner's chat, then to the first one ([`SettingsView::notify_chat`]).
    chat: Option<i64>,
    /// The chat and the stored row the editor was filled from.
    loaded: Option<(i64, ChatNotifyRow)>,
    /// What the fields were filled with, to tell the user's edits from the stored row.
    loaded_fields: [String; FIELDS],
    /// Why there is nothing to edit for the picked chat, from the last read.
    unavailable: Option<Unavailable>,
    /// The stored row moved on while the editor held unsaved edits.
    stale: bool,
    pub(super) draft: NotifySettings,
    min_volume: Entity<MoonInputState>,
    profit: Entity<MoonInputState>,
    loss: Entity<MoonInputState>,
    after_minutes: Entity<MoonInputState>,
    /// The deal chart rule's thresholds (`chat_notify_charts`).
    pub(super) chart_profit: Entity<MoonInputState>,
    pub(super) chart_loss: Entity<MoonInputState>,
    /// The trade rule's per-currency rows (`chat_notify_quotes`).
    pub(super) trade_quotes: Vec<QuoteRowEd>,
    /// The deal chart rule's per-currency rows.
    pub(super) chart_quotes: Vec<QuoteRowEd>,
    /// What the per-currency rows were filled with — trades, then charts — to tell the user's
    /// edits from the stored row.
    loaded_quotes: (QuoteTexts, QuoteTexts),
    /// A save sent to the station for this chat at this revision, awaiting its answer.
    sending: Option<(i64, u64)>,
    /// How the last save of this chat came out.
    status: Option<(i64, Result<String, String>)>,
}

impl NotifyEd {
    /// An empty editor; [`SettingsView::chat_notify_sync`] fills it for the picked chat.
    pub(in crate::settings) fn new<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Self {
        let mut input = || cx.new(|cx| MoonInputState::new(window, cx));
        Self {
            chat: None,
            loaded: None,
            loaded_fields: Default::default(),
            unavailable: None,
            stale: false,
            draft: NotifySettings::default(),
            min_volume: input(),
            profit: input(),
            loss: input(),
            after_minutes: input(),
            chart_profit: input(),
            chart_loss: input(),
            trade_quotes: Vec::new(),
            chart_quotes: Vec::new(),
            loaded_quotes: Default::default(),
            sending: None,
            status: None,
        }
    }

    /// The fields, in [`fields_of`]'s order.
    fn inputs(&self) -> [&Entity<MoonInputState>; FIELDS] {
        [
            &self.min_volume,
            &self.profit,
            &self.loss,
            &self.after_minutes,
            &self.chart_profit,
            &self.chart_loss,
        ]
    }

    /// The fields as typed now.
    fn fields(&self, cx: &App) -> [String; FIELDS] {
        self.inputs()
            .map(|field| field.read(cx).value().to_string())
    }

    /// The per-currency rows as typed now: trades, then charts.
    fn quotes(&self, cx: &App) -> (QuoteTexts, QuoteTexts) {
        (
            quote_texts(&self.trade_quotes, cx),
            quote_texts(&self.chart_quotes, cx),
        )
    }

    /// Whether the editor holds edits the stored row does not.
    fn edited(&self, cx: &App) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|(_, row)| row.settings != self.draft)
            || self.fields(cx) != self.loaded_fields
            || self.quotes(cx) != self.loaded_quotes
    }

    /// Forget how the last save came out: the editor holds a new edit.
    pub(super) fn clear_status(&mut self) {
        self.status = None;
    }
}

/// How many number fields the editor has.
const FIELDS: usize = 6;

/// A threshold as a field shows it: empty for none.
fn amount_text(value: Option<f64>) -> String {
    value.map(|v| format!("{v}")).unwrap_or_default()
}

/// The toggle schedule shared with the bot menu, separate from delivered report titles.
fn auto_label(kind: AutoReport) -> String {
    let key = match kind {
        AutoReport::Hourly => "telegram.auto.toggle_hourly",
        AutoReport::Today => "telegram.auto.today",
        AutoReport::Month => "telegram.auto.month",
    };
    t!(key).to_string()
}

/// A threshold typed into a field: empty is none; a comma reads as the decimal point.
pub(super) fn parse_amount(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim().replace(',', ".");
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(value) if value.is_finite() && value >= 0.0 => Ok(Some(value)),
        _ => Err(()),
    }
}

/// The fields' texts for a stored row.
fn fields_of(settings: &NotifySettings) -> [String; FIELDS] {
    [
        amount_text(settings.trades.min_volume_usd),
        amount_text(settings.trades.profit_at_least_usd),
        amount_text(settings.trades.loss_at_least_usd),
        settings.down.after_minutes.to_string(),
        amount_text(settings.charts.profit_at_least_usd),
        amount_text(settings.charts.loss_at_least_usd),
    ]
}

/// The draft with the fields' numbers in it, or which field does not read.
fn settings_from(
    draft: &NotifySettings,
    fields: [&str; FIELDS],
) -> Result<NotifySettings, &'static str> {
    let [min_volume, profit, loss, after, chart_profit, chart_loss] = fields;
    let mut settings = draft.clone();
    settings.trades.min_volume_usd =
        parse_amount(min_volume).map_err(|_| "telegram.notify_editor.err_amount")?;
    settings.trades.profit_at_least_usd =
        parse_amount(profit).map_err(|_| "telegram.notify_editor.err_amount")?;
    settings.trades.loss_at_least_usd =
        parse_amount(loss).map_err(|_| "telegram.notify_editor.err_amount")?;
    settings.down.after_minutes = after
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|m| (1..=1440).contains(m))
        .ok_or("telegram.notify_editor.err_minutes")?;
    settings.charts.profit_at_least_usd =
        parse_amount(chart_profit).map_err(|_| "telegram.notify_editor.err_amount")?;
    settings.charts.loss_at_least_usd =
        parse_amount(chart_loss).map_err(|_| "telegram.notify_editor.err_amount")?;
    if matches!(&settings.trades.cores, CoreScope::Only(ids) if ids.is_empty()) {
        return Err("telegram.mini_settings_err_cores");
    }
    Ok(settings)
}

/// What committing the editor of `chat` does now.
#[derive(Debug, PartialEq)]
enum Commit {
    /// Save these settings against the revision the row was read at.
    Save(NotifySettings, u64),
    /// Nothing is saved; the reason is shown and the edits stay.
    Refuse(&'static str),
    /// The editor holds nothing the stored row does not.
    Nothing,
}

/// Decide a commit: the edits are saved only when they read, differ from the stored row and the
/// station can take them now. The revision is always the one the row was read at, so a row
/// changed elsewhere meanwhile is refused by the bot rather than overwritten.
///
/// Args:
///     loaded: The chat and stored row the editor was filled from.
///     chat: The chat being committed.
///     draft: Switches and cores as edited.
///     fields: The number fields as typed.
///     quotes: The per-currency rows as typed, trades then charts.
///     station_busy: A station job runs and would drop this one.
fn commit_plan(
    loaded: Option<&(i64, ChatNotifyRow)>,
    chat: i64,
    draft: &NotifySettings,
    fields: [&str; FIELDS],
    quotes: &(QuoteTexts, QuoteTexts),
    station_busy: bool,
) -> Commit {
    let Some((_, row)) = loaded.filter(|(c, _)| *c == chat) else {
        return Commit::Nothing;
    };
    let settings = match settings_with_quotes(draft, fields, quotes) {
        Ok(settings) => settings,
        Err(key) => return Commit::Refuse(key),
    };
    if settings == row.settings || settings == as_edited(&row.settings) {
        return Commit::Nothing;
    }
    if station_busy {
        return Commit::Refuse("telegram.mini_error_busy_station");
    }
    Commit::Save(settings, row.revision)
}

/// `stored` as an untouched editor would write it back: the per-currency rows rebuilt from their
/// fields put a stored lowercase ticker in canonical case and a row of unknown currency last. An
/// editor holding exactly that has nothing to save — without this, a stored list in another shape
/// would be written again, and its revision bumped, on every Blur.
fn as_edited(stored: &NotifySettings) -> NotifySettings {
    let fields = fields_of(stored);
    let quotes = (
        stored_texts(QuoteList::Trades, stored),
        stored_texts(QuoteList::Charts, stored),
    );
    settings_with_quotes(stored, fields.each_ref().map(String::as_str), &quotes)
        .unwrap_or_else(|_| stored.clone())
}

/// Whether the chat picker may leave the chat after its edits were committed: not while they
/// were refused or are still on their way to the station — the editor would be refilled over
/// them.
fn may_switch(refused: bool, sending: bool) -> bool {
    !refused && !sending
}

/// [`settings_from`] with the per-currency rows as typed (trades, then charts) in place of the
/// stored lists. A stored row naming a currency this build does not know has no row in the editor
/// and is kept as stored, after the typed ones: the editor cannot show it, so it must not delete it.
fn settings_with_quotes(
    draft: &NotifySettings,
    fields: [&str; FIELDS],
    (trades, charts): &(QuoteTexts, QuoteTexts),
) -> Result<NotifySettings, &'static str> {
    let mut settings = settings_from(draft, fields)?;
    let unknown = |rows: &[moon_core::telegram::notify::QuoteThresholds]| {
        rows.iter()
            .filter(|row| row.currency().is_none())
            .cloned()
            .collect::<Vec<_>>()
    };
    settings.trades.by_quote = rows_from(QuoteList::Trades, trades)?;
    settings
        .trades
        .by_quote
        .extend(unknown(&draft.trades.by_quote));
    settings.charts.by_quote = rows_from(QuoteList::Charts, charts)?;
    settings
        .charts
        .by_quote
        .extend(unknown(&draft.charts.by_quote));
    Ok(settings)
}

impl SettingsView {
    pub(super) fn notify_ed(&self, side: ChatsOf) -> &NotifyEd {
        &self.chat_ed(side).notify
    }

    pub(super) fn notify_ed_mut(&mut self, side: ChatsOf) -> &mut NotifyEd {
        &mut self.chat_ed_mut(side).notify
    }

    /// The chat `side`'s Notifications column edits: the one picked while it is still in the
    /// draft, else the owner's, else the first; `None` without chats.
    fn notify_chat(&self, side: ChatsOf, cx: &App) -> Option<i64> {
        let telegram = self.chats(side, cx)?;
        let chats = &telegram.authorized_chat_ids;
        self.notify_ed(side)
            .chat
            .filter(|chat| chats.contains(chat))
            .or_else(|| telegram.owner().filter(|chat| chats.contains(chat)))
            .or_else(|| chats.first().copied())
    }

    /// `chat`'s stored notifications on `side`: the terminal bot's file, against its saved chats;
    /// or the station's as last read.
    fn notify_row(&self, side: ChatsOf, chat: i64, cx: &App) -> Result<ChatNotifyRow, Unavailable> {
        match side {
            ChatsOf::Terminal => {
                let b = self.backend.read(cx);
                if !b.config.telegram.authorized_chat_ids.contains(&chat) {
                    return Err(Unavailable::NotSaved);
                }
                moon_tg::notify_rows(&b.telegram, &b.config.telegram)
                    .ok_or(Unavailable::BotOff)?
                    .remove(&chat)
                    .ok_or(Unavailable::NotSaved)
            }
            ChatsOf::Station => {
                let seen = self
                    .telegram
                    .server
                    .access_seen()
                    .ok_or(Unavailable::StationSilent)?;
                if !seen.authorized_chat_ids.contains(&chat) {
                    return Err(Unavailable::NotSaved);
                }
                seen.notify
                    .as_ref()
                    .ok_or(Unavailable::StationSilent)?
                    .get(&chat)
                    .cloned()
                    .ok_or(Unavailable::StationSilent)
            }
        }
    }

    /// Read each side's picked chat's stored row and fill its editor when the chat or the row
    /// changed — unless the user holds unsaved edits, which stay and are marked stale. Also ends
    /// a save sent to the station once its answer is in. Runs at the window's render root, where
    /// the fields' window is.
    pub(in crate::settings) fn chat_notify_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A save sent to the station ends once its job has, whichever chat is open by then.
        if let Some((sent_chat, _)) = self.notify_ed(ChatsOf::Station).sending {
            let row = self.notify_row(ChatsOf::Station, sent_chat, cx).ok();
            self.notify_station_answer(row.as_ref(), cx);
        }
        for side in [ChatsOf::Terminal, ChatsOf::Station] {
            let Some(chat) = self.notify_chat(side, cx) else {
                let ed = self.notify_ed_mut(side);
                ed.loaded = None;
                ed.stale = false;
                continue;
            };
            // While this chat's save is on its way the editor keeps what was sent.
            if self
                .notify_ed(side)
                .sending
                .is_some_and(|(sent, _)| sent == chat)
            {
                continue;
            }
            let row = self.notify_row(side, chat, cx);
            let row = match row {
                Ok(row) => row,
                Err(why) => {
                    self.notify_ed_mut(side).unavailable = Some(why);
                    continue;
                }
            };
            let ed = self.notify_ed(side);
            let same_chat = ed.loaded.as_ref().is_some_and(|(c, _)| *c == chat);
            let same_row = ed
                .loaded
                .as_ref()
                .is_some_and(|(c, r)| *c == chat && *r == row);
            if same_row {
                self.notify_ed_mut(side).unavailable = None;
                continue;
            }
            if same_chat && ed.edited(cx) {
                let ed = self.notify_ed_mut(side);
                ed.unavailable = None;
                ed.stale = true;
                continue;
            }
            self.notify_fill(side, chat, row, window, cx);
        }
    }

    /// Fill `side`'s editor with `chat`'s stored `row`.
    fn notify_fill(
        &mut self,
        side: ChatsOf,
        chat: i64,
        row: ChatNotifyRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let values = fields_of(&row.settings);
        let quotes = (
            stored_texts(QuoteList::Trades, &row.settings),
            stored_texts(QuoteList::Charts, &row.settings),
        );
        let ed = self.notify_ed_mut(side);
        let old = (
            std::mem::take(&mut ed.trade_quotes),
            std::mem::take(&mut ed.chart_quotes),
        );
        let trade_quotes = refill_rows(side, old.0, &quotes.0, window, cx);
        let chart_quotes = refill_rows(side, old.1, &quotes.1, window, cx);
        let ed = self.notify_ed_mut(side);
        ed.trade_quotes = trade_quotes;
        ed.chart_quotes = chart_quotes;
        ed.loaded_quotes = quotes;
        ed.unavailable = None;
        ed.stale = false;
        ed.draft = row.settings.clone();
        ed.loaded = Some((chat, row));
        ed.loaded_fields = values.clone();
        let fields = ed.inputs().map(Clone::clone);
        for (field, value) in fields.into_iter().zip(values) {
            field.update(cx, |state, cx| state.set_value(value, window, cx));
        }
    }

    /// End a save sent to the station once its job has: saved when the sent chat's stored row
    /// (`row`, as now read) moved past the revision it was sent at — then the editor reads it
    /// again; else the job's error, or "not confirmed" when the job ended well but no read shows
    /// the new row — the edits stay either way.
    fn notify_station_answer(&mut self, row: Option<&ChatNotifyRow>, cx: &App) {
        let Some((sent_chat, revision)) = self.notify_ed(ChatsOf::Station).sending else {
            return;
        };
        if self.backend.read(cx).station.running {
            return;
        }
        let saved = row.is_some_and(|row| row.revision > revision);
        let failure = match &self.backend.read(cx).station.outcome {
            Some(Err(reason)) => reason.clone(),
            _ => t!("telegram.notify_editor.not_confirmed").to_string(),
        };
        let ed = self.notify_ed_mut(ChatsOf::Station);
        ed.sending = None;
        if saved {
            ed.loaded = None;
        }
        ed.status = Some((
            sent_chat,
            match saved {
                true => Ok(t!("telegram.notify_editor.saved").to_string()),
                false => Err(failure),
            },
        ));
    }

    /// Change `side`'s draft switches and apply them at once.
    pub(super) fn notify_edit(
        &mut self,
        side: ChatsOf,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut NotifySettings),
    ) {
        let ed = self.notify_ed_mut(side);
        edit(&mut ed.draft);
        ed.status = None;
        if let Some(chat) = ed.loaded.as_ref().map(|(chat, _)| *chat) {
            self.notify_save(side, chat, cx);
        }
        cx.notify();
    }

    /// Apply the number fields of `side`'s editor once one is left or confirmed with Enter.
    pub(super) fn wire_notify(side: ChatsOf, ed: &NotifyEd, cx: &mut Context<Self>) {
        for field in ed.inputs() {
            cx.subscribe(field, move |this, _, ev: &moon_ui::MoonInputEvent, cx| {
                if matches!(
                    ev,
                    moon_ui::MoonInputEvent::Blur | moon_ui::MoonInputEvent::PressEnter { .. }
                ) && let Some(chat) = this.notify_ed(side).loaded.as_ref().map(|(c, _)| *c)
                {
                    this.notify_save(side, chat, cx);
                }
            })
            .detach();
        }
    }

    /// Apply the edits of the chat `side`'s editor was filled for, if any: what a per-currency row
    /// does on add, remove, leave or Enter (`chat_notify_quotes`).
    pub(super) fn notify_commit_open(&mut self, side: ChatsOf, cx: &mut Context<Self>) {
        if let Some(chat) = self.notify_ed(side).loaded.as_ref().map(|(chat, _)| *chat) {
            self.notify_save(side, chat, cx);
        }
    }

    /// Commit the open chat's edits before the picker shows `picked`; stay on the open chat when
    /// they were refused or are still being sent.
    fn notify_pick(&mut self, side: ChatsOf, picked: i64, cx: &mut Context<Self>) {
        if let Some(open) = self.notify_chat(side, cx).filter(|open| *open != picked) {
            self.notify_save(side, open, cx);
            let ed = self.notify_ed(side);
            let refused = matches!(&ed.status, Some((c, Err(_))) if *c == open);
            if !may_switch(refused, ed.sending.is_some()) {
                cx.notify();
                return;
            }
        }
        self.notify_ed_mut(side).chat = Some(picked);
        cx.notify();
    }

    /// Drop the edits and read the stored row again.
    fn notify_reload(&mut self, side: ChatsOf, cx: &mut Context<Self>) {
        let ed = self.notify_ed_mut(side);
        ed.loaded = None;
        ed.stale = false;
        ed.status = None;
        cx.notify();
    }

    /// Save `chat`'s notifications on `side` against the revision they were read at.
    fn notify_save(&mut self, side: ChatsOf, chat: i64, cx: &mut Context<Self>) {
        let ed = self.notify_ed(side);
        if ed.sending.is_some() {
            return;
        }
        let fields = ed.fields(cx);
        let quotes = ed.quotes(cx);
        let busy = side == ChatsOf::Station && self.backend.read(cx).station.busy();
        let plan = commit_plan(
            ed.loaded.as_ref(),
            chat,
            &ed.draft,
            fields.each_ref().map(String::as_str),
            &quotes,
            busy,
        );
        let (settings, revision) = match plan {
            Commit::Save(settings, revision) => (settings, revision),
            Commit::Refuse(key) => {
                self.notify_ed_mut(side).status = Some((chat, Err(t!(key).to_string())));
                cx.notify();
                return;
            }
            Commit::Nothing => {
                let ed = self.notify_ed_mut(side);
                if matches!(&ed.status, Some((c, Err(_))) if *c == chat) {
                    ed.status = None;
                }
                return;
            }
        };
        let rows = BTreeMap::from([(chat, ChatNotifyRow { settings, revision })]);
        match side {
            ChatsOf::Terminal => {
                let result = self.backend.update(cx, |b, bcx| {
                    let zone = moon_core::util::display_time::zone_or_utc(b.header_clock_zone());
                    let result =
                        moon_tg::save_notify_rows(&b.telegram, &b.config.telegram, &rows, zone);
                    bcx.notify();
                    result
                });
                let ed = self.notify_ed_mut(side);
                if result.is_ok() {
                    // Read back at once: the saved row, not a stale mark over the user's edits.
                    ed.loaded = None;
                }
                ed.status = Some((
                    chat,
                    result.map(|()| t!("telegram.notify_editor.saved").to_string()),
                ));
            }
            ChatsOf::Station => {
                // This chat's row alone, on the chats and the menu exactly as last read: no zone
                // (the terminal pushes it on its own) and no other chat's row.
                let (Some(seen), Some(target)) =
                    (self.telegram.server.access_seen().cloned(), known_server())
                else {
                    return;
                };
                // Nor the groups: sent back as read, they would take back a set another
                // terminal sent since.
                let access = Access {
                    zone: None,
                    notify: Some(rows),
                    groups: None,
                    ..seen.clone()
                };
                let base = Access {
                    notify: None,
                    ..seen
                };
                self.server_bot_run(
                    Ok(Job::Access {
                        target,
                        base,
                        access,
                        edits: false,
                    }),
                    cx,
                );
                let ed = self.notify_ed_mut(side);
                ed.sending = Some((chat, revision));
                ed.status = None;
            }
        }
        cx.notify();
    }

    /// `side`'s Notifications column: the chat picker, then that chat's notifications.
    pub(in crate::settings) fn chat_notify_column(
        &self,
        side: ChatsOf,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let column = v_flex().w_full().min_w_0().gap(design::ui_px(cx, 10.0));
        let Some(telegram) = self.chats(side, cx) else {
            return column.into_any_element();
        };
        let Some(chat) = self.notify_chat(side, cx) else {
            return column
                .child(
                    div()
                        .text_color(muted)
                        .child(t!("telegram.notify_editor.no_chats").to_string()),
                )
                .into_any_element();
        };
        let id = |what: &str| -> SharedString {
            match side {
                ChatsOf::Terminal => format!("tgn-{what}").into(),
                ChatsOf::Station => format!("tgns-{what}").into(),
            }
        };
        let items = {
            let weak = cx.entity().downgrade();
            crate::panels::radio_items(
                telegram.authorized_chat_ids.iter().map(|&each| {
                    (
                        each,
                        SharedString::from(format!("{}-{each}", id("chat"))),
                        SharedString::from(super::access::chat_title(telegram, each)),
                    )
                }),
                chat,
                crate::panels::RadioMark::Check,
                move |app, picked| {
                    let _ = weak.update(app, |this, cx| this.notify_pick(side, picked, cx));
                },
            )
        };
        column
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("telegram.notify_editor.chat").to_string()),
                    )
                    .child(
                        // The editor holds the chat a station save was sent for until its answer
                        // is in; another chat filled meanwhile would be refilled over by it.
                        MoonDropdown::new(id("chat-pick"))
                            .disabled(self.notify_ed(side).sending.is_some())
                            .label(super::access::chat_title(telegram, chat))
                            .trigger_caret(true)
                            .trigger_variant(MoonButtonVariant::Neutral)
                            .trigger_size(MoonButtonSize::density(cx))
                            .trigger_width_scaled(200.0)
                            .menu_width_scaled(240.0)
                            .items(items),
                    ),
            )
            .child(self.chat_notify_box(side, chat, cx))
            .into_any_element()
    }

    /// The picked chat's notifications on `side`.
    fn chat_notify_box(&self, side: ChatsOf, chat: i64, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let id = |what: &str| -> SharedString {
            match side {
                ChatsOf::Terminal => format!("tgn-{what}-{chat}").into(),
                ChatsOf::Station => format!("tgns-{what}-{chat}").into(),
            }
        };
        let block = v_flex().gap(design::ui_px(cx, 8.0));
        let ed = self.notify_ed(side);
        let sending = ed.sending.is_some_and(|(c, _)| c == chat);
        if sending {
            return block
                .child(
                    div()
                        .text_color(muted)
                        .child(t!("telegram.server.applying").to_string()),
                )
                .into_any_element();
        }
        let loaded = ed.loaded.as_ref().is_some_and(|(c, _)| *c == chat);
        if let Some(why) = ed.unavailable.filter(|_| !loaded) {
            let why = match why {
                Unavailable::BotOff => t!("telegram.notify_editor.bot_off"),
                Unavailable::NotSaved => match side {
                    ChatsOf::Terminal => t!("telegram.notify_editor.save_chats_first"),
                    ChatsOf::Station => t!("telegram.notify_editor.apply_chats_first"),
                },
                Unavailable::StationSilent => t!("telegram.notify_editor.station_too_old"),
            };
            return block
                .child(div().text_color(muted).child(why.to_string()))
                .into_any_element();
        }
        if !loaded {
            return block.into_any_element();
        }
        let draft = &ed.draft;
        let switch = |what: &str, label: String, on: bool, set: fn(&mut NotifySettings, bool)| {
            MoonCheckbox::new(id(what))
                .checked(on)
                .label(label)
                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                    let v = *v;
                    this.notify_edit(side, cx, |s| set(s, v));
                }))
        };
        // The cores a rule may name: every core for the owner, the granted ones for a viewer.
        let cores = self.notify_cores(side, chat, cx);
        let all = matches!(draft.trades.cores, CoreScope::All);
        let picked = match &draft.trades.cores {
            CoreScope::Only(ids) => ids.clone(),
            CoreScope::All => Vec::new(),
        };
        let all_ids: Vec<u64> = cores.iter().map(|(id, _)| *id).collect();
        let core_list =
            h_flex()
                .flex_wrap()
                .gap(design::ui_px(cx, 10.0))
                .children(cores.into_iter().map(|(core, name)| {
                    MoonCheckbox::new(id(&format!("core-{core}")))
                        .checked(picked.contains(&core))
                        .label(name)
                        .on_change(cx.listener(move |this, v: &bool, _, cx| {
                            let v = *v;
                            this.notify_edit(side, cx, |s| {
                                if let CoreScope::Only(ids) = &mut s.trades.cores {
                                    ids.retain(|id| *id != core);
                                    if v {
                                        ids.push(core);
                                    }
                                }
                            });
                        }))
                }));
        let status = ed
            .status
            .as_ref()
            .filter(|(c, _)| *c == chat)
            .map(|(_, status)| status.clone());
        let auto = |kind: AutoReport| {
            MoonCheckbox::new(id(&format!("auto-{kind:?}")))
                .checked(draft.reports.on(kind))
                .label(auto_label(kind))
                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                    let v = *v;
                    this.notify_edit(side, cx, |s| s.reports.set(kind, v));
                }))
        };
        let open_basis = self
            .chats(side, cx)
            .is_some_and(|t| t.bot.period_basis == ReportBasis::Open);
        let mut auto_hint = t!("telegram.auto.hint").to_string();
        if open_basis && draft.reports.any() {
            auto_hint.push_str("\n\n");
            auto_hint.push_str(&t!("telegram.auto.open_basis"));
        }
        let (trade_amounts, trade_amounts_w) = amounts_line(
            vec![
                (
                    &ed.min_volume,
                    "min-volume",
                    t!("telegram.mini_settings_min_volume").to_string(),
                ),
                (
                    &ed.profit,
                    "profit",
                    t!("telegram.mini_settings_profit_at_least").to_string(),
                ),
                (
                    &ed.loss,
                    "loss",
                    t!("telegram.mini_settings_loss_at_least").to_string(),
                ),
            ],
            id,
            cx,
        );
        block
            .when(ed.stale, |block| {
                block.child(
                    h_flex()
                        .flex_wrap()
                        .gap(design::ui_px(cx, 8.0))
                        .items_center()
                        .child(
                            div()
                                .text_color(rgba_from(p.red_text, 1.0))
                                .child(t!("telegram.notify_editor.stale").to_string()),
                        )
                        .child(
                            MoonButton::new(id("reload"))
                                .ghost()
                                .padding_x(12.0)
                                .label(t!("telegram.notify_editor.reload").to_string())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.notify_reload(side, cx);
                                }))
                                .render(),
                        ),
                )
            })
            .when_some(status, |block, status| {
                let (text, color) = match status {
                    Ok(text) => (text, p.text_muted),
                    Err(reason) => (reason, p.red_text),
                };
                block.child(
                    div()
                        .debug_selector(|| "tg-notify-status".into())
                        .text_color(rgba_from(color, 1.0))
                        .child(text),
                )
            })
            .child(super::super::common::section(
                &t!("telegram.notify.sec.closed"),
                p,
                cx,
            ))
            .child(common::aside_row(
                &id("row-trades"),
                vec![
                    h_flex()
                        .flex_wrap()
                        .gap(design::ui_px(cx, 20.0))
                        .child(switch(
                            "trades",
                            t!("telegram.mini_settings_trades").to_string(),
                            draft.trades.on,
                            |s, v| s.trades.on = v,
                        ))
                        .child(
                            MoonCheckbox::new(id("all-cores"))
                                .checked(all)
                                .label(t!("telegram.mini_settings_all_cores").to_string())
                                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                                    let v = *v;
                                    let ids = all_ids.clone();
                                    this.notify_edit(side, cx, |s| {
                                        s.trades.cores = if v {
                                            CoreScope::All
                                        } else {
                                            CoreScope::Only(ids)
                                        };
                                    });
                                })),
                        )
                        .into_any_element(),
                ]
                .into_iter()
                .chain((!all).then(|| core_list.into_any_element()))
                .chain(std::iter::once(trade_amounts))
                .chain(std::iter::once(self.quote_block(
                    side,
                    chat,
                    QuoteList::Trades,
                    cx,
                )))
                .collect(),
                Some(t!("telegram.mini_settings_trades_hint").to_string()),
                trade_amounts_w,
                false,
                cx,
            ))
            .child(common::aside_row(
                &id("row-usd"),
                vec![
                    switch(
                        "usd-followup",
                        t!("telegram.notify_editor.usd_followup").to_string(),
                        draft.trades.usd_followup,
                        |s, v| s.trades.usd_followup = v,
                    )
                    .into_any_element(),
                ],
                Some(
                    t!(
                        "telegram.notify_editor.trades_hint",
                        minutes = moon_tg::TRADE_HOLD_MINUTES
                    )
                    .to_string(),
                ),
                0.0,
                side != ChatsOf::Station,
                cx,
            ))
            .when(side == ChatsOf::Station, |block| {
                let (controls, hint, min_w) = self.chart_notify_parts(side, chat, cx);
                block.child(common::aside_row(
                    &id("row-charts"),
                    controls,
                    Some(hint),
                    min_w,
                    true,
                    cx,
                ))
            })
            .child(super::super::common::section(
                &t!("telegram.notify.sec.core"),
                p,
                cx,
            ))
            .child(common::aside_row(
                &id("row-events"),
                vec![
                    h_flex()
                        .flex_wrap()
                        .gap(design::ui_px(cx, 20.0))
                        .child(switch(
                            "opened",
                            t!("telegram.notify_editor.opened").to_string(),
                            draft.events.opened,
                            |s, v| s.events.opened = v,
                        ))
                        .child(switch(
                            "detects",
                            t!("telegram.notify_editor.detects").to_string(),
                            draft.events.detects,
                            |s, v| s.events.detects = v,
                        ))
                        .into_any_element(),
                ],
                Some(t!("telegram.notify_editor.events_hint").to_string()),
                0.0,
                false,
                cx,
            ))
            .child(common::aside_row(
                &id("row-down"),
                vec![
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap(design::ui_px(cx, 14.0))
                        .child(switch(
                            "down",
                            t!("telegram.mini_settings_down").to_string(),
                            draft.down.on,
                            |s, v| s.down.on = v,
                        ))
                        .child(
                            h_flex()
                                .gap(design::ui_px(cx, 8.0))
                                .items_center()
                                .child(
                                    div().text_color(muted).child(
                                        t!("telegram.mini_settings_after_minutes").to_string(),
                                    ),
                                )
                                .child(super::super::common::num_input(
                                    "tg-notify-after",
                                    &ed.after_minutes,
                                    1.0..=1440.0,
                                    1.0,
                                    None,
                                )),
                        )
                        .into_any_element(),
                ],
                None,
                0.0,
                true,
                cx,
            ))
            .child(super::super::common::section(
                &t!("telegram.notify.sec.auto"),
                p,
                cx,
            ))
            .child(common::aside_row(
                &id("row-auto"),
                AutoReport::ALL
                    .map(|kind| auto(kind).into_any_element())
                    .into_iter()
                    .collect(),
                Some(auto_hint),
                0.0,
                true,
                cx,
            ))
            .into_any_element()
    }

    /// The cores `chat` may name in a trade rule, by name: every configured core for the owner;
    /// for a viewer, the cores of its grant as the bot holds it (saved, or as the station read
    /// it) — the grant the save is checked against, not a draft's.
    fn notify_cores(&self, side: ChatsOf, chat: i64, cx: &App) -> Vec<(u64, String)> {
        let b = self.backend.read(cx);
        let granted = match side {
            ChatsOf::Terminal => b.config.telegram.report_access(chat),
            ChatsOf::Station => self.telegram.server.access_seen().and_then(|seen| {
                let mut telegram = moon_core::config::TelegramConfig::default();
                seen.apply_to(&mut telegram);
                telegram.report_access(chat)
            }),
        };
        let granted = match granted {
            Some(TelegramReportAccess::Viewer(ids)) => Some(ids),
            _ => None,
        };
        b.config
            .servers
            .iter()
            .filter(|s| s.uid != 0)
            .filter(|s| granted.as_ref().is_none_or(|ids| ids.contains(&s.uid)))
            .map(|s| (s.uid, s.name.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests;
