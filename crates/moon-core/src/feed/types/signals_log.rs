//! Core notifications, detects, log lines and chart overlays.

/// Something a core's strategy asked to report to Telegram, for the bot to relay: what the core
/// would send to its own Telegram, which does not come over the wire.
///
/// Only flagged events are produced — a regular detect of a strategy with `ReportToTelegram`, the
/// entry of a trade whose strategy has `ReportTradesToTelegram` — so the stream stays small even
/// on a core whose detects storm.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreTgEvent {
    /// A regular detect.
    Detect {
        market: String,
        /// The detect's own line, bounded like [`DetectRow::msg`], addresses redacted.
        msg: String,
        /// The strategy's name, bounded like [`DetectRow::strat_name`].
        strat_name: String,
        is_short: bool,
    },
    /// A trade's entry: the report row that first carried its coin and entry stamp.
    Opened {
        rec_id: i64,
        coin: String,
        strat_name: String,
        /// The core's emulator traded it, not the exchange.
        emulator: bool,
        /// The entry stamp as the core wrote it, core-local.
        buy: crate::db::ReportStamp,
    },
}

/// One core detect for the toolbar and history, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct DetectRow {
    /// Monotonic per-core sequence number used as the ingestion cursor for the detects feed.
    pub seq: u64,
    /// Market or coin.
    pub market: String,
    /// Receipt time in Unix milliseconds.
    pub time_ms: f64,
    /// Whether the source strategy enables a sound alert through `SoundAlert=Yes`. The UI drops a
    /// detect when both this and `is_alert` are false; drawn-object alerts may therefore render
    /// without `sound_alert`. Detects auto-added to charts pass the same gate, and reach the
    /// regular buttons only where the feed's `show_add_to_chart` setting asks for them.
    pub sound_alert: bool,
    /// Number of seconds to keep the button, from strategy `KeepAlert`, defaulting to 60.
    pub keep_alert_secs: u32,
    /// Strategy `AddToChart` tab number, such as 1, 2, or 3, to which the coin chart is added
    /// automatically. `0` only disables automatic addition; a regular button still requires
    /// `sound_alert` or `is_alert`.
    pub add_to_chart: u32,
    /// Strategy `KeepInChart` duration in seconds before closing the automatically added coin
    /// chart while retaining the tab.
    ///
    /// **Zero means keep it indefinitely**, as it does in Moonbot: the chart's TTL is then infinite
    /// and only the user, or a tab's chart cap, closes it. Sixty is the fallback used when neither
    /// the strategy nor its schema says anything. Read the value through
    /// [`DetectRow::keep_in_chart_ttl_ms`] rather than multiplying this field by 1000.
    pub keep_in_chart_secs: u32,
    /// Strategy sound name as a WAV stem to play when the detect arrives; `None` is silent.
    pub sound_name: Option<String>,
    /// Whether this detect is a drawn-object alert trigger, `DETECT_KIND_ALERT`. These are shown and
    /// played even without a strategy, using the default sound when the strategy has none.
    pub is_alert: bool,
    /// Source strategy-kind ordinal from `StrategyKind`; see `strat_kind_name`. `0` means Unknown
    /// or a missing strategy snapshot, while an alert trigger without a strategy maps to 22
    /// (Alerts). Used for the detect-kind badge in the feed.
    pub kind: u8,
    /// Source strategy direction from `is_short`, where `true` means short. Used to outline the
    /// feed badge by direction. A missing strategy snapshot defaults to `false`, or long.
    pub is_short: bool,
    /// The detect's own line, as the core wrote it — the text Moonbot prints in its log.
    ///
    /// Carried rather than dropped because it is the only thing that says WHY the detect fired, and
    /// the chart prints it beside the coin it fired on. Empty when the core sent none, and bounded
    /// to [`DETECT_MSG_KEEP`] on the way in: the ring holds two thousand of these per core, and a
    /// caption cannot show a paragraph anyway.
    pub msg: String,
    /// Name of the strategy that produced it, resolved from the strategy snapshot the same way the
    /// sound and TTL above are, and bounded to [`DETECT_STRAT_NAME_KEEP`] on one line.
    ///
    /// A strategy nobody named still comes back named — `strat <id>` — so EMPTY never means an
    /// unnamed strategy. It means no snapshot backed this detect: an alert firing, which is a drawn
    /// chart object and has no strategy at all, or a detect that arrived before its core's strategy
    /// set. Readers treat the two alike, and can: a snapshot-less detect carries no sound and no
    /// TTL either, so it never becomes a detect card, and the chart caption that reads every row
    /// prints nothing for both.
    pub strat_name: String,
}

