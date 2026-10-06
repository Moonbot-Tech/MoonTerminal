//! Persisted report filters, table sorting, and tuner preferences.

use super::*;

/// One Report toolbar filter set, persisted per host context.
///
/// Holds seven stored members: the six shared Report toolbar filters that decide WHICH TRADES the
/// panel reads — direction, order kind, the deleted-only switch, the open-positions switch, the
/// single-server period preset, and the Auto strategy-name mask — plus the Auto Overview period
/// preset. The comment pane is a
/// display choice and stays in `app_meta` beside the other view preferences; the split is
/// deliberate, so do not "unify" the two stores. These filters belong here because they must
/// survive a quit that a detached preference write would not: the whole layout rides the quit
/// snapshot, and it outlives a report replica that integrity recovery retires.
///
/// Every field is optional and read leniently, so a wrongly-typed member drops only THAT field to
/// `None` and leaves its neighbours, and the rest of the layout, intact. Unknown string ids remain
/// stored here because this crate does not own their vocabulary; the Report decoder treats them as
/// no instruction and keeps the panel's current value. One level up the salvage is coarser: an
/// entry that is not a table at all takes the whole `report_filters` map down to empty with it, the
/// same as every other leniently-read map here. Both outcomes cost only filter preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportFilterPrefs {
    /// Direction filter id.
    ///
    /// Opaque here: this crate stores it, and the Report panel's own encoder in `moon-ui-gpui`
    /// owns the vocabulary. Listing the values in both places is how one copy goes quietly wrong.
    #[serde(default, deserialize_with = "de_lenient")]
    pub side: Option<String>,
    /// Order-kind id, opaque here for the same reason as [`Self::side`].
    #[serde(default, deserialize_with = "de_lenient")]
    pub kind: Option<String>,
    /// Whether the panel showed only soft-deleted trades.
    #[serde(default, deserialize_with = "de_lenient")]
    pub deleted_only: Option<bool>,
    /// Whether the panel admits still-running positions alongside closed trades when its host does
    /// not force closed rows.
    ///
    /// A LIFECYCLE axis, independent of [`Self::kind`], which is about a trade's ORIGIN. Absent
    /// means "no instruction", and the Report decoder then keeps its own default of ON — which is
    /// exactly what every file written before this field existed must continue to mean.
    #[serde(default, deserialize_with = "de_lenient")]
    pub show_open: Option<bool>,
    /// Which timestamp the period bounds apply to, as the Report panel's own id — opaque here for
    /// the same reason as [`Self::side`].
    ///
    /// Absent means the close date, which is what every file written before this field existed
    /// already meant.
    #[serde(default, deserialize_with = "de_lenient")]
    pub period_basis: Option<String>,
    /// Classic and Auto single-server period preset id — the panel's menu key, opaque here for
    /// the same reason as [`Self::side`].
    ///
    /// Only an explicit menu pick is stored. Typing a manual date also displays "all", but that is
    /// a consequence of the date rather than a chosen preset, so it never reaches this field.
    #[serde(default, deserialize_with = "de_lenient")]
    pub period: Option<String>,
    /// Auto Overview period preset id, falling back to [`Self::period`] when absent or unknown.
    ///
    /// Only an explicit menu pick is stored, matching the manual-date rule on [`Self::period`].
    #[serde(default, deserialize_with = "de_lenient")]
    pub period_overview: Option<String>,
    /// Strategy-name query retained for group Auto mode, in the shared syntax (comma = OR,
    /// space = AND, `!word` excludes; see `moon_core::strategy_query`).
    ///
    /// `Some("")` is a deliberate clear. A missing or malformed value leaves the panel's current
    /// value standing when it changes host context.
    #[serde(default, deserialize_with = "de_lenient")]
    pub strategy_name_mask: Option<String>,
    /// Report-only narrowing of the Auto Overview scope, as core uids; empty or absent means the
    /// whole Overview. A set that no longer intersects the scope is read as the whole Overview.
    // wire-id-exempt: terminal-issued core uids, never a core id — see `config::wire_id`.
    #[serde(default, deserialize_with = "de_lenient")]
    pub overview_cores: Option<Vec<u64>>,
}

