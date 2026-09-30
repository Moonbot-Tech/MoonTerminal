//! Per-chat role and core assignment editor, for either bot: the terminal's (edits the Settings
//! draft, saved by the shared Save transaction) or the station's (edits a draft of its own, sent
//! to the server by "Apply on the server").
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::TelegramConfig;
use moon_core::config::telegram_access::{TelegramChatAccess, TelegramReportAccess};
use moon_ui::{
    MoonButton, MoonCheckbox, MoonGroupBox, MoonInput, MoonInputEvent, MoonInputState, MoonPalette,
    h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::design;
use moon_core::session::core_order::CoreOrder;

/// Whose chats an editor shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::settings) enum ChatsOf {
    /// The terminal's own bot: the Settings draft, saved by Save.
    Terminal,
    /// The bot on the station: a draft of the server's chats, applied by a button.
    Station,
}

impl ChatsOf {
    /// An element id unique to this editor: both can be on screen at once.
    fn id(self, what: impl std::fmt::Display) -> SharedString {
        match self {
            Self::Terminal => format!("tg-{what}").into(),
            Self::Station => format!("tgs-{what}").into(),
        }
    }
}

/// One chat editor's own state: the chat opened, an ownership transfer awaiting its second click,
/// the opened chat's name and the core search.
pub(in crate::settings) struct ChatEd {
    pub(super) active_chat: Option<i64>,
    /// Ownership transfer requires a second explicit click within the selected chat.
    pub(super) pending_owner: Option<i64>,
    name: Entity<MoonInputState>,
    search: Entity<MoonInputState>,
}

impl ChatEd {
    /// A closed editor; [`Self::wire`] binds its name field to a side's draft.
    pub(in crate::settings) fn new<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Self {
        let name = cx.new(|cx| MoonInputState::new(window, cx));
        let search = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&search, |_, _, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        Self {
            active_chat: None,
            pending_owner: None,
            name,
            search,
        }
    }

    /// Write the opened chat's caption into `side`'s draft as it is typed.
    pub(in crate::settings) fn wire(&self, side: ChatsOf, cx: &mut Context<SettingsView>) {
        cx.subscribe(&self.name, move |this, emitter, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let Some(chat) = this.chat_ed(side).active_chat else {
                    return;
                };
                let value = emitter.read(cx).value().to_string();
                this.chats_edit(side, cx, |telegram| {
                    // Opening a chat fills this field too: an unchanged name must not create
                    // an empty profile, which would read as an edit.
                    let current = telegram
                        .chat_access
                        .iter()
                        .find(|a| a.chat_id == chat)
                        .map_or("", |a| a.name.as_str());
                    if !telegram.authorized_chat_ids.contains(&chat) || current == value {
                        return false;
                    }
                    telegram.chat_profile_mut(chat).name = value;
                    true
                });
            }
        })
        .detach();
    }

    /// Close the opened chat: its chats were replaced from elsewhere.
    pub(in crate::settings) fn close(&mut self) {
        self.active_chat = None;
        self.pending_owner = None;
    }
}

impl SettingsView {
    fn chat_ed(&self, side: ChatsOf) -> &ChatEd {
        match side {
            ChatsOf::Terminal => &self.telegram.chats,
            ChatsOf::Station => &self.telegram.server.chats,
        }
    }

    fn chat_ed_mut(&mut self, side: ChatsOf) -> &mut ChatEd {
        match side {
            ChatsOf::Terminal => &mut self.telegram.chats,
            ChatsOf::Station => &mut self.telegram.server.chats,
        }
    }

    /// `side`'s chats as edited; `None` while the station's are not read yet.
    fn chats<'a>(&'a self, side: ChatsOf, cx: &'a App) -> Option<&'a TelegramConfig> {
        match side {
            ChatsOf::Terminal => {
                let b = self.backend.read(cx);
                Some(&b.preview.as_ref().unwrap_or(&b.config).telegram)
            }
            ChatsOf::Station => self.telegram.server.access_draft.as_ref(),
        }
    }

