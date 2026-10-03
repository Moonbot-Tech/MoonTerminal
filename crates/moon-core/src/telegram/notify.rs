//! Per-chat Telegram notification settings, the announce-once ledger, and the durable outbox.
//!
//! This module is the file shape only. Nothing here sends a message or reads a trade.
//! A missing file is a fresh all-off document. A present file that cannot be parsed is an error,
//! so a bad read never replaces a chat's settings with the defaults.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Context;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Which cores a trade notification applies to.
///
/// `All` is every core the chat is allowed to see. Decisions use that grant,
/// and the sender rechecks each queued row's disclosure before sending.
/// `Only` is an explicit list and must be non-empty.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "ids", rename_all = "snake_case")]
pub enum CoreScope {
    /// Every core visible to the chat.
    #[default]
    All,
    /// Explicit core ids. An empty list fails [`NotifySettings::validate`].
    Only(Vec<u64>),
}

/// Closed-trade card rule. `on` defaults to off, so a new chat announces nothing.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct TradeRule {
    /// Send a card for a closed trade when the other fields match.
    pub on: bool,
    /// Cores this rule applies to. Missing JSON is [`CoreScope::All`].
    pub cores: CoreScope,
    /// Minimum trade volume in USD. `None` applies no volume floor.
    pub min_volume_usd: Option<f64>,
    /// Send when profit is at least this many USD. `None` applies no profit floor.
    pub profit_at_least_usd: Option<f64>,
    /// Send when the loss is at least this many USD (a positive magnitude). `None` applies no loss floor.
    pub loss_at_least_usd: Option<f64>,
}

/// Minutes the core must stay disconnected before a down notice. Default delay is 5.
const fn default_after_minutes() -> u16 {
    5
}

/// Core down/back notice. `on` defaults to off.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct DownRule {
    /// Send one down notice and one back notice.
    pub on: bool,
    /// Disconnect time before the down notice, from 1 to 1440 minutes.
    #[serde(default = "default_after_minutes")]
    pub after_minutes: u16,
}

impl Default for DownRule {
    /// Keep outage notices off and use a five-minute delay when settings are absent.
    fn default() -> Self {
        Self {
            on: false,
            after_minutes: default_after_minutes(),
        }
    }
}

/// Hour of the daily summary. Default is 21:00.
const fn default_daily_hour() -> u8 {
    21
}

/// Minute of the daily summary. Default is 21:00.
const fn default_daily_minute() -> u8 {
    0
}

/// Once-a-day summary. `on` defaults to off, and the clock defaults to 21:00.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct DailyRule {
    /// Queue the summary for the calendar day in the host's report zone.
    pub on: bool,
    /// Hour in `0..24`.
    #[serde(default = "default_daily_hour")]
    pub hour: u8,
    /// Minute in `0..60`.
    #[serde(default = "default_daily_minute")]
    pub minute: u8,
}

impl Default for DailyRule {
    /// Keep summaries off and use 21:00 in the host's report zone when settings are absent.
    fn default() -> Self {
        Self {
            on: false,
            hour: default_daily_hour(),
            minute: default_daily_minute(),
        }
    }
}

/// One automatic report: a report the bot sends on its own at a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoReport {
    /// At every hh:00, the hour that just ended.
    Hourly,
    /// At every hh:00, today so far; at 00:00 the day that just ended.
    Today,
    /// At 00:00, the month up to the day that just ended; on the 1st the month that just ended.
    Month,
}

impl AutoReport {
    /// Every automatic report, in the order screens list them.
    pub const ALL: [Self; 3] = [Self::Hourly, Self::Today, Self::Month];

    /// Whether a new report of this kind takes the previous one's place in the chat. The running
    /// totals (today, month) do; hourly reports stay in the chat's history (LinKvo 03.10).
    pub fn replaces_previous(self) -> bool {
        !matches!(self, Self::Hourly)
    }
}

/// Which automatic reports a chat receives. All off by default; a report opens in the bot's own
/// view and counts on the bot's own basis.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct AutoReports {
    pub hourly: bool,
    pub today: bool,
    pub month: bool,
}

