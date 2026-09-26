//! Local storage settings in `cfg/storage.toml`.
//!
//! One settings file covers all local SQLite databases. The Settings UI's Storage tab writes
//! its keys here, while system settings such as the version ignore list are edited manually.
//! If the file is absent, the first read writes defaults so system keys are visible and editable
//! without consulting the source.

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};

/// Strategy fields whose changes do NOT create a new version (cosmetics/status/presentation).
/// Ported from mb_ai after a year of production use, plus:
/// - `PreventWorkingUntil`: sgStop/sgStart is state, not a parameter edit (tracked in
///   head.checked); otherwise every stop would create a version;
/// - `OrderSize`: order size changes routinely, including through hotkeys, and does not need
///   a parameter version (decision 2026-07-16);
/// - `StrategyName`: renaming is not a parameter edit; the current name lives in the head row
///   (decision 2026-07-16).
pub const DEFAULT_IGNORE_FIELDS: &[&str] = &[
    "Active",
    "LastEditDate",
    "ReportToTelegram",
    "ReportTradesToTelegram",
    "SoundAlert",
    "SoundKind",
    "KeepAlert",
    "SilentNoCharts",
    "AddToChart",
    "KeepInChart",
    "DontKeepOrdersOnChart",
    "UseCustomColors",
    "OrderLineKind",
    "SellOrderColor",
    "BuyOrderColor",
    "DontWriteLog",
    "DebugLog",
    "Comment",
    "PreventWorkingUntil",
    "OrderSize",
    "StrategyName",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageCfg {
    pub strategies: StrategiesStoreCfg,
    pub trade_replay: TradeReplayStoreCfg,
}

/// The `[trade_replay]` section for the persisted trade prints (`trades.sqlite`).
///
/// Read through [`TradeReplayStoreRaw`]: a file written before the margin moved to seconds
/// carries `margin_min`, and the conversion happens on load, not in the parser's field names.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "TradeReplayStoreRaw")]
pub struct TradeReplayStoreCfg {
    /// Whether the prints a trade window fetched are kept on disk for the next window and the
    /// next launch. Off, they live in memory for the session only and the file is not touched.
    pub persist_trades: bool,
    /// Ceiling on the packed prints the file may hold, in megabytes; past it the spans written
    /// longest ago go first. `0` keeps everything, with no age limit.
    pub max_mb: u32,
    /// Seconds of prints kept around a trade, per end: a short position gets this much before
    /// its entry and after its exit; a long one (held past [`Self::long_position_min`]) gets
    /// this much on both sides of each end, with bars between. It sizes what a trade window
    /// fetches, what a close copies out of the core's ring, and what the file keeps. One of
    /// [`TRADE_MARGIN_STEPS_S`]: a hand-edited value is snapped to the nearest step on load.
    pub margin_s: u32,
    /// Minutes a position may be held and still count as SHORT; held past them it is LONG —
    /// walked as its two ends with bars between, by a trade window and by the close-time capture.
    /// Bounded to [`LONG_POSITION_MIN_RANGE`] on load; a file written before the field reads
    /// as the default, which is what the threshold was while it was a constant.
    pub long_position_min: u32,
    /// Whether the terminal runs the Storage tab's cleanup on its own once the cores are up.
    /// Off by default: it rewrites the file unasked. A file written before the field reads as
    /// off.
    pub cleanup_at_startup: bool,
}

/// Default minutes of [`TradeReplayStoreCfg::long_position_min`]: the five minutes the
/// threshold was as a constant.
pub const DEFAULT_LONG_POSITION_MIN: u32 = 5;

/// What [`TradeReplayStoreCfg::long_position_min`] may be, inclusive: one minute — below it
/// every trade is "long" and no window shows its middle — to two hours, the margin's own
/// ceiling.
pub const LONG_POSITION_MIN_RANGE: std::ops::RangeInclusive<u32> = 1..=120;

/// Default ceiling on `trades.sqlite`, megabytes: a day of busy replays is tens of megabytes,
/// so this is months of them for the reader who never touches the setting.
pub const DEFAULT_TRADES_MAX_MB: u32 = 256;

/// The values [`TradeReplayStoreCfg::margin_s`] may take, ascending: the Storage tab steps
/// through this list rather than by a fixed amount, so the short end is fine-grained and the
/// long end coarse. The floor is 30 s — the tuner's run-up and tail
/// (`trade_replay::MODEL_PAD_MS`): one setting sizes the chart's window, the close-time capture,
/// the tuner's fetch and the cleanup alike, and none of them pads it behind the tab's back (the
/// developer's call, 2026-09-23; the steps started at 5 s before that, and the tuner lifted
/// them to a minute on its own). 65 s is a step so the default survives the snap; it is not
/// the floor. The ceiling is two hours: the bar context after an exit is two hours at least,
/// and prints past the bars would have nowhere to draw.
pub const TRADE_MARGIN_STEPS_S: &[u32] = &[30, 60, 65, 180, 300, 600, 900, 1800, 3600, 7200];

/// Default seconds of prints around a trade, per end. 65 s (the user's call, 2026-09-26;
/// 30 s from 2026-09-23, 5 s from 2026-09-21, 15 minutes before that). Not the floor of
/// [`TRADE_MARGIN_STEPS_S`]: 30 s stays the tuner's pad and a step, so a file that already
/// stores 30 keeps 30. A file with no margin key at all takes this default.
pub const DEFAULT_TRADE_MARGIN_S: u32 = 65;

/// Ceiling on [`TradeReplayStoreCfg::margin_s`] — the last of [`TRADE_MARGIN_STEPS_S`].
pub const MAX_TRADE_MARGIN_S: u32 = 7200;

impl Default for TradeReplayStoreCfg {
    fn default() -> Self {
        Self {
            persist_trades: true,
            max_mb: DEFAULT_TRADES_MAX_MB,
            margin_s: DEFAULT_TRADE_MARGIN_S,
            long_position_min: DEFAULT_LONG_POSITION_MIN,
            cleanup_at_startup: false,
        }
    }
}

/// The on-disk shape of `[trade_replay]`, one field wider than the struct: `margin_min` is the
/// key every file written before 2026-09-20 carries, in minutes. Both absent reads as the
/// default; both present, the new key wins — a file the terminal wrote never has both.
#[derive(Deserialize)]
#[serde(default)]
struct TradeReplayStoreRaw {
    persist_trades: bool,
    max_mb: u32,
    margin_s: Option<u32>,
    margin_min: Option<u32>,
    long_position_min: u32,
    cleanup_at_startup: bool,
}

impl Default for TradeReplayStoreRaw {
    fn default() -> Self {
        let d = TradeReplayStoreCfg::default();
        Self {
            persist_trades: d.persist_trades,
            max_mb: d.max_mb,
            margin_s: None,
            margin_min: None,
            long_position_min: d.long_position_min,
            cleanup_at_startup: d.cleanup_at_startup,
        }
    }
}

impl From<TradeReplayStoreRaw> for TradeReplayStoreCfg {
    fn from(raw: TradeReplayStoreRaw) -> Self {
        let margin_s = raw
            .margin_s
            .or_else(|| raw.margin_min.map(|min| min.saturating_mul(60)))
            .unwrap_or(DEFAULT_TRADE_MARGIN_S);
        Self {
            persist_trades: raw.persist_trades,
            max_mb: raw.max_mb,
            margin_s,
            long_position_min: raw.long_position_min,
            cleanup_at_startup: raw.cleanup_at_startup,
        }
    }
}

/// [`LONG_POSITION_MIN_RANGE`] applied to a value from the file or the tab.
pub fn clamp_long_position_min(minutes: u32) -> u32 {
    minutes.clamp(
        *LONG_POSITION_MIN_RANGE.start(),
        *LONG_POSITION_MIN_RANGE.end(),
    )
}

/// The step of [`TRADE_MARGIN_STEPS_S`] nearest to `secs` — the lower one when `secs` sits
/// exactly between two (a migrated `margin_min = 45` lands on 30 minutes, not 60). Anything
/// past the last step is the last step, anything under the first is the first.
///
/// Args:
///     secs: A margin in seconds, from the file or a caller.
///
/// Returns:
///     A member of [`TRADE_MARGIN_STEPS_S`].
pub fn snap_trade_margin_s(secs: u32) -> u32 {
    TRADE_MARGIN_STEPS_S
        .iter()
        .copied()
        .min_by_key(|step| (step.abs_diff(secs), *step))
        .unwrap_or(DEFAULT_TRADE_MARGIN_S)
}

/// The step `delta` places away from `secs` in [`TRADE_MARGIN_STEPS_S`], from the step nearest
/// to `secs`; the list's ends absorb the rest. What the Storage tab's stepper does.
///
/// Args:
///     secs: The current margin in seconds.
///     delta: How many steps to move, negative for shorter.
///
/// Returns:
///     A member of [`TRADE_MARGIN_STEPS_S`].
pub fn step_trade_margin_s(secs: u32, delta: i32) -> u32 {
    let snapped = snap_trade_margin_s(secs);
    let index = TRADE_MARGIN_STEPS_S
        .iter()
        .position(|&step| step == snapped)
        .unwrap_or(0);
    let last = TRADE_MARGIN_STEPS_S.len() - 1;
    let target = (index as i64 + i64::from(delta)).clamp(0, last as i64) as usize;
    TRADE_MARGIN_STEPS_S[target]
}

/// The `[strategies]` section for the local strategy and version database.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategiesStoreCfg {
    /// Whether to maintain the local strategy database (head + versions). Disabling stops
    /// writes while preserving existing history on disk.
    pub enabled: bool,
    /// Maximum versions per strategy (0 = unlimited). Older versions are pruned.
    pub version_limit: u32,
    /// Fields whose changes do not create a version. This system setting is not exposed in
    /// the UI and is edited manually; accidentally excluding a field silently loses edit history.
    pub ignore_fields: Vec<String>,
}

