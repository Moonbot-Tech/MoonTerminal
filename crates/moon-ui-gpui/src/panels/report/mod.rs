//! Report panel ported from egui's `src/dock/report_view.rs`.
//!
//! It displays closed trades from the local SQLite database, with core, coin, strategy and date
//! filters, one merged scope field (side, order kind, deleted trades, comment pane) plus column
//! selection above the table, the current row's comment and exact period totals below it. The
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
    self, ReadResult, ReportFilter, ReportStrategy, ReportStrategyKey, SideFilter,
};
use moon_core::session::CoreId;

pub use window::open_scoped;

/// Moonbot-style report period presets with selected-zone civil-day boundaries.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    All,
    Today,
    Yesterday,
    Week,
    /// The calendar month from the 1st to today, unlike `Month`, which counts 30
    /// days back regardless of where the month boundary falls.
    CurMonth,
    Month,
    Year,
}

impl Period {
    pub(super) const ALL: [Self; 7] = [
        Self::All,
        Self::Today,
        Self::Yesterday,
        Self::Week,
        Self::CurMonth,
        Self::Month,
        Self::Year,
    ];

    pub(super) fn label(self) -> String {
        match self {
            Self::All => t!("report.filter.all"),
            Self::Today => t!("report.period.today"),
            Self::Yesterday => t!("report.period.yesterday"),
            Self::Week => t!("report.period.week"),
            Self::CurMonth => t!("report.period.cur_month"),
            Self::Month => t!("report.period.month"),
            Self::Year => t!("report.period.year"),
        }
        .to_string()
    }

    /// Return inclusive `(from, to)` Unix-second bounds in one display time zone.
    ///
    /// Args:
    ///     zone: User-selected display time zone.
    ///
    /// Returns:
    ///     Inclusive absolute bounds; `None` leaves that edge unbounded.
    fn range(self, zone: Tz) -> (Option<i64>, Option<i64>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.range_at(now, zone)
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
        match self {
            Self::All => (None, None),
            Self::Today => (day, None),
            Self::Yesterday => (shifted_start(-1), day.map(|value| value - 1)),
            Self::Week => (shifted_start(-6), None),
            Self::CurMonth => {
                let first = today.with_day(1).and_then(day_start).or(day);
                (first, None)
            }
            // 30 days back including today (like Week = 7, Year = 365) is a rolling window,
            // not the calendar month: that one is CurMonth.
            Self::Month => (shifted_start(-29), None),
            Self::Year => (shifted_start(-364), None),
        }
    }
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
    pub(super) side: SideFilter,
    /// Period preset, defaulting to Today as in Moonbot.
    ///
    /// Editing a non-empty manual date switches to All. Under other presets, the preset lower bound
    /// overrides From, while only Yesterday supplies an upper bound that overrides To.
    pub(super) period: Period,
    /// All, real, or emulated order kind, defaulting to real as in Orders.
    pub(super) kind: ReportKind,
    /// Show ONLY soft-deleted trades when set; hide them when clear (the default).
    pub(super) deleted_only: bool,
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
    /// Durable generation refresh state; only an active-window render consumes its due edge.
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

impl ReportPanel {
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
