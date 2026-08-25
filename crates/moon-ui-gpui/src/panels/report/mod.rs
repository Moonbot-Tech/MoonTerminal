//! Report panel ported from egui's `src/dock/report_view.rs`.
//!
//! It displays trades from the local SQLite database — closed ones within the selected period,
//! plus still-running positions only when the host does not force closed rows, that period reaches
//! the present, AND the scope field's open-positions switch admits them ([`RowScope`], whose money
//! the footer states apart from the realized total) — with core, coin, exact-strategy,
//! Auto strategy-name, and date filters, one merged scope field (side, order kind, deleted trades,
//! open positions, comment pane) plus column selection above the table, the current row's comment
//! and exact period
//! totals below it. The
//! generic table supports every displayable database column and header-click sorting. A writer
//! generation counter in `Backend.reports` triggers throttled automatic refreshes.
//!
//! Responsibilities are split across this module for state, queries, and lifecycle; [`controls`]
//! for selectors and the column menu; [`columns`] for column, cell, and header formatting; and
//! [`export`] for file export; and [`totals`] for footer fact priority and recovery text.

mod actions;
mod columns;
mod comment;
mod controls;
mod export;
mod query;
mod render;
mod selection;
mod state;
mod strategy_filter;
mod totals;
mod trade_detail;
mod trade_log;
mod widths;
mod window;

use columns::{as_i64, value_to_string};
use controls::ReportScopeControl;
use query::ReportData;
use selection::ReportSelection;
use strategy_filter::{
    ReportStrategyCatalog, ReportStrategyChoice, ReportStrategyDelegate, ReportStrategySearch,
    exact_strategy_selection, merge_strategy_metadata, normalized_strategy_filter_keys,
    ordered_strategy_cores, strategy_choice_indices, strategy_groups, strategy_selection_summary,
};
use widths::{NaturalWidthsCache, complete_widths};

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::Datelike as _;
use chrono_tz::Tz;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::MoonWindowExt as _;
use moon_ui::{
    DockArea, MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonCombobox, MoonComboboxEvent, MoonComboboxMenuChrome, MoonComboboxState,
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDataTableState,
    MoonDataTableWidthPolicy, MoonDateTimePicker, MoonDateTimePickerEvent, MoonDateTimePickerState,
    MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize,
    MoonNotification, MoonPalette, MoonScrollbarVisibility, MoonTone, MoonWindowFrame, Panel,
    PanelEvent, PanelState, Root, StyledExt, h_flex, rgba_from, v_flex,
};
use rusqlite::types::Value;
use rust_i18n::t;

use crate::controls::date_range::{self, Bound};
use crate::core_order::CoreOrder;
use crate::load_state::{LoadState, Note, note_el};
use crate::workspace::{EffectiveCoreScope, RetainedCoreScope};
use crate::{Backend, design};
use moon_core::db::valuation::ValuationStatus;
use moon_core::db::{
    self, ReadResult, ReportAxis, ReportFilter, ReportStrategy, ReportStrategyKey, SideFilter,
};
use moon_core::session::CoreId;

pub use window::open_scoped;

/// Moonbot-style report period presets with selected-zone civil-day boundaries.
///
/// Two families, deliberately named apart because a preset that only says "week" or "year" cannot
/// be read: a CALENDAR preset starts at the period's own boundary (Monday, the 1st, January 1st),
/// while a ROLLING one counts a fixed number of days back from today inclusive. The menu keeps the
/// families in separate groups for the same reason.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    All,
    Today,
    Yesterday,
    /// The calendar week from Monday to today.
    CurWeek,
    /// The calendar month from the 1st to today.
    CurMonth,
    /// The calendar year from January 1st to today.
    CurYear,
    /// The rolling seven days ending today.
    Days7,
    /// The rolling thirty days ending today.
    Days30,
    /// The rolling three hundred and sixty-five days ending today.
    Days365,
}

