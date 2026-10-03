//! The notifications of one chat, picked in the Notifications column of the bot box beside the
//! menu tree — what the Mini App's Settings tab edits: closed-trade cards (cores, volume and result
//! thresholds), core down/back notices, the daily summary — and what only the bot's chat and this
//! column edit: the cards' dollar follow-up and the automatic reports.
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

use super::SettingsView;
use super::access::ChatsOf;
use super::server_bot::known_server;
use crate::backend::station::job::Job;
use crate::design;

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
    loaded_fields: [String; 5],
    /// Why there is nothing to edit for the picked chat, from the last read.
    unavailable: Option<Unavailable>,
    /// The stored row moved on while the editor held unsaved edits.
    stale: bool,
    draft: NotifySettings,
    min_volume: Entity<MoonInputState>,
    profit: Entity<MoonInputState>,
    loss: Entity<MoonInputState>,
    after_minutes: Entity<MoonInputState>,
    daily_time: Entity<MoonInputState>,
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
            daily_time: input(),
            sending: None,
            status: None,
        }
    }

    /// The fields as typed now.
    fn fields(&self, cx: &App) -> [String; 5] {
        [
            &self.min_volume,
            &self.profit,
            &self.loss,
            &self.after_minutes,
            &self.daily_time,
        ]
        .map(|field| field.read(cx).value().to_string())
    }

    /// Whether the editor holds edits the stored row does not.
    fn edited(&self, cx: &App) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|(_, row)| row.settings != self.draft)
            || self.fields(cx) != self.loaded_fields
    }
}

/// A threshold as a field shows it: empty for none.
fn amount_text(value: Option<f64>) -> String {
    value.map(|v| format!("{v}")).unwrap_or_default()
}

/// A threshold typed into a field: empty is none; a comma reads as the decimal point.
fn parse_amount(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim().replace(',', ".");
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(value) if value.is_finite() && value >= 0.0 => Ok(Some(value)),
        _ => Err(()),
    }
}

/// `HH:MM` as typed: an hour of the day and a minute.
fn parse_time(text: &str) -> Result<(u8, u8), ()> {
    let (hour, minute) = text.trim().split_once(':').ok_or(())?;
    let hour: u8 = hour.trim().parse().map_err(|_| ())?;
    let minute: u8 = minute.trim().parse().map_err(|_| ())?;
    (hour < 24 && minute < 60)
        .then_some((hour, minute))
        .ok_or(())
}

/// The fields' texts for a stored row.
fn fields_of(settings: &NotifySettings) -> [String; 5] {
    [
        amount_text(settings.trades.min_volume_usd),
        amount_text(settings.trades.profit_at_least_usd),
        amount_text(settings.trades.loss_at_least_usd),
        settings.down.after_minutes.to_string(),
        format!("{:02}:{:02}", settings.daily.hour, settings.daily.minute),
    ]
}

/// The draft with the fields' numbers in it, or which field does not read.
fn settings_from(
    draft: &NotifySettings,
    fields: [&str; 5],
) -> Result<NotifySettings, &'static str> {
    let [min_volume, profit, loss, after, time] = fields;
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
    let (hour, minute) = parse_time(time).map_err(|_| "telegram.notify_editor.err_time")?;
    settings.daily.hour = hour;
    settings.daily.minute = minute;
    if matches!(&settings.trades.cores, CoreScope::Only(ids) if ids.is_empty()) {
        return Err("telegram.mini_settings_err_cores");
    }
    Ok(settings)
}

impl SettingsView {
    fn notify_ed(&self, side: ChatsOf) -> &NotifyEd {
        &self.chat_ed(side).notify
    }