    /// `side`'s grants as saved: they stay offered after being unchecked in the draft.
    fn chats_saved<'a>(&'a self, side: ChatsOf, cx: &'a App) -> &'a [TelegramChatAccess] {
        match side {
            ChatsOf::Terminal => &self.backend.read(cx).config.telegram.chat_access,
            ChatsOf::Station => self
                .telegram
                .server
                .access_base
                .as_ref()
                .map_or(&[], |a| a.chat_access.as_slice()),
        }
    }

    /// Change `side`'s draft; `edit` says whether it changed anything.
    fn chats_edit(
        &mut self,
        side: ChatsOf,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut TelegramConfig) -> bool,
    ) {
        match side {
            ChatsOf::Terminal => self.backend.update(cx, |b, bcx| {
                if let Some(draft) = b.preview.as_mut()
                    && edit(&mut draft.telegram)
                {
                    bcx.notify();
                }
            }),
            ChatsOf::Station => {
                if let Some(draft) = self.telegram.server.access_draft.as_mut() {
                    edit(draft);
                }
            }
        }
        cx.notify();
    }

    /// Load archived candidates off GPUI without deriving the catalog from mutable checkbox state.
    fn load_telegram_history(&mut self, cx: &mut Context<Self>) {
        if self.telegram.history_loaded || self.telegram.history_loading {
            return;
        }
        self.telegram.history_loading = true;
        self.telegram.history_failed = false;
        let task = cx.background_executor().spawn(async {
            let conn = moon_core::db::open_reader()?;
            moon_core::db::distinct_cores(&conn)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.telegram.history_loading = false;
                    match result {
                        Ok(cores) => {
                            this.telegram.history_cores = cores;
                            this.telegram.history_loaded = true;
                        }
                        Err(_) => this.telegram.history_failed = true,
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Open one chat without carrying another client's name, search, or transfer confirmation.
    fn edit_telegram_chat(
        &mut self,
        side: ChatsOf,
        chat: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_telegram_history(cx);
        let name = self
            .chats(side, cx)
            .and_then(|telegram| telegram.chat_access.iter().find(|a| a.chat_id == chat))
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let ed = self.chat_ed_mut(side);
        ed.active_chat = Some(chat);
        ed.pending_owner = None;
        let (name_state, search_state) = (ed.name.clone(), ed.search.clone());
        name_state.update(cx, |state, cx| state.set_value(name, window, cx));
        search_state.update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    /// Stack chat cards and expand a single editor so narrow Settings needs no sideways scrolling.
    ///
    /// Args:
    ///     side: Whose chats.
    ///     footer: What ends the section: the terminal's save hint, the station's Apply row.
    pub(in crate::settings) fn telegram_chat_access(
        &self,
        side: ChatsOf,
        footer: AnyElement,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let palette = MoonPalette::active(cx);
        let muted = rgba_from(palette.text_muted, 1.0);
        let section = MoonGroupBox::new(side.id("chat-access"))
            .title(t!("telegram.access_title").to_string())
            .padding(14.0)
            .gap(10.0)
            .child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.access_roles_hint").to_string()),
            );
        let telegram = match self.chats(side, cx) {
            Some(telegram) if !telegram.authorized_chat_ids.is_empty() => telegram,
            _ => {
                return section.child(
                    div()
                        .text_color(muted)
                        .child(t!("telegram.access_first_owner").to_string()),
                );
            }
        };
        let ed = self.chat_ed(side);
        let mut section = section;
        for &chat in &telegram.authorized_chat_ids {
            let owner = telegram.owner() == Some(chat);
            let profile = telegram.chat_access.iter().find(|a| a.chat_id == chat);
            let title = profile
                .map(|a| a.name.trim())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| t!("telegram.access_chat", id = chat).to_string());
            let summary = if owner {
                t!("telegram.access_owner_summary").to_string()
            } else {
                let count = match telegram.report_access(chat) {
                    Some(TelegramReportAccess::Viewer(ids)) => ids.len(),
                    _ => 0,
                };
                if count == 0 {
                    t!("telegram.access_waiting").to_string()
                } else {
                    t!("telegram.access_viewer_summary", count = count).to_string()
                }
            };
            let active = ed.active_chat == Some(chat);
            let mut card = v_flex()
                .w_full()
                .min_w_0()
                .gap(design::ui_px(cx, 8.0))
                .p(design::ui_px(cx, 10.0))
                .border_1()
                .border_color(rgba_from(palette.border, 1.0))
                .rounded(design::ui_px(cx, 6.0))
                .child(
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .gap(design::ui_px(cx, 8.0))
                        .items_center()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_color(rgba_from(palette.text, 1.0)).child(title))
                                .child(div().text_color(muted).child(summary)),
                        )
                        .child(
                            MoonButton::new(side.id(format_args!("edit-{chat}")))
                                .ghost()
                                .label(
                                    if active {
                                        t!("telegram.access_collapse")
                                    } else {
                                        t!("telegram.access_edit")
                                    }
                                    .to_string(),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.chat_ed(side).active_chat == Some(chat) {
                                        let ed = this.chat_ed_mut(side);
                                        ed.active_chat = None;
                                        ed.pending_owner = None;
                                        cx.notify();
                                    } else {
                                        this.edit_telegram_chat(side, chat, window, cx);
                                    }
                                }))
                                .render(),
                        ),
                );
            if active {
                card = card
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("telegram.access_chat", id = chat).to_string()),
                    )
                    .child(div().child(t!("telegram.access_name").to_string()))
                    .child(
                        MoonInput::new(side.id("chat-name"))
                            .state(&ed.name)
                            .placeholder(t!("telegram.access_name_placeholder").to_string())
                            .size(design::INPUT_SIZE),
                    );
                if !owner {
                    card = card.child(self.telegram_core_access(side, chat, cx));
                    let confirm = ed.pending_owner == Some(chat);
                    card = card
                        .when(confirm, |card| {
                            card.child(
                                div()
                                    .text_color(muted)
                                    .child(t!("telegram.access_transfer_hint").to_string()),
                            )
                        })
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap(design::ui_px(cx, 8.0))
                                .child(
                                    MoonButton::new(side.id(format_args!("owner-{chat}")))
                                        .ghost()
                                        .label(
                                            if confirm {
                                                t!("telegram.access_transfer_confirm")
                                            } else {
                                                t!("telegram.access_make_owner")
                                            }
                                            .to_string(),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.chat_ed(side).pending_owner == Some(chat) {
                                                this.chats_edit(side, cx, |telegram| {
                                                    telegram.set_owner(chat);
                                                    true
                                                });
                                                this.chat_ed_mut(side).pending_owner = None;
                                            } else {
                                                this.chat_ed_mut(side).pending_owner = Some(chat);
                                            }
                                            cx.notify();
                                        }))
                                        .render(),
                                )
                                .child(
                                    MoonButton::new(side.id(format_args!("revoke-{chat}")))
                                        .ghost()
                                        .label(t!("telegram.access_remove").to_string())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.chats_edit(side, cx, |telegram| {
                                                if telegram.owner() == Some(chat) {
                                                    return false;
                                                }
                                                // Freeze legacy ownership before removing an entry from the ordered pairing list.
                                                telegram.owner_chat_id = telegram.owner();
                                                telegram
                                                    .authorized_chat_ids
                                                    .retain(|id| *id != chat);
                                                telegram.chat_access.retain(|a| a.chat_id != chat);
                                                true
                                            });
                                            let ed = this.chat_ed_mut(side);
                                            ed.active_chat = None;
                                            ed.pending_owner = None;
                                            cx.notify();
                                        }))
                                        .render(),
                                ),
                        );
                }
            }
            section = section.child(card);
        }
        section.child(footer)
    }

    /// Viewer assignments use stable saved core IDs; selecting today's list never grants future cores.
    fn telegram_core_access(
        &self,
        side: ChatsOf,
        chat: i64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let backend = self.backend.read(cx);
        let cfg = backend.preview.as_ref().unwrap_or(&backend.config);
        let telegram = self.chats(side, cx);
        let selected = match telegram.and_then(|t| t.report_access(chat)) {
            Some(TelegramReportAccess::Viewer(ids)) => ids,
            _ => Vec::new(),
        };
        // The station serves the terminal's cores: the same uids, the same names.
        let mut cores: Vec<_> = cfg
            .servers
            .iter()
            .filter(|s| s.uid != 0)
            .map(|s| (s.uid, s.name.clone()))
            .collect();
        CoreOrder::new(cfg).sort_by(&mut cores, |(id, _)| *id);
        let current_ids: Vec<_> = cores.iter().map(|(id, _)| *id).collect();
        for (id, name) in &self.telegram.history_cores {
            if *id != 0 && !cores.iter().any(|(known, _)| known == id) {
                cores.push((
                    *id,
                    format!("{} ({})", name, t!("telegram.access_archived")),
                ));
            }
        }
        // Saved grants stay available during a failed or pending history read, even after unchecking.
        let drafted = telegram.map_or(&[][..], |t| t.chat_access.as_slice());
        for profile in self.chats_saved(side, cx).iter().chain(drafted) {
            for &id in &profile.core_uids {
                if id != 0
                    && id != moon_core::config::NO_MATCH_CORE_UID
                    && !cores.iter().any(|(known, _)| *known == id)
                {
                    cores.push((id, t!("telegram.access_archived").to_string()));
                }
            }
        }
        CoreOrder::new(cfg).sort_by(&mut cores, |(id, _)| *id);
        let ed = self.chat_ed(side);
        let query = ed.search.read(cx).value().trim().to_lowercase();
        let mut content = v_flex()
            .w_full()
            .min_w_0()
            .gap(design::ui_px(cx, 8.0))
            .child(div().child(t!("telegram.access_cores").to_string()))
            .child(
                MoonInput::new(side.id("core-search"))
                    .state(&ed.search)
                    .size(design::INPUT_SIZE)
                    .placeholder(t!("telegram.access_search").to_string()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        MoonButton::new(side.id("select-current"))
                            .ghost()
                            .label(t!("telegram.access_select_current").to_string())
                            .disabled(current_ids.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.chats_edit(side, cx, |telegram| {
                                    let ids = &mut telegram.chat_profile_mut(chat).core_uids;
                                    for id in &current_ids {
                                        if !ids.contains(id) {
                                            ids.push(*id);
                                        }
                                    }
                                    true
                                });
                            }))
                            .render(),
                    )
                    .child(
                        MoonButton::new(side.id("clear-cores"))
                            .ghost()
                            .label(t!("telegram.access_clear").to_string())
                            .disabled(selected.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.chats_edit(side, cx, |telegram| {
                                    telegram.chat_profile_mut(chat).core_uids.clear();
                                    true
                                });
                            }))
                            .render(),
                    ),
            );
        if self.telegram.history_loading {
            content = content.child(div().child(t!("telegram.access_history_loading").to_string()));
        }
        if self.telegram.history_failed {
            content = content
                .child(div().child(t!("telegram.access_history_failed").to_string()))
                .child(
                    MoonButton::new(side.id("history-retry"))
                        .ghost()
                        .label(t!("telegram.access_retry").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load_telegram_history(cx);
                            cx.notify();
                        }))
                        .render(),
                );
        }
        let mut count = 0;
        for (id, name) in cores {
            let label = format!("#{id} {name}");
            if !label.to_lowercase().contains(&query) {
                continue;
            }
            count += 1;
            content = content.child(
                MoonCheckbox::new(side.id(format_args!("core-{chat}-{id}")))
                    .checked(selected.contains(&id))
                    .label(label)
                    .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                        let checked = *checked;
                        this.chats_edit(side, cx, |telegram| {
                            let ids = &mut telegram.chat_profile_mut(chat).core_uids;
                            if checked {
                                if !ids.contains(&id) {
                                    ids.push(id);
                                }
                            } else {
                                ids.retain(|value| *value != id);
                            }
                            true
                        });
                    })),
            );
        }
        content.when(count == 0, |content| {
            content.child(
                div()
                    .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
                    .child(t!("telegram.access_no_matches").to_string()),
            )
        })
    }
}