impl Period {
    /// The menu's four groups in display order, rendered with a separator between them.
    ///
    /// This is the single declaration of menu membership and grouping, so a variant added without
    /// a home here remains unreachable from the menu rather than being silently appended.
    pub(super) const GROUPS: [&'static [Self]; 4] = [
        &[Self::Today, Self::Yesterday],
        &[Self::CurWeek, Self::CurMonth, Self::CurYear],
        &[Self::Days7, Self::Days30, Self::Days365],
        &[Self::All],
    ];

    /// Return the stable menu-item element id for this preset.
    ///
    /// Named rather than positional so reordering [`Self::GROUPS`] cannot silently re-point a menu
    /// item at a different preset. Returned whole and `'static` because the menu is rebuilt on the
    /// panel's render path, where formatting nine ids per frame would allocate for nothing.
    ///
    /// Returns:
    ///     The element id, unique across the menu.
    pub(super) fn menu_key(self) -> &'static str {
        match self {
            Self::All => "rp-all",
            Self::Today => "rp-today",
            Self::Yesterday => "rp-yesterday",
            Self::CurWeek => "rp-cur-week",
            Self::CurMonth => "rp-cur-month",
            Self::CurYear => "rp-cur-year",
            Self::Days7 => "rp-days-7",
            Self::Days30 => "rp-days-30",
            Self::Days365 => "rp-days-365",
        }
    }

    /// Restore a preset from a persisted [`Self::menu_key`].
    ///
    /// Deliberately its OWN exhaustive match rather than a lookup over [`Self::GROUPS`]. Deriving
    /// it from the menu would make writing and reading share one authority: dropping a variant
    /// from `GROUPS`, or renaming every key coherently, would keep a round-trip green while every
    /// value already on disk silently stopped restoring.
    ///
    /// Args:
    ///     key: Stored menu key.
    ///
    /// Returns:
    ///     The preset, or `None` for a key this build does not know — which leaves the panel's own
    ///     default standing.
    pub(super) fn from_menu_key(key: &str) -> Option<Self> {
        Some(match key {
            "rp-all" => Self::All,
            "rp-today" => Self::Today,
            "rp-yesterday" => Self::Yesterday,
            "rp-cur-week" => Self::CurWeek,
            "rp-cur-month" => Self::CurMonth,
            "rp-cur-year" => Self::CurYear,
            "rp-days-7" => Self::Days7,
            "rp-days-30" => Self::Days30,
            "rp-days-365" => Self::Days365,
            _ => return None,
        })
    }

    /// Return this preset's localized menu label.
    ///
    /// The rolling presets deliberately keep the older `report.period.week`/`.month`/`.year` keys,
    /// whose values now name a day count in all three languages: a bare "week" beside a calendar
    /// "this week" is exactly the ambiguity the grouping exists to remove.
    ///
    /// Returns:
    ///     The label in the active locale.
    pub(super) fn label(self) -> String {
        match self {
            Self::All => t!("report.filter.all"),
            Self::Today => t!("report.period.today"),
            Self::Yesterday => t!("report.period.yesterday"),
            Self::CurWeek => t!("report.period.cur_week"),
            Self::CurMonth => t!("report.period.cur_month"),
            Self::CurYear => t!("report.period.cur_year"),
            Self::Days7 => t!("report.period.week"),
            Self::Days30 => t!("report.period.month"),
            Self::Days365 => t!("report.period.year"),
        }
        .to_string()
    }

    /// Return inclusive preset bounds at one pinned UTC instant.
    ///
    /// Args:
    ///     now: Current UTC Unix timestamp in seconds.
    ///     zone: User-selected display time zone.
    ///
    /// Returns:
    ///     Inclusive absolute bounds; `None` leaves that edge unbounded.
    fn range_at(self, now: i64, zone: Tz) -> (Option<i64>, Option<i64>) {
        let Some(today) = moon_core::util::display_time::date(now, zone) else {
            return (None, None);
        };
        let day_start = |date| moon_core::util::display_time::day_start(date, zone);
        let day = day_start(today);
        let shifted_start = |days| {
            day.and_then(|start| moon_core::util::display_time::shift_day_start(start, days, zone))
        };
        let calendar = |first: Option<chrono::NaiveDate>| (first.and_then(day_start).or(day), None);
        match self {
            Self::All => (None, None),
            Self::Today => (day, None),
            Self::Yesterday => (shifted_start(-1), day.map(|value| value - 1)),
            // Calendar presets differ ONLY in which first date they step back to; the rest of the
            // rule is shared. `day_start` survives a zone whose midnight does not exist that day,
            // and `.or(day)` keeps a preset that cannot resolve its boundary bounded at today
            // rather than silently unbounded.
            Self::CurWeek => calendar(Some(moon_core::util::display_time::shift_date(
                today,
                -i64::from(today.weekday().num_days_from_monday()),
            ))),
            Self::CurMonth => calendar(today.with_day(1)),
            Self::CurYear => calendar(today.with_month(1).and_then(|date| date.with_day(1))),
            // Rolling windows count back from today INCLUSIVE, so the shift is one day short of the
            // window: seven days is today plus the six before it.
            Self::Days7 => (shifted_start(-6), None),
            Self::Days30 => (shifted_start(-29), None),
            Self::Days365 => (shifted_start(-364), None),
        }
    }
}