    fn notify_ed_mut(&mut self, side: ChatsOf) -> &mut NotifyEd {
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
        let ed = self.notify_ed_mut(side);
        ed.unavailable = None;
        ed.stale = false;
        ed.draft = row.settings.clone();
        ed.loaded = Some((chat, row));
        ed.loaded_fields = values.clone();
        let fields = [
            ed.min_volume.clone(),
            ed.profit.clone(),
            ed.loss.clone(),
            ed.after_minutes.clone(),
            ed.daily_time.clone(),
        ];
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

    /// Change `side`'s draft switches.
    fn notify_edit(
        &mut self,
        side: ChatsOf,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut NotifySettings),
    ) {
        let ed = self.notify_ed_mut(side);
        edit(&mut ed.draft);
        ed.status = None;
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
        let Some(revision) = ed
            .loaded
            .as_ref()
            .filter(|(c, _)| *c == chat)
            .map(|(_, row)| row.revision)
        else {
            return;
        };
        let fields = ed.fields(cx);
        let settings = match settings_from(
            &ed.draft,
            [&fields[0], &fields[1], &fields[2], &fields[3], &fields[4]],
        ) {
            Ok(settings) => settings,
            Err(key) => {
                self.notify_ed_mut(side).status = Some((chat, Err(t!(key).to_string())));
                cx.notify();
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
                let access = Access {
                    zone: None,
                    notify: Some(rows),
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
                    let _ = weak.update(app, |this, cx| {
                        this.notify_ed_mut(side).chat = Some(picked);
                        cx.notify();
                    });
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
            let key = match kind {
                AutoReport::Hourly => "telegram.auto.hourly",
                AutoReport::Today => "telegram.auto.today",
                AutoReport::Month => "telegram.auto.month",
            };
            MoonCheckbox::new(id(&format!("auto-{kind:?}")))
                .checked(draft.reports.on(kind))
                .label(t!(key).to_string())
                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                    let v = *v;
                    this.notify_edit(side, cx, |s| s.reports.set(kind, v));
                }))
        };
        let open_basis = self
            .chats(side, cx)
            .is_some_and(|t| t.bot.period_basis == ReportBasis::Open);
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
            .when(!all, |block| block.child(core_list))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 16.0))
                    .child(field(
                        &ed.min_volume,
                        "min-volume",
                        t!("telegram.mini_settings_min_volume").to_string(),
                    ))
                    .child(field(
                        &ed.profit,
                        "profit",
                        t!("telegram.mini_settings_profit_at_least").to_string(),
                    ))
                    .child(field(
                        &ed.loss,
                        "loss",
                        t!("telegram.mini_settings_loss_at_least").to_string(),
                    )),
            )
            .child(switch(
                "usd-followup",
                t!("telegram.notify_editor.usd_followup").to_string(),
                draft.trades.usd_followup,
                |s, v| s.trades.usd_followup = v,
            ))
            .child(
                div().text_color(muted).child(
                    t!(
                        "telegram.notify_editor.trades_hint",
                        minutes = moon_tg::TRADE_HOLD_MINUTES
                    )
                    .to_string(),
                ),
            )
            .child(
                div()
                    .text_color(rgba_from(p.text, 1.0))
                    .child(t!("telegram.notify_editor.events").to_string()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 16.0))
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
                    )),
            )
            .child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.notify_editor.events_hint").to_string()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 16.0))
                    .child(switch(
                        "down",
                        t!("telegram.mini_settings_down").to_string(),
                        draft.down.on,
                        |s, v| s.down.on = v,
                    ))
                    .child(field(
                        &ed.after_minutes,
                        "after",
                        t!("telegram.mini_settings_after_minutes").to_string(),
                    )),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 16.0))
                    .child(switch(
                        "daily",
                        t!("telegram.mini_settings_daily").to_string(),
                        draft.daily.on,
                        |s, v| s.daily.on = v,
                    ))
                    .child(field(
                        &ed.daily_time,
                        "time",
                        t!("telegram.mini_settings_time").to_string(),
                    )),
            )
            .child(
                div()
                    .text_color(rgba_from(p.text, 1.0))
                    .child(t!("telegram.auto.title").to_string()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 16.0))
                    .children(AutoReport::ALL.map(auto)),
            )
            .child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.auto.hint").to_string()),
            )
            .when(open_basis && draft.reports.any(), |block| {
                block.child(
                    div()
                        .text_color(muted)
                        .child(t!("telegram.auto.open_basis").to_string()),
                )
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        MoonButton::new(id("save"))
                            .primary()
                            .padding_x(12.0)
                            .label(t!("telegram.notify_editor.save").to_string())
                            .disabled(
                                side == ChatsOf::Station && self.backend.read(cx).station.busy(),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.notify_save(side, chat, cx);
                            }))
                            .render(),
                    )
                    .when_some(status, |row, status| {
                        let (text, color) = match status {
                            Ok(text) => (text, p.text_muted),
                            Err(reason) => (reason, p.red_text),
                        };
                        row.child(div().text_color(rgba_from(color, 1.0)).child(text))
                    }),
            )
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
