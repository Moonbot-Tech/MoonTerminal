//! Per-chat Telegram notification settings, the announce-once ledger, and the durable outbox.
//!
//! This module is the file shape only. Nothing here sends a message or reads a trade.
//! A missing file is a fresh all-off document. A present file that cannot be parsed is an error,
//! so a bad read never replaces a chat's settings with the defaults.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::db::QuoteCurrency;

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

/// Closed-trade card rule. Missing stored fields keep cards off.
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
    /// A card sent in a currency other than a USD stablecoin before its dollar value was known gets
    /// that value written into it once the valuation has it. Off by default. The Mini App does not know this
    /// switch, so it is not written while off and the Mini App's document stays as it was; its
    /// save keeps the stored value.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub usd_followup: bool,
    /// Thresholds in a trade's own currency, at most one row per currency. A trade in a listed
    /// currency is judged by its row in its own money instead of by the dollar fields above, and
    /// does not wait for a dollar valuation. Not written while empty, so a document from before
    /// the list stays as it was; the Mini App does not know it, and its save keeps the stored one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub by_quote: Vec<QuoteThresholds>,
}

impl TradeRule {
    /// The row that judges a trade in `quote`, if the rule lists that currency.
    pub fn thresholds_for(&self, quote: Option<QuoteCurrency>) -> Option<&QuoteThresholds> {
        find_quote(&self.by_quote, quote)
    }
}

/// Deal chart rule: a picture of a closed trade's tape, sent on its own, whatever the card rule
/// says (LinKvo 04.10: cards may be off while pictures are on). Off by default. The thresholds are
/// the card's, in dollars, and the picture waits for the trade's dollar value before it is
/// judged: a trade whose value never comes gets no picture — unless its currency has a row of its
/// own in [`Self::by_quote`], which judges it in that currency at once.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ChartRule {
    /// Send a picture for a closed trade that passes the thresholds.
    pub on: bool,
    /// Send when profit is at least this many USD. `None` applies no profit floor.
    pub profit_at_least_usd: Option<f64>,
    /// Send when the loss is at least this many USD (a positive magnitude). `None` applies no loss
    /// floor.
    pub loss_at_least_usd: Option<f64>,
    /// Profit and loss thresholds in a trade's own currency, as [`TradeRule::by_quote`]; a chart
    /// row carries no volume floor ([`NotifySettings::validate`] refuses one). Not written while
    /// empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub by_quote: Vec<QuoteThresholds>,
}

impl ChartRule {
    /// Whether the rule is in its default, all-off state, which is not written.
    pub fn is_off(&self) -> bool {
        *self == Self::default()
    }

    /// The row that judges a trade in `quote`, if the rule lists that currency.
    pub fn thresholds_for(&self, quote: Option<QuoteCurrency>) -> Option<&QuoteThresholds> {
        find_quote(&self.by_quote, quote)
    }
}

/// One currency's thresholds, in that currency's own money (LinKvo 13.10: a core trading in RUB
/// has no dollar valuation anywhere, so dollar thresholds could never judge its trades).
///
/// A row with no threshold set is a valid row: every trade in that currency passes, at once.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct QuoteThresholds {
    /// The currency's ticker as [`QuoteCurrency::ticker`] spells it. Text rather than the typed
    /// currency so a document naming a currency this build does not know still loads — one
    /// unparsable row must not cost the chat every other setting. Such a row matches no trade,
    /// and every save keeps it as stored: a newer build that wrote it still finds it, and a save
    /// from a surface that cannot show it (the Mini App, this build's editor) is not refused.
    pub quote: String,
    /// Minimum entry volume in this currency. `None` applies no volume floor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_volume: Option<f64>,
    /// Send when profit is at least this much of this currency. `None` applies no profit floor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profit_at_least: Option<f64>,
    /// Send when the loss is at least this much of this currency (a positive magnitude). `None`
    /// applies no loss floor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loss_at_least: Option<f64>,
}

impl QuoteThresholds {
    /// The currency this row names, or `None` for a ticker this build does not know.
    pub fn currency(&self) -> Option<QuoteCurrency> {
        QuoteCurrency::from_ticker(&self.quote)
    }
}