/// Durable period slot selected by the live Report workspace scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReportPeriodBucket {
    /// Auto Overview, which owns its dedicated stored period.
    Overview,
    /// Classic and every Auto single-server scope, which share the legacy stored period.
    Single,
}

/// Resolve which trades the panel's current controls ask the database for.
///
/// THREE authorities meet here and their precedence is fixed rather than incidental.
///
/// `closed_only` is a QUERY predicate the HOST set — today only the Analytics-owned scoped window
/// — so it wins outright: someone who asked for closed trades gets closed trades whatever else is
/// showing. `show_open` is the USER's own toolbar switch and comes next: with it off, the panel
/// excludes still-running positions however far into the present the period reaches. Only when
/// neither has excluded them does the PERIOD decide — and the period's verdict is no longer this
/// layer's to give. `date_to` is compared against a CORE-LOCAL column, so one window can have
/// ended for a core running behind UTC and still be current for one running ahead; the panel sees
/// one period and cannot resolve that for a fleet. It therefore records the INTENT
/// ([`db::RowScope::ClosedAndOpenIfCurrent`]) and `db::report_read` decides per offset group,
/// where both the axis and the cores in play are known.
///
/// The first two are written as ONE disjunction rather than a nesting because both roads lead to
/// the same answer, and spelling it that way keeps "the period only decides when nobody asked"
/// visible at a glance.
///
/// Written as one named function rather than a field threaded through, so the three facts compose
/// in exactly one place instead of at every construction site. That single place is what makes the
/// rows, the totals and the export agree by construction: they share ONE [`db::ReportFilter`], so
/// there is no second path on which a footer could total trades the table is not showing.
///
/// Args:
///     closed_only: Whether the HOST deliberately excludes still-running positions.
///     show_open: Whether the USER's toolbar switch admits still-running positions.
///
/// Returns:
///     The row scope the database filter carries.
pub(super) fn row_scope_for(closed_only: bool, show_open: bool) -> db::RowScope {
    if closed_only || !show_open {
        db::RowScope::Closed
    } else {
        db::RowScope::ClosedAndOpenIfCurrent
    }
}

/// Resolve the durable period slot for one effective workspace scope.
///
/// Args:
///     scope: Current group scope, or `None` for a standalone Report.
///
/// Returns:
///     Overview only for workspace-owned non-core scope; every other scope is Single.
pub(super) fn period_bucket_for_scope(scope: Option<&EffectiveCoreScope>) -> ReportPeriodBucket {
    if scope.is_some_and(|scope| scope.is_workspace_owned() && !scope.is_auto_core()) {
        ReportPeriodBucket::Overview
    } else {
        ReportPeriodBucket::Single
    }
}

