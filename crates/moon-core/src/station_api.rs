//! The station's control API (`docs-internal/STATION.md` §4.5): what the terminal asks a running
//! station and what it answers, shared by both ends.
//!
//! Transport: a Unix socket in the station's runtime directory ([`SERVICE_SOCKET`]), reached from
//! the terminal through the helper's `ctl` over SSH — `moon-station ctl` relays one request from
//! its stdin and prints the reply. One exchange per connection: the station sends [`Hello`], the
//! client one [`Request`], the station one [`Reply`]; each frame is a big-endian `u32` length and
//! that many bytes of JSON.
//!
//! The Settings tab's share: the station's and its bot's state, a pairing code on demand, and the
//! paired chats with their access, read and replaced. The terminal's pull (§4.9): the tape around
//! closed trades and their order traces, what the station holds of what the terminal lacks.
//! Nothing here deletes data.

use serde::{Deserialize, Serialize};

use crate::config::TelegramConfig;
use crate::config::telegram_access::TelegramChatAccess;
use crate::config::telegram_menu::BotSettings;
use crate::feed::report_traces::{ArchivedLineKind, ArchivedOrderTrace};
use crate::telegram::TelegramStatus;
use crate::telegram::notify::NotifySettings;
use crate::telegram::runtime::mini_app::MiniAppStatus;
use std::collections::BTreeMap;

/// Bumped on any change a peer of the previous version would misread — every type here, and the
/// ones it carries (`TelegramStatus`, `MiniAppStatus`, `TelegramChatAccess`): they have no
/// fallback for a variant or field they do not know. The terminal refuses a station of another
/// version ([`CtlOutput::hello`]) before it reads the reply.
pub const PROTO_VERSION: u32 = 2;
/// The socket's name in the station's runtime directory.
pub const SOCKET_FILE: &str = "api.sock";
/// The socket of the service (`RuntimeDirectory=moon-station` in its unit).
pub const SERVICE_SOCKET: &str = "/run/moon-station/api.sock";
/// The largest frame either end accepts.
pub const MAX_FRAME: usize = 1 << 20;
/// How many bytes of prints one [`Tape`] answer carries at most, leaving the rest of a frame to
/// the JSON around them.
pub const TAPE_REPLY_BUDGET: usize = MAX_FRAME * 3 / 4;
/// Most markets one [`Request::TapeFetch`] names.
pub const MAX_TAPE_ITEMS: usize = 256;
/// Most spans one [`TapeWant`] names.
pub const MAX_TAPE_SPANS: usize = 64;
/// Most trades one [`Request::TracesFetch`] names.
pub const MAX_TRACE_UIDS: usize = 256;

/// The station's first frame on every connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub proto_version: u32,
    pub station_version: String,
}

/// What `moon-station ctl` prints: the station's hello and its reply, one JSON line — so the
/// terminal, not the station's own client, decides whether it speaks the station's version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CtlOutput {
    pub hello: Hello,
    pub reply: Reply,
}

/// What a client asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// before it. The bot's settings and zone ride along: an absent one keeps the station's, and
    /// a `base` that carries the bot's settings is refused when they moved since (see
    /// [`Access::base_holds`]).
    #[serde(rename = "access.set")]
    // Boxed: the bot's settings make an access the largest thing a request carries. The wire
    // form is the same.
    AccessSet {
        base: Box<Access>,
        access: Box<Access>,
    },
    /// What the station's recording holds inside these stretches of these markets — the tape of
    /// closed trades the terminal lacks. Answered from the file alone, off the main loop.
    #[serde(rename = "tape.fetch")]
    TapeFetch { items: Vec<TapeWant> },
    /// The order traces the station holds for these trades of one core; a trade it holds none
    /// for is left out.
    #[serde(rename = "traces.fetch")]
    TracesFetch {
        core_uid: u64,
        report_uids: Vec<i64>,
    },
}

/// What the station answers: the answer, or why there is none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Status(Status),
    Pairing(PairingCode),
    Access(Access),
    Tape(Tape),
    Traces(Traces),
}

/// Stretches of one market's tape a client lacks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeWant {
    /// The exchange key the tape is filed under (`"{code}:{dex:08x}"`).
    pub exchange: String,
    /// The exchange-native market name.
    pub market: String,
    /// Inclusive `(from_ms, to_ms)` stretches, true-UTC milliseconds, ascending and disjoint.
    pub spans: Vec<(i64, i64)>,
}

/// One covered stretch of a wanted market: every print the station recorded in it — none is a
/// quiet stretch, still covered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapePiece {
    /// Index of its [`TapeWant`] in the request.
    pub item: u32,
    pub from_ms: i64,
    pub to_ms: i64,
    /// The prints, packed as the tape file packs them, in base64
    /// (`market::trade_replay::trade_cache::encode_prints`).
    pub prints: String,
}

/// Where an answer that hit [`TAPE_REPLY_BUDGET`] stopped: ask again from `item`, its spans cut
/// to start at `from_ms`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeResume {
    pub item: u32,
    pub from_ms: i64,
}