impl Default for StrategiesStoreCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            version_limit: 0,
            ignore_fields: DEFAULT_IGNORE_FIELDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// Reads `storage.toml`; if absent, writes defaults so system keys are visible. A corrupt file
/// is logged and defaults are used without overwriting it.
pub fn load() -> StorageCfg {
    let path = paths::storage_path();
    if !path.exists() {
        let cfg = StorageCfg::default();
        if let Err(e) = toml_io::save(&path, &cfg, "storage.toml") {
            log::warn!("storage.toml: не удалось записать дефолт: {e:#}");
        }
        return cfg;
    }
    sanitize(toml_io::load_or_default(&path, "storage.toml", |_| {}))
}

/// Bound what a hand-edited file may carry: the long-position threshold is clamped to
/// [`LONG_POSITION_MIN_RANGE`], and the margin is snapped onto [`TRADE_MARGIN_STEPS_S`],
/// which also caps it at [`MAX_TRADE_MARGIN_S`].
fn sanitize(mut cfg: StorageCfg) -> StorageCfg {
    cfg.trade_replay.margin_s = snap_trade_margin_s(cfg.trade_replay.margin_s);
    cfg.trade_replay.long_position_min =
        clamp_long_position_min(cfg.trade_replay.long_position_min);
    cfg
}

/// Saves storage settings from the Storage tab.
pub fn save(cfg: &StorageCfg) {
    if let Err(e) = toml_io::save(&paths::storage_path(), cfg, "storage.toml") {
        log::warn!("storage.toml: сохранение не удалось: {e:#}");
    }
}

#[cfg(test)]
mod tests;
