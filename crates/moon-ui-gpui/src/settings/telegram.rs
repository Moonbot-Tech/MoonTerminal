//! Telegram Settings tab: token, pairing status, Mini App.
//!
//! Edits stay on `Backend.preview` and persist through the existing Save transaction. Live
//! service status and pairing come from the live service, independently of unsaved edits.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonCheckboxSize, MoonGroupBox, MoonInput, MoonInputEvent, MoonInputState,
    MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{Backend, design};
use moon_core::config::Secret;

mod access;

/// Password-field width in unscaled pixels, matching the Security tab.
const TOKEN_FIELD_W: f32 = 240.0;

/// Per-window editor state for the Telegram tab.
pub(super) struct TelegramEd {
    token: Entity<MoonInputState>,
    /// One expanded chat keeps long client lists compact.
    active_chat: Option<i64>,
    /// Ownership transfer requires a second explicit click within the selected chat.
    pending_owner: Option<i64>,
    name: Entity<MoonInputState>,
    search: Entity<MoonInputState>,
    /// History candidates are independent of the draft grants so deselection is reversible.
    history_cores: Vec<(u64, String)>,
    history_loaded: bool,
    history_loading: bool,
    history_failed: bool,
}

/// Build masked token input bound to the Settings draft.
///
/// Args:
///     backend: Settings backend that owns the draft configuration.
///     window: Settings window used to create the input state.
///     cx: Settings context used to install the change subscription.
///
/// Returns:
///     Editor state whose token field writes `AppConfig::telegram.token` on each change.
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> TelegramEd {
    let initial = {
        let b = backend.read(cx);
        let d = b.preview.as_ref().unwrap_or(&b.config);
        d.telegram.token.expose().to_string()
    };
    let token = cx.new(|cx| {
        MoonInputState::new(window, cx)
            .masked(true)
            .default_value(initial)
    });
    token.update(cx, |st, c| st.set_masked(true, window, c));
    cx.subscribe(&token, |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut() {
                    if p.telegram.token.expose() != val {
                        p.telegram.token = Secret::new(val);
                        bcx.notify();
                    }
                }
            });
        }
    })
    .detach();
    let name = cx.new(|cx| MoonInputState::new(window, cx));
    cx.subscribe(&name, |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let Some(chat) = this.telegram.active_chat else {
                return;
            };
            let value = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(draft) = b.preview.as_mut()
                    && draft.telegram.authorized_chat_ids.contains(&chat)
                {
                    let profile = draft.telegram.chat_profile_mut(chat);
                    if profile.name != value {
                        profile.name = value;
                        bcx.notify();
                    }
                }
            });
        }
    })
    .detach();
    let search = cx.new(|cx| MoonInputState::new(window, cx));
    cx.subscribe(&search, |_, _, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            cx.notify();
        }
    })
    .detach();
    TelegramEd {
        token,
        active_chat: None,
        pending_owner: None,
        name,
        search,
        history_cores: Vec::new(),
        history_loaded: false,
        history_loading: false,
        history_failed: false,
    }
}

impl SettingsView {
    /// Render the Telegram Settings tab.
    ///
    /// Controls stack vertically so a 620-pixel Settings width does not need a horizontal
    /// scrollbar. Unsaved edits remain explicit while live transport health is shown
    /// independently from the draft.
    ///
    /// Args:
    ///     cx: Settings context used for palette, draft, and callbacks.
    ///
    /// Returns:
    ///     The assembled Telegram tab.
    pub(super) fn telegram_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let cfg = self.backend.read(cx);
        let draft = cfg.preview.as_ref().unwrap_or(&cfg.config);
        let telegram = &draft.telegram;
        let token_changed = telegram.token.expose() != cfg.config.telegram.token.expose();
        let status_text = if token_changed {
            t!("telegram.state.unsaved").to_string()
        } else {
            cfg.telegram_service_status_text()
        };
        let pairing = cfg
            .telegram
            .pairing
            .as_ref()
            .map(|(code, _)| t!("telegram.pair_code", code = code.clone()).to_string())
            .unwrap_or_default();
        let mini_changed =
            token_changed || telegram.mini_app_enabled != cfg.config.telegram.mini_app_enabled;
        let mini_status = if mini_changed {
            t!("telegram.state.unsaved").to_string()
        } else if !telegram.mini_app_enabled {
            t!("telegram.mini_disabled").to_string()
        } else if telegram.token.is_empty() {
            t!("telegram.state.disabled").to_string()
        } else {
            use moon_core::telegram::runtime::mini_app::MiniAppStatus;
            match &cfg.telegram.mini_status {
                MiniAppStatus::Tunneling { .. } => t!("telegram.mini_ready"),
                MiniAppStatus::Starting | MiniAppStatus::Listening { .. } => {
                    t!("telegram.mini_starting")
                }
                MiniAppStatus::Failed { .. } => t!("telegram.mini_failed"),
                MiniAppStatus::Stopped => t!("telegram.stopped"),
            }
            .to_string()
        };
        let mini_url = if telegram.mini_app_enabled && !mini_changed {
            match &cfg.telegram.mini_status {
                moon_core::telegram::runtime::mini_app::MiniAppStatus::Tunneling {
                    url, ..
                } => Some(url.clone()),
                _ => None,
            }
        } else {
            None
        };
        let paired = if telegram.authorized_chat_ids.is_empty() {
            t!("telegram.paired_none").to_string()
        } else {
            t!(
                "telegram.paired_count",
                count = telegram.authorized_chat_ids.len()
            )
            .to_string()
        };
        let mini_app = telegram.mini_app_enabled;