impl AutoReports {
    /// Whether `kind` is switched on.
    pub fn on(&self, kind: AutoReport) -> bool {
        match kind {
            AutoReport::Hourly => self.hourly,
            AutoReport::Today => self.today,
            AutoReport::Month => self.month,
        }
    }

    /// Switch `kind` on or off.
    pub fn set(&mut self, kind: AutoReport, on: bool) {
        match kind {
            AutoReport::Hourly => self.hourly = on,
            AutoReport::Today => self.today = on,
            AutoReport::Month => self.month = on,
        }
    }

    /// Whether any automatic report is on.
    pub fn any(&self) -> bool {
        AutoReport::ALL.into_iter().any(|kind| self.on(kind))
    }

    /// Whether every automatic report is off.
    pub fn is_off(&self) -> bool {
        !self.any()
    }
}

/// One chat's notification rules. Every switch defaults to off.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct NotifySettings {
    /// Closed-trade cards.
    pub trades: TradeRule,
    /// Core down and back notices.
    pub down: DownRule,
    /// Daily summary.
    pub daily: DailyRule,
    /// Automatic reports. Missing JSON — a document from before them, or the Mini App's own save,
    /// which does not know them — is all off. Not written while all off, so the Mini App's
    /// document stays as it was.
    #[serde(skip_serializing_if = "AutoReports::is_off")]
    pub reports: AutoReports,
}

impl NotifySettings {
    /// Reject a document later batches must not persist.
    ///
    /// Returns the first broken rule. Set thresholds must be finite and `>= 0`.
    /// `after_minutes` is `1..=1440`. `hour` is `0..24` and `minute` is `0..60`.
    /// [`CoreScope::Only`] must list at least one core.
    ///
    /// # Errors
    ///
    /// [`NotifyError`] names the broken rule.
    pub fn validate(&self) -> Result<(), NotifyError> {
        if matches!(&self.trades.cores, CoreScope::Only(ids) if ids.is_empty()) {
            return Err(NotifyError::EmptyCores);
        }
        reject_threshold("min_volume_usd", self.trades.min_volume_usd)?;
        reject_threshold("profit_at_least_usd", self.trades.profit_at_least_usd)?;
        reject_threshold("loss_at_least_usd", self.trades.loss_at_least_usd)?;
        if !(1..=1440).contains(&self.down.after_minutes) {
            return Err(NotifyError::AfterMinutes {
                value: self.down.after_minutes,
            });
        }
        if self.daily.hour >= 24 {
            return Err(NotifyError::Hour {
                value: self.daily.hour,
            });
        }
        if self.daily.minute >= 60 {
            return Err(NotifyError::Minute {
                value: self.daily.minute,
            });
        }
        Ok(())
    }
}

/// One validation rule for [`NotifySettings::validate`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NotifyError {
    /// A volume, profit, or loss threshold is NaN, infinite, or negative.
    Threshold {
        /// `min_volume_usd`, `profit_at_least_usd`, or `loss_at_least_usd`.
        field: &'static str,
        /// The rejected number, which may be NaN.
        value: f64,
    },
    /// [`DownRule::after_minutes`] is outside `1..=1440`.
    AfterMinutes {
        /// The rejected minute count.
        value: u16,
    },
    /// [`DailyRule::hour`] is not in `0..24`.
    Hour {
        /// The rejected hour.
        value: u8,
    },
    /// [`DailyRule::minute`] is not in `0..60`.
    Minute {
        /// The rejected minute.
        value: u8,
    },
    /// [`CoreScope::Only`] listed no cores.
    EmptyCores,
}

impl std::fmt::Display for NotifyError {
    /// Write the rejected rule and value as a diagnostic, propagating formatter errors.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifyError::Threshold { field, value } => write!(
                formatter,
                "notification threshold {field} must be a finite number >= 0, got {value}"
            ),
            NotifyError::AfterMinutes { value } => write!(
                formatter,
                "core-down delay must be from 1 to 1440 minutes, got {value}"
            ),
            NotifyError::Hour { value } => {
                write!(
                    formatter,
                    "daily summary hour must be from 0 to 23, got {value}"
                )
            }
            NotifyError::Minute { value } => write!(
                formatter,
                "daily summary minute must be from 0 to 59, got {value}"
            ),
            NotifyError::EmptyCores => formatter.write_str(
                "trade notifications need at least one core when the scope is an explicit list",
            ),
        }
    }
}