impl DetectRow {
    /// This detection's auto-chart TTL, in milliseconds.
    ///
    /// `KeepInChart = 0` becomes `f64::INFINITY` — "keep it indefinitely", as Moonbot does — so
    /// `prune_ttl` never takes such a pane and no close timer is armed for it. Every caller must
    /// come through here: multiplying the field by 1000 turns "forever" into "one millisecond",
    /// and clamping it to at least one turns it into "one second".
    pub fn keep_in_chart_ttl_ms(&self) -> f64 {
        if self.keep_in_chart_secs == 0 {
            f64::INFINITY
        } else {
            (self.keep_in_chart_secs as f64) * 1000.0
        }
    }
}

/// Longest detect line retained, in characters.
///
/// Generous next to what a caption can print — the chart cuts it again to what fits — and small
/// next to what a core is free to send. The bound is here because the retained ring multiplies it:
/// two thousand rows per core, on every core.
pub const DETECT_MSG_KEEP: usize = 200;

/// The strategy field holding its coin blacklist: a comma-separated token list, in the format of
/// the core-wide blacklist ([`crate::symbol::coin_list`]).
pub const FIELD_COINS_BLACK_LIST: &str = "CoinsBlackList";

/// Longest strategy name retained on a detect, in characters.
///
/// Same reasoning as [`DETECT_MSG_KEEP`] and a quarter of it: the wire type behind a strategy name
/// is a 16-bit-length string, the ring multiplies whatever arrives by two thousand rows per core,
/// and a card chip shows a fraction of this much anyway.
pub const DETECT_STRAT_NAME_KEEP: usize = 50;

/// One core server-log line from `Event::ServerLog`, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct CoreLogLine {
    /// Line time in Unix milliseconds from `ServerLogEvent::unix_millis`.
    pub time_ms: i64,
    /// Terminal-local receipt time recorded by the feed thread in Unix milliseconds.
    pub recv_ms: i64,
    pub msg: String,
}

/// One chart alert accepted by the core through `Event::ChartAlert::Upserted`.
///
/// In Moonbot an alert is a drawn chart object, such as a line, channel, or Fibonacci tool, with
/// its Alert option enabled. `blob` is its opaque `TChartObject.Save()` binary. The terminal keeps
/// it unchanged for subsequent `upsert` calls that enable or disable the alert and for format
/// reverse engineering in phase zero. This type is decoupled from moonproto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartAlertRow {
    pub market: String,
    pub obj_uid: u64,
    pub blob: Vec<u8>,
}

/// Change to the core's authoritative chart-alert set.
///
/// The server owns the set, and the terminal requests a full snapshot after reconnecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChartAlertUpdate {
    Upserted(ChartAlertRow),
    Deleted { market: String, obj_uid: u64 },
}

/// Core-built strategy-filter overlay rows for one market.
///
/// MoonBot paints these on its own chart ("Daily vol. doesn't match…", missing tag, EMA miss).
/// The terminal asks for them through [`crate::feed::CoreCmd::SetChartText`] and prints them as a
/// chart caption (`ChartLabelField::StrategyFilters`) — the core is the calculator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartTextRows {
    pub market: String,
    pub filter_lines: Vec<String>,
}
