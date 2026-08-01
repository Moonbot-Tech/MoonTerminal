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
//! [`export`] for file export.

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
mod widths;
mod window;

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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::MoonWindowExt as _;
use moon_ui::{
    DockArea, MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonCombobox, MoonComboboxEvent, MoonComboboxMenuChrome, MoonComboboxState,
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDataTableState,
    MoonDataTableWidthPolicy, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState,
    MoonMenuItem, MoonMenuSize, MoonNotification, MoonPalette, MoonScrollbarVisibility, MoonTone,
    MoonWindowFrame, Panel, PanelEvent, PanelState, Root, StyledExt, h_flex, rgba_from, v_flex,
};
use rusqlite::Connection;
use rusqlite::types::Value;
use rust_i18n::t;

use crate::core_order::CoreOrder;
use crate::load_state::{LoadState, Note, note_el};
use crate::{Backend, design};
use moon_core::db::{
    self, ReadResult, ReportFilter, ReportStrategy, ReportStrategyKey, SideFilter,
};

pub use window::open_scoped;

/// Moonbot-style report period presets with UTC-day boundaries.
///
/// Database and table dates are also displayed in UTC, keeping filters aligned with visible values.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    All,
    Today,
    Yesterday,
    Week,
    /// The calendar month from the 1st to today (UTC) — unlike `Month`, which counts 30
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

    /// Return inclusive `(from, to)` Unix-second bounds; `None` leaves that edge unbounded.
    fn range(self) -> (Option<i64>, Option<i64>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let day = now - now.rem_euclid(86_400);
        match self {
            Self::All => (None, None),
            Self::Today => (Some(day), None),
            Self::Yesterday => (Some(day - 86_400), Some(day - 1)),
            Self::Week => (Some(day - 6 * 86_400), None),
            Self::CurMonth => {
                // Midnight of the 1st of the current month, UTC — derived exactly as the
                // Analytics period bar does it (`analytics::period`): "YYYY-MM" from the DB
                // formatter + "-01", no calendar arithmetic of our own. Unparseable → today.
                let ym = moon_core::db::fmt_unix(now);
                let start = moon_core::db::parse_ymd(&format!("{}-01", &ym[..7.min(ym.len())]))
                    .unwrap_or(day);
                (Some(start), None)
            }
            // 30 days back including today (like Week = 7, Year = 365) — a ROLLING window,
            // not the calendar month: that one is CurMonth.
            Self::Month => (Some(day - 29 * 86_400), None),
            Self::Year => (Some(day - 364 * 86_400), None),
        }
    }
}

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
    "profitpct",
    "lev",
    "sellreason",
    "comment",
];

/// Closed-trade report surface shared by docked, detached, and scoped standalone hosts.
pub struct ReportPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    generation: Option<Arc<AtomicU64>>,
    last_gen: u64,

    conn: Option<Connection>,
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

    /// Multi-selected core UIDs; an empty set means all cores.
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
    from: Entity<MoonInputState>,
    /// Mirror of the From input, used to suppress duplicate scoped-update queries.
    from_query: String,
    to: Entity<MoonInputState>,
    /// Mirror of the To input, used to suppress duplicate scoped-update queries.
    to_query: String,
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
    /// default and persisted in `app_meta` per host — a docked tab and a detached window keep
    /// separate answers, exactly as their column sets and widths do. A display choice, not a filter.
    pub(super) show_comment: bool,
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
    /// Whether a trailing generation-refresh timer is already waiting out the throttle interval.
    throttle_armed: bool,
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
    /// Content-derived base widths cached until report data or font scale changes.
    natural_widths: NaturalWidthsCache,
    /// Whether the panel is detached; docked tabs omit manual date fields for a compact filter row.
    detached: bool,
    /// Whether this panel owns the dedicated standalone Report tool window and its title bar.
    standalone: bool,
    // `table_state()` exposes the retained state to the detached window's automatic-width button.
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}