/// The answer to [`Request::TapeFetch`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tape {
    /// Ascending by item, then by time. A wanted stretch not covered by any piece is one the
    /// station does not hold.
    pub pieces: Vec<TapePiece>,
    /// `None`: every wanted stretch was answered.
    pub resume: Option<TapeResume>,
}

/// The answer to [`Request::TracesFetch`]: the traces of the first `answered` trades asked, those
/// the station holds lines for. An answer that reached [`TAPE_REPLY_BUDGET`] answers fewer than
/// were asked; the client asks again for the rest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Traces {
    pub trades: Vec<TradeTraces>,
    /// How many of the asked `report_uids`, in order, this answer covers.
    pub answered: u32,
}

/// The order traces of one trade, as the station archived them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradeTraces {
    pub report_uid: i64,
    pub lines: Vec<TraceLine>,
}

/// One line of a trade's trace ([`ArchivedOrderTrace`] on the wire).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceLine {
    pub own: bool,
    /// The exit leg; `false` is the entry.
    pub exit: bool,
    pub stop_price: Option<f64>,
    pub stop_time_ms: Option<f64>,
    /// `(Unix UTC ms, price)`.
    pub points: Vec<(f64, f64)>,
}

impl From<&ArchivedOrderTrace> for TraceLine {
    fn from(trace: &ArchivedOrderTrace) -> Self {
        Self {
            own: trace.own,
            exit: trace.kind == ArchivedLineKind::Exit,
            stop_price: trace.stop_price,
            stop_time_ms: trace.stop_time_ms,
            points: trace.points.clone(),
        }
    }
}

impl From<TraceLine> for ArchivedOrderTrace {
    fn from(line: TraceLine) -> Self {
        Self {
            own: line.own,
            kind: if line.exit {
                ArchivedLineKind::Exit
            } else {
                ArchivedLineKind::Entry
            },
            stop_price: line.stop_price,
            stop_time_ms: line.stop_time_ms,
            points: line.points,
        }
    }
}

/// The station now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub station_version: String,
    pub cores_ready: usize,
    pub cores_total: usize,
    /// `None`: the station runs no bot — no token, or a pairing file it could not read.
    pub bot: Option<BotStatus>,
    /// The window around a trade the station records with now. `None` from a station older than
    /// it — an added field with a default, so neither end of version 2 misreads the other.
    #[serde(default)]
    pub tape: Option<TapeWindow>,
    /// The machine and the process under the station. `None` from a station older than it — an
    /// added field with a default, so neither end of version 2 misreads the other. Boxed: it is
    /// the bulk of the status, and every reply would otherwise carry its size.
    #[serde(default)]
    pub host: Option<Box<Host>>,
    /// How the last update ended, as the server's helper left it: `<UTC time> <verdict line>`.
    /// `None` before the first update, or from a station older than it — an added field with a
    /// default, so neither end of version 2 misreads the other.
    #[serde(default)]
    pub last_update: Option<String>,
    /// Whether the station updates itself from the release (`[update] auto`). `None` from a
    /// station older than the switch — an added field with a default, so neither end of version 2
    /// misreads the other.
    #[serde(default)]
    pub auto_update: Option<bool>,
}

/// The window around a trade the tape is recorded in (the terminal's `[trade_replay]`, the
/// station's `[tape]` in `station.toml`): the station's own, set from the terminal's Settings by
/// hand, never behind the user's back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeWindow {
    /// Seconds of prints on each side of a trade.
    pub margin_s: u32,
    /// From how many minutes a position is recorded as its two ends.
    pub long_position_min: u32,
}

/// The station's server and process: what it has run, what it uses, what it keeps on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// Seconds since the station process started.
    pub uptime_s: u64,
    /// Processor load over the last hour and the last day, each as far back as the process has
    /// run; empty before its first whole minute.
    pub cpu: Vec<CpuWindow>,
    /// `None` where the process could not read it.
    pub memory: Option<Memory>,
    /// The filesystem the data root is on; `None` where it could not be read.
    pub disk: Option<Disk>,
    /// Everything in the data root, largest first: a database with its `-wal`/`-shm`/`-journal`
    /// as one entry, a directory as the sum of what is in it.
    pub files: Vec<DataFile>,
}

/// Processor load over one window, in tenths of a percent of the whole machine (every core):
/// 1000 is the machine at full load. A peak is the busiest minute's average.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuWindow {
    /// How many minutes the window covers: 60 or 1440, or fewer while the process is younger.
    pub minutes: u32,
    pub station_avg_permille: u16,
    pub station_peak_permille: u16,
    pub machine_avg_permille: u16,
    pub machine_peak_permille: u16,
}

/// The station's memory and the machine's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    /// Resident memory of the station process now.
    pub rss_bytes: u64,
    /// The largest resident memory it has had since it started, sampled once a second.
    pub rss_peak_bytes: u64,
    /// What the machine can still hand out without swapping (`MemAvailable`).
    pub available_bytes: u64,
    pub total_bytes: u64,
}

