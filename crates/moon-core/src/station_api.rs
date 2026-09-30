//! The station's control API (`docs-internal/STATION.md` §4.5): what the terminal asks a running
//! station and what it answers, shared by both ends.
//!
//! Transport: a Unix socket in the station's runtime directory ([`SERVICE_SOCKET`]), reached from
//! the terminal through the helper's `ctl` over SSH — `moon-station ctl` relays one request from
//! its stdin and prints the reply. One exchange per connection: the station sends [`Hello`], the
//! client one [`Request`], the station one [`Reply`]; each frame is a big-endian `u32` length and
//! that many bytes of JSON.
//!
//! Only what the Settings tab needs so far: the station's and its bot's state, a pairing code on
//! demand, and the paired chats with their access, read and replaced. Nothing here deletes data.

use serde::{Deserialize, Serialize};

use crate::config::TelegramConfig;
use crate::config::telegram_access::TelegramChatAccess;
use crate::telegram::TelegramStatus;
use crate::telegram::runtime::mini_app::MiniAppStatus;

/// Bumped on any change a peer of the previous version would misread — every type here, and the
/// ones it carries (`TelegramStatus`, `MiniAppStatus`, `TelegramChatAccess`): they have no
/// fallback for a variant or field they do not know. The terminal refuses a station of another
/// version ([`CtlOutput::hello`]) before it reads the reply.
pub const PROTO_VERSION: u32 = 1;
/// The socket's name in the station's runtime directory.
pub const SOCKET_FILE: &str = "api.sock";
/// The socket of the service (`RuntimeDirectory=moon-station` in its unit).
pub const SERVICE_SOCKET: &str = "/run/moon-station/api.sock";
/// The largest frame either end accepts.
pub const MAX_FRAME: usize = 1 << 20;

/// The station's first frame on every connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub proto_version: u32,
    pub station_version: String,
}

/// What `moon-station ctl` prints: the station's hello and its reply, one JSON line — so the
/// terminal, not the station's own client, decides whether it speaks the station's version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtlOutput {
    pub hello: Hello,
    pub reply: Reply,
}

/// What a client asks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum Request {
    /// The station's cores and its bot.
    #[serde(rename = "status")]
    Status,
    /// A fresh ten-minute pairing code from the running bot, for one more chat.
    #[serde(rename = "pair.issue")]
    PairIssue,
    /// The paired chats, the owner and the viewers' grants.
    #[serde(rename = "access.get")]
    AccessGet,
    /// Replace the paired chats with `access`, only while they are still `base` — as the client
    /// read them: a chat paired on the station after that read is not dropped by an edit made
    /// before it.
    #[serde(rename = "access.set")]
    AccessSet { base: Access, access: Access },
}

/// What the station answers: the answer, or why there is none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Ok(Answer),
    Err(String),
}

impl From<Result<Answer, String>> for Reply {
    fn from(result: Result<Answer, String>) -> Self {
        match result {
            Ok(answer) => Self::Ok(answer),
            Err(reason) => Self::Err(reason),
        }
    }
}

/// An answer, one kind per request kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Status(Status),
    Pairing(PairingCode),
    Access(Access),
}

/// The station now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub station_version: String,
    pub cores_ready: usize,
    pub cores_total: usize,
    /// `None`: the station runs no bot — no token, or a pairing file it could not read.
    pub bot: Option<BotStatus>,
}

/// The station's bot now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotStatus {
    pub status: TelegramStatus,
    /// The Mini App is switched on (`[telegram] mini_app`); its state is `mini_app`.
    pub mini_app_on: bool,
    pub mini_app: MiniAppStatus,
    /// The code the bot accepts now: offered by the station itself while no chat is paired, or
    /// issued on request.
    pub pairing: Option<PairingCode>,
}

/// A code to send to the bot as `/pair <code>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingCode {
    pub code: String,
    /// How many seconds it is still accepted, counted when the answer was made.
    pub expires_in_s: u64,
}

/// Who may talk to the bot: the paired chats, the owner, the viewers' grants. Also the station's
/// own `telegram.json`.
///
/// Unknown fields are ignored, not refused: a station binary rolled back after a newer one wrote
/// the file must still start its bot. Nothing secret is here — the token is a credential.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    #[serde(default)]
    pub authorized_chat_ids: Vec<i64>,
    #[serde(default)]
    pub owner_chat_id: Option<i64>,
    #[serde(default)]
    pub chat_access: Vec<TelegramChatAccess>,
}

impl Access {
    /// The access part of a Telegram configuration.
    pub fn of(telegram: &TelegramConfig) -> Self {
        Self {
            authorized_chat_ids: telegram.authorized_chat_ids.clone(),
            owner_chat_id: telegram.owner_chat_id,
            chat_access: telegram.chat_access.clone(),
        }
    }

    /// Put this access into `telegram`, leaving its token and switches as they are.
    pub fn apply_to(&self, telegram: &mut TelegramConfig) {
        telegram.authorized_chat_ids = self.authorized_chat_ids.clone();
        telegram.owner_chat_id = self.owner_chat_id;
        telegram.chat_access = self.chat_access.clone();
    }

    /// Why a bot could not run with this access: a chat paired twice, an owner who is not a paired
    /// chat, a profile for a chat that is not paired.
    pub fn check(&self) -> Result<(), String> {
        for (i, chat) in self.authorized_chat_ids.iter().enumerate() {
            if self.authorized_chat_ids[..i].contains(chat) {
                return Err(format!("chat {chat} is paired twice"));
            }
        }
        if let Some(owner) = self.owner_chat_id {
            if !self.authorized_chat_ids.contains(&owner) {
                return Err(format!("the owner {owner} is not a paired chat"));
            }
        }
        if let Some(stray) = self
            .chat_access
            .iter()
            .find(|a| !self.authorized_chat_ids.contains(&a.chat_id))
        {
            return Err(format!(
                "chat {} has access but is not paired",
                stray.chat_id
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