/// Decode the period stored for one live bucket over the panel's current value.
///
/// Args:
///     prefs: Persisted toolbar filters for the current host context.
///     bucket: Live period bucket.
///     current: Period currently displayed by the panel.
///
/// Returns:
///     The valid bucket value, the legacy Single value for an absent or unknown Overview value,
///     or `current` when neither applicable stored id is known.
pub(super) fn apply_period_from_prefs(
    prefs: &moon_core::config::ReportFilterPrefs,
    bucket: ReportPeriodBucket,
    current: Period,
) -> Period {
    let decode = |value: Option<&String>| value.map(String::as_str).and_then(Period::from_menu_key);
    match bucket {
        ReportPeriodBucket::Overview => decode(prefs.period_overview.as_ref())
            .or_else(|| decode(prefs.period.as_ref()))
            .unwrap_or(current),
        ReportPeriodBucket::Single => decode(prefs.period.as_ref()).unwrap_or(current),
    }
}

/// Compose the next persisted toolbar entry while changing only the live period bucket.
///
/// Args:
///     existing: Current host entry, retained so the inactive period bucket survives.
///     bucket: Live period bucket receiving an explicit pick.
///     picked_period: Explicit menu pick, or `None` to preserve both stored period values.
///     live: The panel's live toolbar filters. Its `period` member is deliberately NOT read —
///         the period slot is chosen by `bucket` and written only from `picked_period`, because
///         a displayed period is not always an explicit menu pick.
///
/// Returns:
///     A complete host entry with shared filters refreshed and only the selected period slot
///     changed.
pub(super) fn next_prefs_for_period_pick(
    existing: Option<&moon_core::config::ReportFilterPrefs>,
    bucket: ReportPeriodBucket,
    picked_period: Option<Period>,
    live: &state::ReportFilterSet,
) -> moon_core::config::ReportFilterPrefs {
    let mut prefs = existing.cloned().unwrap_or_default();
    prefs.side = Some(side_id(live.side).to_string());
    prefs.kind = Some(live.kind.id().to_string());
    prefs.deleted_only = Some(live.deleted_only);
    prefs.show_open = Some(live.show_open);
    prefs.strategy_name_mask = Some(live.strategy_name_mask.clone());
    if let Some(period) = picked_period {
        let key = Some(period.menu_key().to_string());
        match bucket {
            ReportPeriodBucket::Overview => prefs.period_overview = key,
            ReportPeriodBucket::Single => prefs.period = key,
        }
    }
    prefs
}

/// Return whether the effective workspace owns the strategy-name mask.
///
/// Both Auto Overview and a selected Auto core are workspace-owned Report scopes. Classic and
/// standalone Reports retain the stored value but must neither render nor apply it.
fn strategy_name_mask_enabled(scope: Option<&crate::workspace::EffectiveCoreScope>) -> bool {
    scope.is_some_and(crate::workspace::EffectiveCoreScope::is_workspace_owned)
}

#[cfg(test)]
mod tests;

/// Report order-type filter: all, real, or emulator orders.
///
/// Real orders are selected by default.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReportKind {
    All,
    Real,
    Emu,
}

impl ReportKind {
    /// Convert to the database filter: `None` selects all, `Some(false)` real, and `Some(true)` emulated.
    fn to_filter(self) -> Option<bool> {
        match self {
            ReportKind::All => None,
            ReportKind::Real => Some(false),
            ReportKind::Emu => Some(true),
        }
    }

    /// Return the stable id this kind persists as.
    ///
    /// Named rather than positional, and matched exhaustively so a new variant fails the build
    /// until it has an id of its own. `id`/`from_id` is the repo's settled spelling for a
    /// persisted-enum pair.
    fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Real => "real",
            Self::Emu => "emu",
        }
    }

    /// Restore a kind from a persisted [`Self::id`].
    ///
    /// Args:
    ///     id: Stored id.
    ///
    /// Returns:
    ///     The kind, or `None` for an id this build does not know.
    fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "all" => Self::All,
            "real" => Self::Real,
            "emu" => Self::Emu,
            _ => return None,
        })
    }

    /// Restore the UI kind selector from the database emulator filter.
    ///
    /// Args:
    ///     emulator: Database emulator constraint.
    ///
    /// Returns:
    ///     The equivalent Report kind option.
    fn from_filter(emulator: Option<bool>) -> Self {
        match emulator {
            None => Self::All,
            Some(false) => Self::Real,
            Some(true) => Self::Emu,
        }
    }
}