/// The row of `rows` naming `quote`. A trade of unknown currency has no row.
fn find_quote(rows: &[QuoteThresholds], quote: Option<QuoteCurrency>) -> Option<&QuoteThresholds> {
    let quote = quote?;
    rows.iter().find(|row| row.currency() == Some(quote))
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

/// One chat's notification rules. Missing stored switches default to off.
///
/// Unknown fields are ignored, not rejected: a document written before the daily summary was
/// removed still carries `"daily"`, and it must keep loading.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct NotifySettings {
    /// Closed-trade cards.
    pub trades: TradeRule,
    /// Core down and back notices.
    pub down: DownRule,
    /// Automatic reports. Missing JSON — a document from before them, or the Mini App's own save,
    /// which does not know them — is all off. Not written while all off, so the Mini App's
    /// document stays as it was.
    #[serde(skip_serializing_if = "AutoReports::is_off")]
    pub reports: AutoReports,
    /// What the chat hears of the cores' own Telegram reports. Off by default; not written while
    /// off, so the Mini App's document stays as it was.
    #[serde(skip_serializing_if = "EventRule::is_off")]
    pub events: EventRule,
    /// Deal charts. Not written while off, so the Mini App's document stays as it was; its save
    /// keeps the stored value.
    #[serde(skip_serializing_if = "ChartRule::is_off")]
    pub charts: ChartRule,
}

impl NotifySettings {
    /// Initial rules for a chat with no stored entry; serde defaults stay all-off for old chats.
    pub fn new_chat() -> Self {
        Self {
            trades: TradeRule {
                on: true,
                profit_at_least_usd: Some(100.0),
                loss_at_least_usd: Some(100.0),
                ..TradeRule::default()
            },
            down: DownRule {
                on: true,
                ..DownRule::default()
            },
            ..Self::default()
        }
    }
}

/// The cores' own Telegram reports a chat relays: what a strategy marks for Telegram
/// (`ReportToTelegram` on its detects, `ReportTradesToTelegram` on its trades), sent as the core
/// would send it. No filters of the chat's own; emulator trades included.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct EventRule {
    /// A trade was opened.
    pub opened: bool,
    /// A detect fired.
    pub detects: bool,
}

impl EventRule {
    /// Whether either is on.
    pub fn any(&self) -> bool {
        self.opened || self.detects
    }

    /// Whether both are off.
    pub fn is_off(&self) -> bool {
        !self.any()
    }
}

