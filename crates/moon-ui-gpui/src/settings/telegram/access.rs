//! Per-chat role and core assignment editor using the shared Settings save transaction.
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_ui::{
    MoonButton, MoonCheckboxSize, MoonGroupBox, MoonInput, MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{core_order::CoreOrder, design};

impl SettingsView {
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
    fn edit_telegram_chat(&mut self, chat: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.load_telegram_history(cx);
        let name = {
            let backend = self.backend.read(cx);
            let cfg = backend.preview.as_ref().unwrap_or(&backend.config);
            cfg.telegram
                .chat_access
                .iter()
                .find(|a| a.chat_id == chat)
                .map(|a| a.name.clone())
                .unwrap_or_default()
        };
        self.telegram.active_chat = Some(chat);
        self.telegram.pending_owner = None;
        self.telegram
            .name
            .update(cx, |state, cx| state.set_value(name, window, cx));
        self.telegram
            .search
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    /// Stack chat cards and expand a single editor so narrow Settings needs no sideways scrolling.
    pub(in crate::settings) fn telegram_chat_access(&self, cx: &Context<Self>) -> impl IntoElement {
        let palette = MoonPalette::active(cx);
        let muted = rgba_from(palette.text_muted, 1.0);
        let backend = self.backend.read(cx);
        let cfg = backend.preview.as_ref().unwrap_or(&backend.config);
        let telegram = &cfg.telegram;
        let mut section = MoonGroupBox::new("telegram-chat-access")
            .title(t!("telegram.access_title").to_string())
            .padding(14.0)
            .gap(10.0)
            .child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.access_roles_hint").to_string()),
            );
        if telegram.authorized_chat_ids.is_empty() {
            return section.child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.access_first_owner").to_string()),
            );
        }
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
            let active = self.telegram.active_chat == Some(chat);
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
                            MoonButton::new(format!("tg-edit-{chat}"))
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
                                    if this.telegram.active_chat == Some(chat) {
                                        this.telegram.active_chat = None;
                                        this.telegram.pending_owner = None;
                                        cx.notify();
                                    } else {
                                        this.edit_telegram_chat(chat, window, cx);
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
                        MoonInput::new("tg-chat-name")
                            .state(&self.telegram.name)
                            .placeholder(t!("telegram.access_name_placeholder").to_string())
                            .small(),
                    );
                if !owner {
                    card = card.child(self.telegram_core_access(chat, cx));
                    let confirm = self.telegram.pending_owner == Some(chat);
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
                                    MoonButton::new(format!("tg-owner-{chat}"))
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
                                            if this.telegram.pending_owner == Some(chat) {
                                                this.backend.update(cx, |b, bcx| {
                                                    if let Some(cfg) = b.preview.as_mut() {
                                                        cfg.telegram.set_owner(chat);
                                                        bcx.notify();
                                                    }
                                                });
                                                this.telegram.pending_owner = None;
                                            } else {
                                                this.telegram.pending_owner = Some(chat);
                                            }
                                            cx.notify();
                                        }))
                                        .render(),
                                )
                                .child(
                                    MoonButton::new(format!("tg-revoke-{chat}"))
                                        .ghost()
                                        .label(t!("telegram.access_remove").to_string())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.backend.update(cx, |b, bcx| {
                                                if let Some(cfg) = b.preview.as_mut()
                                                    && cfg.telegram.owner() != Some(chat)
                                                {
                                                    // Freeze legacy ownership before removing an entry from the ordered pairing list.
                                                    cfg.telegram.owner_chat_id =
                                                        cfg.telegram.owner();
                                                    cfg.telegram
                                                        .authorized_chat_ids
                                                        .retain(|id| *id != chat);
                                                    cfg.telegram
                                                        .chat_access
                                                        .retain(|a| a.chat_id != chat);
                                                    bcx.notify();
                                                }
                                            });
                                            this.telegram.active_chat = None;
                                            this.telegram.pending_owner = None;
                                            cx.notify();
                                        }))
                                        .render(),
                                ),
                        );
                }
            }
            section = section.child(card);
        }
        section.child(
            div()
                .text_color(muted)
                .child(t!("telegram.access_save_hint").to_string()),
        )
    }

    /// Viewer assignments use stable saved core IDs; selecting today's list never grants future cores.
    fn telegram_core_access(&self, chat: i64, cx: &Context<Self>) -> impl IntoElement {
        let backend = self.backend.read(cx);
        let cfg = backend.preview.as_ref().unwrap_or(&backend.config);
        let selected = match cfg.telegram.report_access(chat) {
            Some(TelegramReportAccess::Viewer(ids)) => ids,
            _ => Vec::new(),
        };
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
        for profile in backend
            .config
            .telegram
            .chat_access
            .iter()
            .chain(cfg.telegram.chat_access.iter())
        {
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
        let query = self.telegram.search.read(cx).value().trim().to_lowercase();
        let mut content = v_flex()
            .w_full()
            .min_w_0()
            .gap(design::ui_px(cx, 8.0))
            .child(div().child(t!("telegram.access_cores").to_string()))
            .child(
                MoonInput::new("tg-core-search")
                    .state(&self.telegram.search)
                    .small()
                    .placeholder(t!("telegram.access_search").to_string()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        MoonButton::new("tg-select-current")
                            .ghost()
                            .label(t!("telegram.access_select_current").to_string())
                            .disabled(current_ids.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend.update(cx, |b, bcx| {
                                    if let Some(cfg) = b.preview.as_mut() {
                                        let ids =
                                            &mut cfg.telegram.chat_profile_mut(chat).core_uids;
                                        for id in &current_ids {
                                            if !ids.contains(id) {
                                                ids.push(*id);
                                            }
                                        }
                                        bcx.notify();
                                    }
                                });
                                cx.notify();
                            }))
                            .render(),
                    )
                    .child(
                        MoonButton::new("tg-clear-cores")
                            .ghost()
                            .label(t!("telegram.access_clear").to_string())
                            .disabled(selected.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend.update(cx, |b, bcx| {
                                    if let Some(cfg) = b.preview.as_mut() {
                                        cfg.telegram.chat_profile_mut(chat).core_uids.clear();
                                        bcx.notify();
                                    }
                                });
                                cx.notify();
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
                    MoonButton::new("tg-history-retry")
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
                self.draft_checkbox(
                    cx,
                    format!("tg-core-{chat}-{id}"),
                    selected.contains(&id),
                    move |cfg, checked| {
                        let ids = &mut cfg.telegram.chat_profile_mut(chat).core_uids;
                        if checked {
                            if !ids.contains(&id) {
                                ids.push(id);
                            }
                        } else {
                            ids.retain(|value| *value != id);
                        }
                        true
                    },
                )
                .label(label)
                .size(MoonCheckboxSize::Normal),
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