/// Return the stable id a direction filter persists as.
///
/// A free function because [`SideFilter`] belongs to `moon-core`, where nothing else needs a UI
/// persistence id. The match is exhaustive, so a new direction there fails this build until it has
/// an id.
///
/// Args:
///     side: Direction filter to encode.
///
/// Returns:
///     The stored id.
fn side_id(side: SideFilter) -> &'static str {
    match side {
        SideFilter::All => "all",
        SideFilter::Long => "long",
        SideFilter::Short => "short",
    }
}

/// Restore a direction filter from a persisted [`side_id`].
///
/// Args:
///     id: Stored id.
///
/// Returns:
///     The direction, or `None` for an id this build does not know.
fn side_from_id(id: &str) -> Option<SideFilter> {
    Some(match id {
        "all" => SideFilter::All,
        "long" => SideFilter::Long,
        "short" => SideFilter::Short,
        _ => return None,
    })
}

/// Return the storage key this host context's toolbar filters persist under.
///
/// Spelled once so the reader and the writer cannot drift onto different contexts. Keyed exactly
/// like the panel's column layout, through the shared context-id helper.
///
/// Args:
///     detached: Whether the host is a window rather than a docked tab.
///
/// Returns:
///     `report-filters:win` or `report-filters:dock`.
fn filters_ctx_id(detached: bool) -> String {
    crate::persistence::table_persist::ctx_id("report-filters", detached)
}

/// Initial or replacement filter applied by Analytics to a standalone Report window.
#[derive(Clone, Debug)]
pub struct ReportScope {
    /// Exact strategy identity, including the core on which the id is meaningful.
    pub strategy: ReportStrategyKey,
    /// Human-readable name retained even when report metadata has not loaded yet.
    pub strategy_name: String,
    /// Inclusive UTC lower close-date bound.
    pub date_from: Option<i64>,
    /// Inclusive UTC upper close-date bound.
    pub date_to: Option<i64>,
    /// Direction copied from the Analytics filters.
    pub side: SideFilter,
    /// Emulator filter copied from Analytics.
    pub emulator: Option<bool>,
}

/// Resolve the coin-search group for one Report core.
///
/// Args:
///     backend: Shared application backend.
///     core_uid: Exact Report core identity.
///     cx: Application context used to inspect live sessions.
///
/// Returns:
///     The live core group, or the same deterministic fallback used for offline cores.
fn report_group_for_core(backend: &Entity<Backend>, core_uid: u64, cx: &App) -> String {
    backend
        .read(cx)
        .session
        .sessions()
        .iter()
        .find(|session| session.id == core_uid)
        .map(|session| session.group.clone())
        .unwrap_or_else(|| "default".to_string())
}

/// Database column names visible by default.
///
/// Listed in `db::DISPLAY_COLUMNS` order for readability only: the set is collapsed into a
/// name-keyed `HashSet`, and render order comes from that constant, never from here.
const DEFAULT_VISIBLE: &[&str] = &[
    "buydate",
    "closedate",
    "core_name",
    "coin",
    "isshort",
    "quantity",
    "buyprice",
    "sellprice",
    "profitbtc",
    "valuation_profit_usdt",
    "profitpct",
    "lev",
    "sellreason",
    "comment",
];

/// Closed-trade report surface shared by docked, detached, and scoped standalone hosts.
pub struct ReportPanel {
    pub(super) backend: Entity<Backend>,
    /// User-selected zone applied to every visible date and civil period boundary.
    pub(super) display_zone: Tz,
    /// The measured time axis this panel reads replicated timestamps on, cached.
    ///
    /// Rebuilt from the retained core snapshots whenever the backend changes or the zone moves,
    /// never inside `render`: it is read once per cell and once per filter, and rebuilding it
    /// there would walk the whole core list on every paint. Starts as the identity, which is the
    /// correct answer until something has actually been measured.
    pub(super) axis: ReportAxis,
    /// Whether picker civil values must be rewritten from their preserved absolute bounds.
    display_zone_fields_dirty: bool,
    pub(super) group: String,
    generation: Option<Arc<AtomicU64>>,
    /// Historical-cache and current-rate data generation combined with report commits for refresh
    /// detection.
    valuation_generation: Option<Arc<AtomicU64>>,
    /// Latest published worker health, refreshed only when its revision moves. Polled separately
    /// from the data generation because a stalled worker commits no rows at all.
    valuation_status: ValuationStatus,
    last_gen: u64,
    /// Health revision already folded into `valuation_status`.
    last_status_rev: u64,
    /// Valuation mode this panel's current rows were queried under.
    ///
    /// Settings may change the application-wide mode while this panel is open, and the change
    /// moves no data generation, so the queried mode is tracked separately.
    last_valuation_mode: moon_core::db::valuation::ValuationMode,