/// Space on the filesystem the data root is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    /// Free to the station (what an unprivileged writer gets).
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// One entry of the data root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFile {
    /// The name in the data root: `reports.sqlite`, or a directory's name with a trailing `/`.
    pub name: String,
    pub bytes: u64,
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

/// Who may talk to the bot: the paired chats, the owner, the viewers' grants — and the bot's own
/// settings and the zone its reports are cut in. Also the station's own `telegram.json`.
///
/// Unknown fields are ignored, not refused: a station binary rolled back after a newer one wrote
/// the file must still start its bot. The bot's settings, the zone and the chats' notifications
/// are optional both ways: a station or terminal that predates them leaves them out, which keeps
/// the other side's — so [`PROTO_VERSION`] did not move for them. The notifications are never
/// part of the file. Nothing secret is here — the token is a credential.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Access {
    #[serde(default)]
    pub authorized_chat_ids: Vec<i64>,
    #[serde(default)]
    pub owner_chat_id: Option<i64>,
    #[serde(default)]
    pub chat_access: Vec<TelegramChatAccess>,
    /// The bot's menu and report settings. A station always answers with them; absent from a
    /// station that predates them, and in a change it means "keep the station's".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<BotSettings>,
    /// The IANA zone the station's reports are cut in: the terminal's header clock, pushed when it
    /// changes. Absent keeps the station's (and on disk, `station.toml`'s `[telegram] zone`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    /// Each chat's notifications with the revision they were read at: answered by the station
    /// from its notifications file; in a change, the chats whose settings to replace — each only
    /// while its stored revision is still the one given. Never written to `telegram.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<BTreeMap<i64, ChatNotifyRow>>,
}

/// One chat's notification settings and the revision of the stored row they come from (`0` for a
/// chat with none stored yet).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatNotifyRow {
    pub settings: NotifySettings,
    pub revision: u64,
}

impl Access {
    /// The chats and the bot's settings of a Telegram configuration; no zone.
    pub fn of(telegram: &TelegramConfig) -> Self {
        Self {
            authorized_chat_ids: telegram.authorized_chat_ids.clone(),
            owner_chat_id: telegram.owner_chat_id,
            chat_access: telegram.chat_access.clone(),
            bot: Some(telegram.bot.clone()),
            zone: None,
            notify: None,
        }
    }

    /// Put this access into `telegram`, leaving its token and switches as they are; the bot's
    /// settings only when they are here.
    pub fn apply_to(&self, telegram: &mut TelegramConfig) {
        telegram.authorized_chat_ids = self.authorized_chat_ids.clone();
        telegram.owner_chat_id = self.owner_chat_id;
        telegram.chat_access = self.chat_access.clone();
        if let Some(bot) = &self.bot {
            telegram.bot = bot.clone();
        }
    }

    /// Whether the chats are the same: who is paired, the owner, the grants.
    pub fn same_chats(&self, other: &Self) -> bool {
        self.authorized_chat_ids == other.authorized_chat_ids
            && self.owner_chat_id == other.owner_chat_id
            && self.chat_access == other.chat_access
    }

    /// Whether a change edited from `self` (as the client read it) still applies to `current`:
    /// the same chats, and — when the client read the bot's settings — the same settings. The zone
    /// is never compared: it is pushed on its own and an edit of the chats does not carry it. Nor
    /// are the notifications: each chat's row carries its own revision.
    pub fn base_holds(&self, current: &Self) -> bool {
        self.same_chats(current)
            && self
                .bot
                .as_ref()
                .is_none_or(|bot| current.bot.as_ref() == Some(bot))
    }

    /// Upgrade a saved pairing that predates the owner: its first chat becomes the explicit owner.
    /// Only for a pairing read from disk, never for one arriving over the control API.
    ///
    /// Returns:
    ///     `true` when the owner was set.
    pub fn adopt_legacy_owner(&mut self) -> bool {
        if self.owner_chat_id.is_some() {
            return false;
        }
        let Some(first) = self.authorized_chat_ids.first().copied() else {
            return false;
        };
        self.owner_chat_id = Some(first);
        true
    }

    /// Why a bot could not run with this access: a chat paired twice, paired chats with no owner,
    /// an owner who is not a paired chat, a profile for a chat that is not paired.
    pub fn check(&self) -> Result<(), String> {
        for (i, chat) in self.authorized_chat_ids.iter().enumerate() {
            if self.authorized_chat_ids[..i].contains(chat) {
                return Err(format!("chat {chat} is paired twice"));
            }
        }
        match self.owner_chat_id {
            Some(owner) if !self.authorized_chat_ids.contains(&owner) => {
                return Err(format!("the owner {owner} is not a paired chat"));
            }
            None if !self.authorized_chat_ids.is_empty() => {
                return Err("the paired chats have no owner".into());
            }
            _ => {}
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