impl std::error::Error for NotifyError {}

/// Reject a set threshold that is not a finite number `>= 0`. `None` is the filter being off.
fn reject_threshold(field: &'static str, value: Option<f64>) -> Result<(), NotifyError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(NotifyError::Threshold { field, value })
    }
}

/// What a queued automatic report needs beyond its HTML: it goes as a rich message with the
/// report's own buttons, and it replaces the chat's previous report of its kind.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AutoRow {
    /// Which automatic report this is.
    pub kind: AutoReport,
    /// The report's inline buttons: paging and views over its frozen period.
    pub keyboard: crate::telegram::api::ReplyMarkup,
}

/// One HTML message accepted into the file and not yet confirmed by Telegram.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct Pending {
    /// Id unique inside this file. Callers allocate it from [`NotifyFile::next_id`].
    pub id: u64,
    /// Destination chat.
    pub chat: i64,
    /// Telegram HTML body.
    pub html: String,
    /// Unix seconds when the entry was queued.
    pub created_utc: i64,
    /// Cores this message discloses.
    ///
    /// `None` is a row stored before cores were recorded, or an owner's automatic report, which
    /// covers every core: either is kept for an owner and dropped for a viewer. `Some` is an
    /// explicit disclosure, and `Some([])` names no core (an empty-day summary). A missing
    /// JSON field loads as `None`. A JSON array, including `[]`, loads as `Some`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cores_opt::deserialize"
    )]
    pub cores: Option<Vec<u64>>,
    /// An automatic report: sent as a rich message with its buttons. `None` is an ordinary HTML
    /// notification. A value this build cannot read loads as `None`, so a file written by a newer
    /// build still opens; such a row then goes as plain HTML, which Telegram refuses for the rich
    /// tags with a `Bad Request` the sender drops it on.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_auto::deserialize"
    )]
    pub auto: Option<AutoRow>,
}

/// [`Pending::auto`] read through a JSON value, so an unknown kind drops the field, not the file.
mod lenient_auto {
    use serde::{Deserialize, Deserializer};

    /// Read the field as `Some` when it parses as an [`super::AutoRow`], otherwise `None`.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<super::AutoRow>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<serde_json::Value>::deserialize(deserializer)?;
        Ok(value.and_then(|value| serde_json::from_value(value).ok()))
    }
}

/// JSON array form of [`Pending::cores`]. A missing field stays `None` via `#[serde(default)]`.
mod cores_opt {
    use serde::{Deserialize, Deserializer};

    /// Read an explicit core array as `Some`; reject null and non-array values.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u64>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<u64>::deserialize(deserializer).map(Some)
    }
}

/// `YYYY-MM-DD` form of [`NaiveDate`]. Chrono's `serde` feature is not enabled on this crate.
mod naive_date_opt {
    use chrono::NaiveDate;
    use serde::{Deserialize, Deserializer, Serializer};

    const FORMAT: &str = "%Y-%m-%d";