    pub(super) cores: Vec<(u64, String)>,
    /// Strategy identities currently available to the exact strategy selector.
    pub(super) strategies: Vec<ReportStrategy>,
    /// Exact keys confirmed by the latest metadata refresh, excluding retained stale choices.
    pub(super) available_strategy_keys: HashSet<ReportStrategyKey>,
    /// Cached schema, kept outside `data` so failures cannot collapse controls or widths.
    pub(super) cols: Rc<Vec<String>>,
    /// Report rows and totals; in-flight refreshes may retain stale data, but
    /// `NotReady` and `Failed` retain none.
    data: LoadState<ReportData>,

    sort_key: String,
    sort_desc: bool,

    /// Retained core filter for Classic group views and standalone Reports; empty means all cores.
    /// Group Auto mode pins its effective workspace scope without using or mutating this set.
    pub(super) sel_cores: HashSet<u64>,
    /// Exact selected strategies, or `None` for implicit All as in the shared core selector.
    pub(super) selected_strategies: Option<HashSet<ReportStrategyKey>>,
    /// Searchable, grouped, virtualized MoonUI selector synchronized with Report filters.
    strategy_select: Entity<MoonComboboxState<ReportStrategyDelegate>>,
    /// Immutable grouped rows and availability indices retained until metadata changes.
    strategy_catalog: Rc<ReportStrategyCatalog>,
    /// Search text shared across metadata delegate replacements.
    strategy_search: ReportStrategySearch,
    /// Whether metadata changes require replacing the grouped combobox delegate on next render.
    strategy_select_items_dirty: bool,
    /// Whether filter changes require replacing the retained combobox selection on next render.
    strategy_select_selection_dirty: bool,
    /// Literal strategy-name substring retained across Classic and Auto workspace switches.
    strategy_name_mask: String,
    /// Auto-workspace input whose value is mirrored in [`Self::strategy_name_mask`].
    strategy_name_mask_input: Entity<MoonInputState>,
    coin: Entity<MoonInputState>,
    /// Mirror of the coin input, updated on `Change`.
    ///
    /// This distinguishes manual edits, which open the match popup, from `on_pick` substitutions,
    /// which update the mirror first so their change event does not reopen the popup.
    coin_query: String,
    /// Whether the shared `controls::coin_search` match popup is open.
    coin_popup_open: bool,
    from: Entity<MoonDateTimePickerState>,
    /// Mirror of the From field in UTC unix seconds, used to suppress duplicate scoped-update
    /// queries. The field picks whole minutes, so this is the first second of the picked minute.
    from_query: Option<i64>,
    to: Entity<MoonDateTimePickerState>,
    /// Mirror of the To field in UTC unix seconds, on the same whole-minute grid as `from_query`.
    /// The inclusive filter bound is derived from it in `filter()`, not stored here.
    to_query: Option<i64>,
    /// Whether a bound changed while its popup was open and still owes a query.
    ///
    /// Every clock-drum step emits `Change`; the mirrors follow it immediately, but the read is
    /// issued once, when the popup closes.
    bounds_pending: bool,
    /// Guards programmatic picker rewrites from being mistaken for user filter edits.
    bound_write_in_progress: bool,
    /// Direction filter, persisted per host in `layout.report_filters`.
    ///
    /// This and the three fields below are the scope control's FILTER members: they decide which
    /// trades the panel reads, so they are stored where a quit cannot lose them and a report-replica
    /// recovery cannot take them with it. Their neighbour `show_comment` is a display choice and
    /// stays in `app_meta`; the split is deliberate.
    pub(super) side: SideFilter,
    /// Period preset, defaulting to Today as in Moonbot. Persisted beside [`Self::side`].
    ///
    /// Editing a non-empty manual date switches to All. Under other presets, the preset lower bound
    /// overrides From, while only Yesterday supplies an upper bound that overrides To.
    pub(super) period: Period,
    /// Last resolved workspace period bucket, used to distinguish cross-bucket rail switches.
    last_period_bucket: ReportPeriodBucket,
    /// All, real, or emulated order kind, defaulting to real as in Orders. Persisted beside
    /// [`Self::side`].
    pub(super) kind: ReportKind,
    /// Show ONLY soft-deleted trades when set; hide them when clear (the default). Persisted beside
    /// [`Self::side`].
    pub(super) deleted_only: bool,
    /// Show still-running positions alongside closed trades. ON by default, which is what the
    /// panel has always done. Persisted beside [`Self::side`].
    ///
    /// A DIFFERENT axis from [`Self::kind`] — that one is a trade's ORIGIN (real or emulated),
    /// this one is its LIFECYCLE — which is why the two are independently selectable rather than
    /// rows of one list. [`Self::closed_only`] still wins outright over it; the precedence is
    /// spelled once, in [`row_scope_for`].
    pub(super) show_open: bool,
    /// Whether Analytics owns this panel's filters.
    ///
    /// A scoped standalone window is handed its side, kind and dates by Analytics and forces
    /// `Period::All`; persisted values neither apply to it nor are written from it, or reopening a
    /// strategy would show something other than that strategy. Set from the constructor's scope
    /// argument and by `apply_scope`, never from `detached` or `standalone` — the first is a host
    /// class, and the second is assigned after the host change has already read the store.
    pub(super) scoped: bool,
    /// Whether the full-width comment pane is shown between the table and the totals. On by
    /// default and persisted in `app_meta` per host: a docked tab and a detached window keep
    /// separate answers, exactly as their column sets and widths do. A display choice, not a filter.
    pub(super) show_comment: bool,
    /// Whether the background metadata read has supplied the current host preference.
    comment_metadata_loaded: bool,
    /// User-edit generations that reject stale initial or host-specific metadata.
    preference_revisions: state::ReportPreferenceRevisions,
    /// Retained owner-scoped dropdown whose popup invalidations never rebuild the Report body.
    scope_control: Entity<ReportScopeControl>,
    /// Whether this Analytics-scoped panel excludes undated/non-positive close timestamps.
    ///
    /// Today it is written from the same fact as [`Self::scoped`] and always equals it. They are
    /// kept apart because they answer different questions — this one is a QUERY predicate that a
    /// future control could offer on an ordinary Report, while `scoped` is the ownership fact that
    /// decides persistence. Route a new scope entry point through both.
    closed_only: bool,
    /// Controlled multi-selection keyed by stable report row identity.
    selection: ReportSelection,
    needs_query: bool,
    query_inflight: bool,
    query_seq: u64,
    /// Start time of the last query, used to throttle writer-generation refreshes.
    ///
    /// The writer advances its generation after writes; without coalescing, a large database could
    /// be rescanned at the same high event frequency.
    last_query_start: Option<std::time::Instant>,
    /// Durable generation refresh state; only a rendered Report panel consumes its due edge.
    generation_refresh: query::GenerationRefreshGate,
    /// Time when periodic selector metadata was last published.
    last_metadata_at: Option<std::time::Instant>,
    /// Canonical non-strategy filter scope of the last successfully published strategy catalog.
    last_strategy_scope: Option<ReportFilter>,