        v_flex()
            .w_full()
            .max_w(design::font_w_px(cx, 680.0))
            .gap(design::ui_px(cx, 16.0))
            .child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.intro").to_string()),
            )
            .child(
                MoonGroupBox::new("telegram-bot-section")
                    .title(t!("telegram.section_bot").to_string())
                    .padding(14.0)
                    .gap(10.0)
                    .child(div().text_color(rgba_from(p.text, 1.0)).child(status_text))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(design::ui_px(cx, 10.0))
                            .items_center()
                            .child(
                                div()
                                    .text_color(rgba_from(p.text_soft, 1.0))
                                    .child(t!("telegram.token").to_string()),
                            )
                            .child(
                                div().w(design::font_w_px(cx, TOKEN_FIELD_W)).child(
                                    MoonInput::new("telegram-token")
                                        .state(&self.telegram.token)
                                        .small()
                                        .mono(true)
                                        .mask_toggle(),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("telegram.token_hint").to_string()),
                    )
                    .when(cfg.config.telegram.token.expose().is_empty(), |token| {
                        token.child(
                            div()
                                .text_color(muted)
                                .child(t!("telegram.onboarding").to_string()),
                        )
                    }),
            )
            .child(
                MoonGroupBox::new("telegram-access-section")
                    .title(t!("telegram.section_access").to_string())
                    .padding(14.0)
                    .gap(10.0)
                    .child(div().text_color(muted).child(paired))
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("telegram.access_hint").to_string()),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(design::ui_px(cx, 8.0))
                            .child(
                                MoonButton::new("telegram-pair")
                                    .primary()
                                    .padding_x(12.0)
                                    .label(t!("telegram.pair_new").to_string())
                                    .disabled(cfg.telegram.service.is_none() || token_changed)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.backend.update(cx, |b, bcx| {
                                            b.issue_telegram_pairing();
                                            bcx.notify();
                                        });
                                    }))
                                    .render(),
                            )
                            .child(
                                MoonButton::new("telegram-reset")
                                    .ghost()
                                    .padding_x(12.0)
                                    .disabled(cfg.config.telegram.authorized_chat_ids.is_empty())
                                    .label(t!("telegram.pair_reset").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.backend.update(cx, |b, bcx| {
                                            b.reset_telegram_pairing();
                                            bcx.notify();
                                        });
                                    }))
                                    .render(),
                            ),
                    )
                    .when(!pairing.is_empty() && !token_changed, |section| {
                        section.child(
                            v_flex()
                                .gap(design::ui_px(cx, 8.0))
                                .child(div().font_family(design::mono()).child(pairing))
                                .child(
                                    MoonButton::new("telegram-copy-pair")
                                        .label(t!("telegram.pair_copy").to_string())
                                        .padding_x(12.0)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let command = this
                                                .backend
                                                .read(cx)
                                                .telegram
                                                .pairing
                                                .as_ref()
                                                .filter(|(_, expiry)| {
                                                    std::time::Instant::now() < *expiry
                                                })
                                                .map(|(code, _)| format!("/pair {code}"));
                                            if let Some(command) = command {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    command,
                                                ));
                                                this.status = Some((
                                                    super::StatusMsg::Key("settings.copied"),
                                                    false,
                                                ));
                                                cx.notify();
                                            }
                                        }))
                                        .render(),
                                ),
                        )
                    }),
            )
            .child(self.telegram_chat_access(cx))
            .child(
                MoonGroupBox::new("telegram-mini-section")
                    .title(t!("telegram.section_mini_app").to_string())
                    .padding(14.0)
                    .gap(10.0)
                    .child(
                        self.draft_checkbox(cx, "tg-mini-app", mini_app, |p, v| {
                            if p.telegram.mini_app_enabled != v {
                                p.telegram.mini_app_enabled = v;
                                true
                            } else {
                                false
                            }
                        })
                        .label(t!("telegram.mini_app").to_string())
                        .size(MoonCheckboxSize::Normal),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("telegram.mini_app_hint").to_string()),
                    )
                    .child(div().text_color(rgba_from(p.text, 1.0)).child(mini_status))
                    .when_some(mini_url, |tab, url| {
                        tab.child(
                            MoonButton::new("telegram-copy-url")
                                .label(t!("telegram.mini_copy_url").to_string())
                                .ghost()
                                .padding_x(12.0)
                                .tooltip(url.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                    this.status =
                                        Some((super::StatusMsg::Key("settings.copied"), false));
                                    cx.notify();
                                }))
                                .render(),
                        )
                    }),
            )
    }
}
