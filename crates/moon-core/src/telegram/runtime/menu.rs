//! Reconcile native chat menus without putting blocking Bot API calls on the Mini App relay.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::mini_app::MiniAppStatus;
use crate::telegram::api::{ApiError, MenuButton, WebAppInfo};

/// Wait after a failed write before the same chat is tried again.
const RETRY: Duration = Duration::from_secs(30);
/// Wait before a paired chat the bot cannot reach (blocked, deleted) is tried again: it changes
/// only when its user acts, so a slow retry costs nothing.
const UNREACHABLE_RETRY: Duration = Duration::from_secs(600);

/// Latest intent replaces older tunnel URLs even while the bot is long polling.
pub(super) type SharedMenu = Arc<Mutex<MenuIntent>>;

/// Non-secret desired menu state published by the Mini App owner.
#[derive(Clone)]
pub(super) struct MenuIntent {
    chats: Vec<i64>,
    button: MenuButton,
}

impl MenuIntent {
    /// A stopped or disabled app must override any previous per-chat web-app link.
    pub(super) fn stopped(chats: &[i64]) -> Self {
        Self {
            chats: chats.to_vec(),
            button: MenuButton::Commands,
        }
    }

    /// Publish only a live tunnel and a localized label to paired private chat identities.
    pub(super) fn publish(
        shared: &SharedMenu,
        chats: &[i64],
        enabled: bool,
        status: &MiniAppStatus,
        labels: &BTreeMap<String, String>,
    ) {
        let mut intent = Self::stopped(chats);
        if enabled {
            if let (MiniAppStatus::Tunneling { url, .. }, Some(text)) =
                (status, labels.get("menu_miniapp"))
            {
                intent.button = MenuButton::WebApp {
                    text: text.clone(),
                    web_app: WebAppInfo { url: url.clone() },
                };
            }
        }
        if let Ok(mut current) = shared.lock() {
            *current = intent;
        }
    }
}

/// Acknowledged remote state and a per-chat cooldown for failed writes.
#[derive(Default)]
struct ChatMenu {
    applied: Option<MenuButton>,
    retry_at: Option<Instant>,
    /// The button an unreachable chat refused: its long cooldown holds only while the wanted
    /// button stays the same — a new tunnel address is tried at once.
    refused: Option<MenuButton>,
}

/// Retain removed chats until their old link has been cleared successfully.
#[derive(Default)]
pub(super) struct MenuSync {
    chats: BTreeMap<i64, ChatMenu>,
}

impl MenuSync {
    /// Carry cleanup targets across service restarts without carrying their authorization.
    pub(super) fn with_cleanup(chats: &[i64]) -> Self {
        Self {
            chats: chats
                .iter()
                .filter(|&&chat| chat > 0)
                .map(|&chat| (chat, ChatMenu::default()))
                .collect(),
        }
    }

    /// Apply changed intent, remembering success only after Telegram acknowledges the write.
    pub(super) fn sync(
        &mut self,
        intent: &MenuIntent,
        now: Instant,
        mut send: impl FnMut(i64, &MenuButton) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        for &chat in intent.chats.iter().filter(|&&chat| chat > 0) {
            self.chats.entry(chat).or_default();
        }
        let mut first_error = None;
        for (&chat, state) in &mut self.chats {
            let desired = if intent.chats.contains(&chat) {
                &intent.button
            } else {
                &MenuButton::Commands
            };
            let cooling = state.retry_at.is_some_and(|at| now < at)
                && state
                    .refused
                    .as_ref()
                    .is_none_or(|refused| refused == desired);
            if state.applied.as_ref() == Some(desired) || cooling {
                continue;
            }
            match send(chat, desired) {
                Ok(()) => {
                    state.applied = Some(desired.clone());
                    state.retry_at = None;
                    state.refused = None;
                }
                Err(error) if crate::telegram::api::is_unreachable_chat(&error) => {
                    // Nobody can see this chat's menu while the bot cannot reach it: a removed
                    // chat is done with, a paired one waits for its user. Neither is bot health.
                    log::info!("telegram menu of an unreachable chat not updated: {error}");
                    if intent.chats.contains(&chat) {
                        state.applied = None;
                        state.retry_at = Some(Instant::now() + UNREACHABLE_RETRY);
                        state.refused = Some(desired.clone());
                    } else {
                        state.applied = Some(MenuButton::Commands);
                        state.retry_at = None;
                        state.refused = None;
                    }
                }
                Err(error) => {
                    // A request may have reached Telegram even if its response was lost.
                    state.applied = None;
                    state.retry_at = Some(Instant::now() + RETRY);
                    state.refused = None;
                    first_error.get_or_insert(error);
                }
            }
        }
        self.chats.retain(|chat, state| {
            intent.chats.contains(chat) || state.applied != Some(MenuButton::Commands)
        });
        first_error.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests;