    /// Visible columns by name rather than index, so runtime schema additions do not shift choices.
    /// A newly discovered column is visible only if its name is already present in the resolved
    /// default, `app_meta`, or per-context set.
    pub(super) visible: HashSet<String>,
    table_state: Entity<MoonDataTableState>,
    /// Versioned context-qualified width-storage ID for docked or detached Report layouts.
    widths_id: String,
    /// Content-derived base widths cached until report data, locale, or resolved typography changes.
    natural_widths: NaturalWidthsCache,
    /// Whether the panel is detached; docked tabs omit manual date fields for a compact filter row.
    detached: bool,
    /// Whether this panel owns the dedicated standalone Report tool window and its title bar.
    standalone: bool,
    // `table_state()` exposes the retained state to the detached window's automatic-width button.
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl crate::controls::CoreComboHost for ReportPanel {
    /// Group Auto owns the effective scope and leaves the retained selection untouched.
    fn core_selection_pinned(&self, cx: &App) -> bool {
        self.workspace_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_workspace_owned())
    }

    /// Return the retained Classic or standalone core filter for shared picker edits.
    fn core_selection_mut(&mut self) -> &mut HashSet<u64> {
        &mut self.sel_cores
    }

    /// Re-scope the strategy filter to the new core set, then requery.
    fn after_core_selection_change(&mut self, cx: &mut Context<Self>) {
        self.reconcile_strategy_core(cx);
        self.request_requery(cx);
    }
}