    /// Serialize a present date as `YYYY-MM-DD`, or an absent date as null.
    pub fn serialize<S>(value: &Option<NaiveDate>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(date) => serializer.serialize_some(&date.format(FORMAT).to_string()),
            None => serializer.serialize_none(),
        }
    }

    /// Read null or a `YYYY-MM-DD` string, returning a serde error for invalid dates.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<NaiveDate>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = Option::<String>::deserialize(deserializer)?;
        match text {
            None => Ok(None),
            Some(text) => NaiveDate::parse_from_str(&text, FORMAT)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

/// Announce-once state for one chat. Restart reads this instead of replaying history.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct NotifyLedger {
    /// Unix seconds when the trade rule was switched on. Absent until a caller records it.
    pub trades_enabled_utc: Option<i64>,
    /// Considered closes, including filter-suppressed rows: core, record id, then UTC seconds.
    ///
    /// JSON object keys are decimal strings. A tuple key is not used because it is not a JSON object key.
    pub seen: BTreeMap<u64, BTreeMap<i64, i64>>,
    /// Cores whose down notice was queued and whose back notice has not been queued.
    pub down_announced: BTreeSet<u64>,
    /// Host-local date last queued or suppressed when daily settings were enabled or moved.
    #[serde(default, with = "naive_date_opt")]
    pub daily_last: Option<NaiveDate>,
    /// Each automatic report's last slot and the message it left in the chat.
    pub reports: AutoLedger,
}

/// One automatic report's state in a chat.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct AutoSlot {
    /// UTC seconds of the last slot this report was queued or suppressed for. A slot after it is due.
    pub slot_utc: Option<i64>,
    /// The message Telegram accepted for this report last, which the next one deletes. Recorded
    /// only for a report that replaces its previous one ([`AutoReport::replaces_previous`]).
    pub message: Option<i64>,
}

/// [`AutoSlot`] of each automatic report. Named fields, so a kind a newer build adds is ignored.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct AutoLedger {
    pub hourly: AutoSlot,
    pub today: AutoSlot,
    pub month: AutoSlot,
}

impl AutoLedger {
    /// `kind`'s state.
    pub fn slot(&self, kind: AutoReport) -> &AutoSlot {
        match kind {
            AutoReport::Hourly => &self.hourly,
            AutoReport::Today => &self.today,
            AutoReport::Month => &self.month,
        }
    }

    /// `kind`'s state, to edit.
    pub fn slot_mut(&mut self, kind: AutoReport) -> &mut AutoSlot {
        match kind {
            AutoReport::Hourly => &mut self.hourly,
            AutoReport::Today => &mut self.today,
            AutoReport::Month => &mut self.month,
        }
    }
}

impl NotifyLedger {
    /// Drop closes older than `older_than_utc`, then drop cores that have no rows left.
    ///
    /// A close time equal to the bound stays. The caller picks the bound (the enable time or the
    /// window edge); this method does not know the window length.
    pub fn prune_seen(&mut self, older_than_utc: i64) {
        self.seen.retain(|_core, rows| {
            rows.retain(|_rec_id, close_utc| *close_utc >= older_than_utc);
            !rows.is_empty()
        });
    }
}

/// Settings plus ledger for one paired chat.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ChatNotify {
    /// What this chat asked to be told.
    pub settings: NotifySettings,
    /// Decisions already recorded, independently of Telegram delivery acknowledgements.
    pub ledger: NotifyLedger,
    /// Caller-owned counter stored with the chat. This module does not increment it.
    pub revision: u64,
}

/// On-disk notification document for every chat of one bot host.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct NotifyFile {
    /// Chat id to that chat's settings and ledger.
    pub chats: BTreeMap<i64, ChatNotify>,
    /// Messages accepted into the file and not yet confirmed sent.
    pub outbox: Vec<Pending>,
    /// Next [`Pending::id`]. Callers allocate it; load and save only keep the number.
    pub next_id: u64,
}

impl NotifyFile {
    /// Read `path`. A missing file is [`NotifyFile::default`] and is not created.
    ///
    /// An unreadable path or a body that is not this document is an error. Those cases never
    /// come back as the all-off default, so a crash cannot wipe stored settings.
    ///
    /// # Errors
    ///
    /// I/O failures other than not-found, and JSON that does not match [`NotifyFile`].
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    /// Replace `path` with pretty JSON of this document.
    ///
    /// The write goes through [`crate::config::write_file_atomic`], so a crash leaves the
    /// previous file or the new one.
    ///
    /// # Errors
    ///
    /// Serialization or the atomic write failed. `path` is left unchanged when the write fails
    /// before the rename.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        crate::config::write_file_atomic(path, text.as_bytes(), "telegram notifications")
    }
}

#[cfg(test)]
mod tests;
