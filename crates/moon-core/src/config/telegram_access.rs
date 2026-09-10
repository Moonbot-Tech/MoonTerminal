//! Saved Telegram roles and core grants, independent of transport and presentation.
use serde::{Deserialize, Serialize};

use super::TelegramConfig;

/// Desktop-assigned caption and explicit read-only core membership for a paired chat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TelegramChatAccess {
    /// Telegram chat identity, never a display name or phone number.
    pub chat_id: i64,
    /// Optional local caption to distinguish clients without exposing their identity elsewhere.
    #[serde(default)]
    pub name: String,
    /// Stable terminal-issued core uids; an empty list grants no data access.
    #[serde(default)]
    pub core_uids: Vec<u64>,
}

/// Owned permission snapshot used to reject reports if grants change during a database read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelegramReportAccess {
    /// The sole owner can read every core, including archived history.
    Owner,
    /// Observers can read only the listed cores and can never control them.
    Viewer(Vec<u64>),
}

impl TelegramConfig {
    /// Compare effective permissions, ignoring captions and equivalent ordering of viewer grants.
    /// A changed result requires cancellation of queued deliveries from the previous generation.
    pub fn same_chat_permissions(&self, other: &Self) -> bool {
        self.authorized_chat_ids == other.authorized_chat_ids
            && self
                .authorized_chat_ids
                .iter()
                .all(|chat| self.report_access(*chat) == other.report_access(*chat))
    }

    /// Resolve one owner while preserving the first pairing on upgrades from the flat chat list.
    pub fn owner(&self) -> Option<i64> {
        self.owner_chat_id
            .or_else(|| self.authorized_chat_ids.first().copied())
            .filter(|id| self.authorized_chat_ids.contains(id))
    }

    /// Pairing is mandatory even when a stale profile still exists in the encrypted config.
    pub fn report_access(&self, chat: i64) -> Option<TelegramReportAccess> {
        if !self.authorized_chat_ids.contains(&chat) {
            return None;
        }
        if self.owner() == Some(chat) {
            return Some(TelegramReportAccess::Owner);
        }
        let mut cores = self
            .chat_access
            .iter()
            .find(|a| a.chat_id == chat)
            .map(|a| a.core_uids.clone())
            .unwrap_or_default();
        cores.retain(|id| *id != 0 && *id != super::NO_MATCH_CORE_UID);
        cores.sort_unstable();
        cores.dedup();
        Some(TelegramReportAccess::Viewer(cores))
    }

    /// Obtain a draft profile without granting any core access implicitly.
    pub fn chat_profile_mut(&mut self, chat: i64) -> &mut TelegramChatAccess {
        let index = self
            .chat_access
            .iter()
            .position(|a| a.chat_id == chat)
            .unwrap_or_else(|| {
                self.chat_access.push(TelegramChatAccess {
                    chat_id: chat,
                    ..Default::default()
                });
                self.chat_access.len() - 1
            });
        &mut self.chat_access[index]
    }

    /// Transfer ownership to a paired chat; the former owner starts with no viewer grants.
    pub fn set_owner(&mut self, chat: i64) -> bool {
        if !self.authorized_chat_ids.contains(&chat) || self.owner() == Some(chat) {
            return false;
        }
        if let Some(previous) = self.owner() {
            self.chat_profile_mut(previous).core_uids.clear();
        }
        self.chat_profile_mut(chat).core_uids.clear();
        self.owner_chat_id = Some(chat);
        true
    }
}

#[cfg(test)]
mod tests;