impl NotifySettings {
    /// Reject a document later batches must not persist.
    ///
    /// Returns the first broken rule. Set thresholds must be finite and `>= 0`.
    /// `after_minutes` is `1..=1440`. [`CoreScope::Only`] must list at least one core.
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
        reject_threshold(
            "charts.profit_at_least_usd",
            self.charts.profit_at_least_usd,
        )?;
        reject_threshold("charts.loss_at_least_usd", self.charts.loss_at_least_usd)?;
        reject_quote_rows("by_quote", &self.trades.by_quote)?;
        reject_quote_rows("charts.by_quote", &self.charts.by_quote)?;
        if let Some(row) = self.charts.by_quote.iter().find(|r| r.min_volume.is_some()) {
            return Err(NotifyError::Threshold {
                field: "charts.by_quote.min_volume",
                value: row.min_volume.unwrap_or(f64::NAN),
            });
        }
        if !(1..=1440).contains(&self.down.after_minutes) {
            return Err(NotifyError::AfterMinutes {
                value: self.down.after_minutes,
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
        /// `min_volume_usd`, `profit_at_least_usd`, `loss_at_least_usd`, one of the chart
        /// rule's two, prefixed `charts.`, or a `by_quote` row's (`by_quote.profit_at_least`,
        /// and `charts.by_quote.min_volume` for a volume floor a chart row may not carry).
        field: &'static str,
        /// The rejected number, which may be NaN.
        value: f64,
    },
    /// [`DownRule::after_minutes`] is outside `1..=1440`.
    AfterMinutes {
        /// The rejected minute count.
        value: u16,
    },
    /// [`CoreScope::Only`] listed no cores.
    EmptyCores,
    /// Two rows of a `by_quote` list name the same currency: which one judges would be a guess.
    DuplicateQuote {
        /// `by_quote` or `charts.by_quote`.
        list: &'static str,
        /// The later of the two rows.
        index: usize,
    },
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
            NotifyError::EmptyCores => formatter.write_str(
                "trade notifications need at least one core when the scope is an explicit list",
            ),
            NotifyError::DuplicateQuote { list, index } => write!(
                formatter,
                "{list}[{index}] names a currency an earlier row already has"
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

/// Reject a `by_quote` list with a currency listed twice — compared by its ticker, ignoring case,
/// so two rows of a currency this build does not know collide too — or a threshold
/// [`reject_threshold`] refuses. A row of unknown currency is valid ([`QuoteThresholds::quote`]).
fn reject_quote_rows(list: &'static str, rows: &[QuoteThresholds]) -> Result<(), NotifyError> {
    let mut seen = BTreeSet::new();
    for (index, row) in rows.iter().enumerate() {
        if !seen.insert(row.quote.trim().to_ascii_uppercase()) {
            return Err(NotifyError::DuplicateQuote { list, index });
        }
        reject_threshold("by_quote.min_volume", row.min_volume)?;
        reject_threshold("by_quote.profit_at_least", row.profit_at_least)?;
        reject_threshold("by_quote.loss_at_least", row.loss_at_least)?;
    }
    Ok(())
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
    /// explicit disclosure, and `Some([])` names no core. A missing
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
    /// The closed trade this card announces, when the chat waits for its dollar value
    /// ([`TradeRule::usd_followup`]): once Telegram accepts the card, its message id goes into
    /// the chat's [`NotifyLedger::cards`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<CardKey>,
    /// A message already in the chat that this row's HTML replaces (`editMessageText`) instead
    /// of a new message. A build that predates the field sends the row as a new message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<i64>,
    /// With [`Self::edit`]: a screen of the bot's menu redrawn in place — the message is edited as
    /// a rich message and its buttons are replaced with these. A build that predates the field
    /// edits the text alone, which drops the buttons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redraw: Option<crate::telegram::api::ReplyMarkup>,
    /// A deal chart: the name of its PNG in the charts folder beside the notifications file
    /// ([`crate::telegram::runtime::chart_spool_dir`]); [`Self::html`] is then its caption. The
    /// file goes with the row. A build that predates the field sends the caption alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<String>,
}

/// One closed trade: its core and report record id.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize,
)]
pub struct CardKey {
    pub core: u64,
    pub rec_id: i64,
}

/// A card waiting for its trade's dollar value ([`TradeRule::usd_followup`]).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct CardWait {
    /// UTC seconds the card was queued; a card waits a day at most.
    pub queued_utc: i64,
    /// The message Telegram accepted for the card. `None` until it is sent.
    pub message: Option<i64>,
    /// The card said its thresholds could not be checked; the corrected card keeps saying so.
    pub unchecked: bool,
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

/// Announce-once state for one chat. Restart reads this instead of replaying history.
///
/// Unknown fields are ignored, so a ledger an older build wrote with a since-removed field loads.
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
    /// Each automatic report's last slot and the message it left in the chat.
    pub reports: AutoLedger,
    /// Closes a threshold could not judge yet because their dollar value is unknown: core, record
    /// id, then the UTC seconds the trade was first held. Not in [`Self::seen`] until decided.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub held: BTreeMap<u64, BTreeMap<i64, i64>>,
    /// Cards sent before their trade's dollar value was known, waiting to have it written in:
    /// core, record id, then the card. Empty unless [`TradeRule::usd_followup`] is on.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cards: BTreeMap<u64, BTreeMap<i64, CardWait>>,
    /// The deal chart rule's own announce-once state, kept apart from the cards': either rule may
    /// be on without the other.
    #[serde(skip_serializing_if = "ChartLedger::is_empty")]
    pub charts: ChartLedger,
}

/// Drop closes older than `older_than_utc` from a rule's seen closes, then the cores left empty —
/// [`NotifyLedger::prune_seen`] for any rule's part of the ledger.
pub fn prune_closes(seen: &mut BTreeMap<u64, BTreeMap<i64, i64>>, older_than_utc: i64) {
    seen.retain(|_core, rows| {
        rows.retain(|_rec_id, close_utc| *close_utc >= older_than_utc);
        !rows.is_empty()
    });
}

/// [`ChartRule`]'s announce-once state, the shape of the card rule's part of [`NotifyLedger`].
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct ChartLedger {
    /// Unix seconds when the chart rule was switched on. Absent while it is off.
    pub enabled_utc: Option<i64>,
    /// Considered closes, including the ones the thresholds turned down: core, record id, then
    /// UTC seconds of the close.
    pub seen: BTreeMap<u64, BTreeMap<i64, i64>>,
    /// Closes waiting for their dollar value: core, record id, then the UTC seconds the trade was
    /// first held.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub held: BTreeMap<u64, BTreeMap<i64, i64>>,
}

impl ChartLedger {
    /// Whether nothing is recorded, which is not written.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
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
        prune_closes(&mut self.seen, older_than_utc);
    }

    /// Stop waiting for cards queued before `older_than_utc`, then drop cores with none left.
    pub fn prune_cards(&mut self, older_than_utc: i64) {
        self.cards.retain(|_core, rows| {
            rows.retain(|_rec_id, card| card.queued_utc >= older_than_utc);
            !rows.is_empty()
        });
    }

    /// Forget `key` as a card waiting for its dollar value.
    pub fn drop_card(&mut self, key: CardKey) {
        if let Some(rows) = self.cards.get_mut(&key.core) {
            rows.remove(&key.rec_id);
            if rows.is_empty() {
                self.cards.remove(&key.core);
            }
        }
    }
}

/// Settings plus ledger for one paired chat.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ChatNotify {
    /// This chat's chosen report mode; absent entries follow the bot's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_view: Option<crate::config::telegram_menu::ReportView>,
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