/// One user-selected table sort stored under a stable per-context table id.
///
/// Column vocabulary remains panel-owned: this core crate only preserves the stable key and the
/// direction MoonUI reports. Panels validate the key against their current descriptors before
/// adopting it, so a renamed or removed column cannot make a table unusable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSortPreference {
    /// Stable column key defined by the owning table.
    pub column: String,
    /// Whether the selected column is ordered ascending.
    pub ascending: bool,
}

/// The "Entry/Exit" tuner's persisted settings ([`WindowLayout::analytics_ticks`]). Every field
/// has a default, so a block missing any of them reads the rest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TicksAxisLayout {
    /// Restart count of the search; `None` = the axis default.
    pub iters: Option<u32>,
    /// Percentage of the period the search may fit on; `None` = the whole period.
    pub train: Option<u32>,
    /// Whether `train` was written under the 70 % default: a layout without it carries the old
    /// 100 % default for every user, which the axis then reads as unset, once.
    pub train_v2: bool,
    /// Base seed of the restarts, as text (see [`WindowLayout::analytics_tuner_seed`]); `None`
    /// draws one per search.
    pub seed: Option<String>,
    /// Passes of coordinate descent per restart; `None` = the search's default.
    pub passes: Option<u32>,
    /// The share of reproduced trades, per cent, under which a parameter group's heading warns
    /// that the search's answer speaks for fewer trades (it no longer locks the group out);
    /// `None` = the axis default.
    pub gate_pct: Option<u32>,
    /// How much deeper than the fact's max drawdown the search's answer may fall, per cent;
    /// `None` = the search's default (`search::DEFAULT_WORSE_PCT`).
    #[serde(deserialize_with = "de_lenient")]
    pub dd_worse_pct: Option<f64>,
    /// How much lower than the fact's win rate the search's answer may be, per cent; `None` =
    /// the search's default.
    #[serde(deserialize_with = "de_lenient")]
    pub wr_worse_pct: Option<f64>,
    /// Strategy fields the search holds at their base value — the unticked grid rows.
    pub locked: Vec<String>,
    /// The model's own settings.
    pub model: crate::db::tuner::ticks::ModelSettings,
    /// Whether the trade pane under the deal table is open.
    pub trade_open: bool,
    /// Whether the search may bring a trade's entry corridor nearer the price than the trade's
    /// own. Off by default — and stored this way round so that a config written before the
    /// switch existed reads it off, the guard on (`SearchParams::keep_corridor`).
    pub allow_closer_corridor: bool,
    /// Whether a search of both groups runs a whole exit search under every entry move it tries,
    /// rather than under the few a quick score ranks first (`SearchParams::screen_entry`). Off by
    /// default — stored this way round so that a config written before the switch existed reads
    /// it off, the screen on.
    pub exit_under_every_entry: bool,
    /// The shortest tape past the close, seconds, a deal must hold to be worked on — the sample
    /// the variant columns and the search run on; `None` = the axis default. The tape of an
    /// older trade cannot be fetched again, and one short tail cut every variant's exit at it.
    pub min_tail_s: Option<u32>,
    /// Steps per field the automatic search ranges are cut into; `None` = the axis default
    /// (`params::range::steps_of`).
    #[serde(deserialize_with = "de_lenient")]
    pub steps_per_param: Option<u32>,
    /// The search ranges the user typed over the automatic ones, by field key — only fields with
    /// a slot typed; a malformed entry is dropped alone.
    #[serde(deserialize_with = "de_tick_ranges")]
    pub ranges:
        std::collections::BTreeMap<String, crate::db::tuner::ticks::params::range::TickRange>,
}