impl ReportPanel {

    /// The time axis every replicated report timestamp in this window is read through.
    ///
    /// Answers from the CACHED axis rather than rebuilding one: this is called from the render
    /// path and from every filter construction, and the value only moves when a core adopts a new
    /// offset or the user picks a different zone — both of which refresh the field directly. A
    /// core with no measurement contributes nothing, so it still reads exactly as stored, which is
    /// what reproduces MoonBot's own report for an unmeasured fleet.
    pub(super) fn report_axis(&self) -> ReportAxis {
        self.axis.clone()
    }

    /// The zone a period BOUND and a calendar window resolve in.
    ///
    /// A bound is compared against the raw replicated column, so it must land on the SAME axis as
    /// that column — never on the user's display zone independently, which is what made a picked
    /// day select a different day's rows. Day labels follow it for the same reason: a caption that
    /// disagreed with the window it selects is worse than either answer alone.
    pub(super) fn bound_zone(&self) -> Tz {
        self.report_axis().zone()
    }
    /// Resolve workspace scope for group-owned Report instances.
    ///
    /// The Analytics-owned standalone window deliberately returns `None` and keeps its exact
    /// `ReportScope`, including offline historical core identities.
    ///
    /// Args:
    ///     b: Backend snapshot containing workspace authority and live group membership.
    ///
    /// Returns:
    ///     Effective group scope, or `None` for an explicit standalone report.
    pub(super) fn workspace_scope(&self, b: &Backend) -> Option<EffectiveCoreScope> {
        if self.standalone {
            return None;
        }
        let retained: Vec<CoreId> = self.sel_cores.iter().copied().collect();
        let retained = if retained.is_empty() {
            RetainedCoreScope::All
        } else {
            RetainedCoreScope::Explicit(&retained)
        };
        Some(b.effective_workspace_scope(&self.group, retained))
    }

    /// Return whether this host's display lens suppresses the redundant core-name column.
    ///
    /// Args:
    ///     b: Backend snapshot containing the current group workspace scope.
    ///
    /// Returns:
    ///     `true` only for a group-owned `AutoCore` Report. Standalone, Classic, and Auto Overview
    ///     keep the raw saved `core_name` preference available.
    pub(super) fn hide_core_name_column(&self, b: &Backend) -> bool {
        self.workspace_scope(b)
            .is_some_and(|scope| scope.is_auto_core())
    }

    /// Return deterministic core IDs used by rows, totals, exports, menus, and metadata queries.
    ///
    /// Args:
    ///     b: Backend snapshot containing workspace authority.
    ///
    /// Returns:
    ///     Effective group IDs or the standalone report's sorted retained IDs.
    pub(super) fn effective_core_ids(&self, b: &Backend) -> Vec<CoreId> {
        if let Some(scope) = self.workspace_scope(b) {
            return scope.ids().to_vec();
        }
        let mut retained: Vec<CoreId> = self.sel_cores.iter().copied().collect();
        retained.sort_unstable();
        retained
    }
}
