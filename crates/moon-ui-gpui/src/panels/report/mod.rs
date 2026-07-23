//! Report panel ported from egui's `src/dock/report_view.rs`.
//!
//! It displays closed trades from the local SQLite database, with core, coin, side, date,
//! order-kind, and deleted-trades filters plus column selection above the table and exact period
//! totals below it. The
//! generic table supports every displayable database column and header-click sorting. A writer
//! generation counter in `Backend.reports` triggers throttled automatic refreshes.
//!
//! Responsibilities are split across this module for state, queries, and lifecycle; [`controls`]
//! for selectors and the column menu; [`columns`] for column, cell, and header formatting; and
//! [`export`] for file export.

mod columns;
mod controls;
mod export;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::MoonWindowExt as _;
use moon_ui::{
    DockArea, MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn,
    MoonDataTableState, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem,
    MoonMenuSize, MoonNotification, MoonPalette, MoonTone, Panel, PanelEvent, PanelState,
    StyledExt, h_flex, v_flex,
};
use rusqlite::Connection;
use rusqlite::types::Value;
use rust_i18n::t;

use crate::core_order::CoreOrder;
use crate::load_state::{LoadState, Note, note_el};
use crate::{Backend, design};
use moon_core::db::{self, ReadResult, ReportFilter, SideFilter};

/// Maximum number of report rows loaded for the current filter and sort order.
///
/// Period totals come from a separate [`db::query_totals`] aggregate over the complete filtered
/// data set, so they remain exact regardless of this limit. Writer generations can request repeated
/// background reads, coalesced by the panel's five-second throttle; keeping the row cap small avoids
/// repeatedly materializing very large result sets.
const MAX_REPORT_ROWS: usize = 100;

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
    "lev",
    "sellreason",
    "comment",
];

/// Complete a partially persisted width map with defaults for every current column.
///
/// Partial maps from older sessions or renamed columns caused the table engine to rescale untouched
/// neighbors on every drag. Completing a non-empty map lets later observations recognize a
/// single-column drag once the previous snapshot has the same membership. Leave an empty map
/// untouched so automatic fill remains active; the first drag then snapshots all widths and may use
/// the proportional overflow path.
fn complete_widths(widths: &mut std::collections::HashMap<String, f32>, cols: &[String]) {
    if widths.is_empty() {
        return;
    }
    for c in cols {
        widths
            .entry(c.clone())
            .or_insert_with(|| columns::width_for(c));
    }
}

/// Minimum column width, mirroring the floor the fork enforces in
/// `MoonDataTableState::set_column_width` (`data_table.rs`). The fork does not export it,
/// so it is duplicated here; the clamp math must use the same floor, or its "does this
/// layout fit" test would disagree with the widths the engine actually stores.
const MIN_COL_W: f32 = 40.0;

/// Pure width-budget policy, split out of [`ReportPanel::clamp_table_widths`] so its
/// no-infinite-loop invariant can be unit-tested without a gpui `App`.
///
/// Returns the width map to write (and notify on), or `None` when nothing should change:
/// the columns already fit, no fitting solution exists, or the computed widths do not
/// actually move. The `None` on an infeasible layout is what stops the observe→notify
/// loop: with a 40px floor per column (enforced here and by the fork's `set_column_width`),
/// a viewport narrower than `40px × visible` can never be satisfied, so writing and
/// notifying anyway would re-arm the observer on every pass — CPU pegged, window frozen.
fn plan_clamp(
    cur: &std::collections::HashMap<String, f32>,
    visible: &[String],
    prev: &std::collections::HashMap<String, f32>,
    viewport: f32,
) -> Option<std::collections::HashMap<String, f32>> {
    if cur.is_empty() || viewport <= 80.0 {
        return None;
    }
    let width_of = |c: &str| cur.get(c).copied().unwrap_or_else(|| columns::width_for(c));
    let sum: f32 = visible.iter().map(|c| width_of(c)).sum();
    if sum - viewport <= 0.5 {
        return None; // already within budget
    }
    // No packing fits at the floor. Leave the stored widths intact and do NOT notify: the
    // fork already fits the DISPLAY itself at render time — `downscale_columns_to_available`
    // shrinks columns down to the floor, and its horizontal scrollbar owns whatever still
    // overflows — all without touching `column_widths`. So there is nothing to store, and
    // notifying on a pass that cannot reduce the overflow would spin observe→clamp→notify.
    if visible.len() as f32 * MIN_COL_W > viewport {
        return None;
    }
    let overflow = sum - viewport;
    // One changed column at an unchanged key set = a live single-column drag.
    let changed: Vec<&str> = cur
        .iter()
        .filter(|(k, v)| prev.get(*k).is_none_or(|p| (**v - p).abs() > 0.5))
        .map(|(k, _)| k.as_str())
        .collect();
    let single_drag = changed.len() == 1 && prev.len() == cur.len();
    let mut next = cur.clone();
    let mut moved = false;
    if single_drag {
        if let Some(w) = next.get_mut(changed[0]) {
            let nw = (*w - overflow).max(MIN_COL_W);
            if (nw - *w).abs() > 0.5 {
                *w = nw;
                moved = true;
            }
        }
    } else {
        let scale = viewport / sum;
        for col in visible {
            if let Some(w) = next.get_mut(col) {
                let nw = (*w * scale).max(MIN_COL_W);
                if (nw - *w).abs() > 0.5 {
                    *w = nw;
                    moved = true;
                }
            }
        }
    }
    // Notify only on real movement; a no-op pass must not re-arm the observer.
    moved.then_some(next)
}

/// Rows and exact period totals from one completed report read.
///
/// The schema is stored separately so a completed failed read cannot collapse
/// column controls. Rows and totals are discarded because retaining them under
/// a changed filter would present stale figures as current.
pub(super) struct ReportData {
    pub(super) rows: Vec<Vec<Value>>,
    pub(super) core_uids: Vec<u64>,
    /// `newrecid` per row, parallel to `rows`; `0` for a legacy row that cannot be soft-deleted.
    /// The deletion-mode checkboxes read it to address rows in `set_report_rows_deleted`.
    pub(super) rec_ids: Vec<i64>,
    /// Exact `(profit sum, order count)` over the full filter, not the displayed top N.
    totals: (f64, i64),
}

/// One completed background batch: data, schema, and optional core refresh.
///
/// An empty `cores` vector is also used when the expensive core query is skipped.
struct ReportRead {
    cores: Vec<(u64, String)>,
    cols: Vec<String>,
    data: ReportData,
}

/// Read cores when requested, rows, and totals from one WAL snapshot.
///
/// `NotReady` means the reports replica is absent. `Failed` means opening the
/// connection, pinning the snapshot, probing schema, or running any query failed.
/// `with_cores` skips the expensive full-database grouping on most rounds.
fn run_report_query(
    filter: ReportFilter,
    sort_key: String,
    sort_desc: bool,
    with_cores: bool,
) -> ReadResult<ReportRead> {
    let started = std::time::Instant::now();
    let conn = db::open_reader()?;
    // Pin one snapshot across cores, rows, and totals. Separate autocommit reads
    // could straddle a writer commit, including legacy cleanup after sync, and
    // publish rows and totals from different database states.
    let snap = db::read_snapshot(&conn)?;
    let cores = if with_cores {
        db::distinct_cores(&snap)?
    } else {
        Vec::new()
    };
    let table = db::query_reports(&snap, &filter, &sort_key, sort_desc, MAX_REPORT_ROWS)?;
    let totals = db::query_totals(&snap, &filter)?;
    // Log measured query latency so slow refreshes are visible; the panel controls their frequency.
    let ms = started.elapsed().as_millis();
    if ms > 250 {
        log::warn!(
            "отчёты: медленный query {ms}ms (rows={} cores={} filter: dates={:?}/{:?})",
            table.rows.len(),
            with_cores,
            filter.date_from,
            filter.date_to,
        );
    } else {
        log::debug!("отчёты: query {ms}ms (rows={})", table.rows.len());
    }
    Ok(ReportRead {
        cores,
        cols: table.cols,
        data: ReportData {
            rows: table.rows,
            core_uids: table.core_uids,
            rec_ids: table.rec_ids,
            totals,
        },
    })
}

pub struct ReportPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    generation: Option<Arc<AtomicU64>>,
    last_gen: u64,

    conn: Option<Connection>,
    pub(super) cores: Vec<(u64, String)>,
    /// Cached schema, kept outside `data` so failures cannot collapse controls or widths.
    pub(super) cols: Rc<Vec<String>>,
    /// Report rows and totals; in-flight refreshes may retain stale data, but
    /// `NotReady` and `Failed` retain none.
    data: LoadState<ReportData>,

    sort_key: String,
    sort_desc: bool,

    /// Multi-selected core UIDs; an empty set means all cores.
    pub(super) sel_cores: HashSet<u64>,
    coin: Entity<MoonInputState>,
    /// Mirror of the coin input, updated on `Change`.
    ///
    /// This distinguishes manual edits, which open the match popup, from `on_pick` substitutions,
    /// which update the mirror first so their change event does not reopen the popup.
    coin_query: String,
    /// Whether the shared `controls::coin_search` match popup is open.
    coin_popup_open: bool,
    from: Entity<MoonInputState>,
    to: Entity<MoonInputState>,
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
    /// Deletion mode (detached window only): the `deleted` column's checkboxes become editable and
    /// the trash button turns into a Save. Toggled by [`Self::on_delete_button`].
    pub(super) delete_mode: bool,
    /// Uncommitted deletion-mode edits: `(core_uid, newrecid) -> desired deleted flag`, holding
    /// only rows whose desired state DIFFERS from the database, so a non-empty map means the Save
    /// is armed. Cleared on commit ([`Self::commit_deletions`]); kept across background refreshes
    /// so an in-flight edit is not lost when the writer generation bumps.
    pub(super) pending_deleted: std::collections::HashMap<(u64, i64), bool>,
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
    /// Time when the last core-list query was launched; its full-database grouping is requested at
    /// most once per minute.
    last_cores_at: Option<std::time::Instant>,

    /// Visible columns by name rather than index, so runtime schema additions do not shift choices.
    /// A newly discovered column is visible only if its name is already present in the resolved
    /// default, `app_meta`, or per-context set.
    pub(super) visible: HashSet<String>,
    table_state: Entity<MoonDataTableState>,
    /// Context-qualified width-storage ID, `report-table:dock` or `report-table:win`.
    widths_id: String,
    /// Whether the panel is detached; docked tabs omit manual date fields for a compact filter row.
    detached: bool,
    /// Width snapshot from the previous observation, used by [`Self::clamp_table_widths`] to detect
    /// the column currently being dragged.
    last_widths: std::collections::HashMap<String, f32>,
    // `table_state()` exposes the retained state to the detached window's automatic-width button.
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl ReportPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let generation = backend
            .read(cx)
            .reports
            .as_ref()
            .map(|h| h.generation.clone());
        // Keep this connection for panel metadata. Startup core/schema probes
        // are deliberately lossy because the fallible background batch below
        // owns user-visible read errors.
        let conn = db::open_reader().ok();
        let cores = conn
            .as_ref()
            .and_then(|c| db::distinct_cores(c).ok())
            .unwrap_or_default();
        let last_gen = generation
            .as_ref()
            .map(|g| g.load(Ordering::Relaxed))
            .unwrap_or(0);
        // Restore visible column names from `app_meta`, or use the defaults when never saved.
        let visible: HashSet<String> = conn
            .as_ref()
            .and_then(db::load_visible)
            .map(|saved| saved.into_iter().collect())
            .unwrap_or_else(|| DEFAULT_VISIBLE.iter().map(|c| c.to_string()).collect());
        // Probe the initial database schema so the first render and column menu are complete.
        let init_cols = conn
            .as_ref()
            .and_then(|c| db::display_columns(c).ok())
            .unwrap_or_default();
        let (sort_key, sort_desc) = conn
            .as_ref()
            .and_then(db::load_sort)
            .unwrap_or_else(|| ("buydate".to_string(), true));
        let widths_id = crate::persistence::table_persist::ctx_id("report-table", false);
        let mut saved_widths =
            crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        complete_widths(&mut saved_widths, &init_cols);
        let table_state = cx.new(|_| MoonDataTableState::new());
        table_state.update(cx, |state, _| {
            state.set_sort(sort_key.clone(), !sort_desc);
            state.column_widths = saved_widths;
        });
        // A column resize mutates table state; clamp visible widths to the viewport budget, then
        // persist them through the shared storage.
        cx.observe(&table_state, |this, state, cx| {
            this.clamp_table_widths(&state, cx);
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();

        let coin = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.coin_ph").to_string())
        });
        let from = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.date_ph").to_string())
        });
        let to = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.date_ph").to_string())
        });
        // Any coin-field change requests a query. Manual input also opens the match popup. An
        // `on_pick` substitution updates `coin_query` first, so either a synchronous or deferred
        // Change event sees the mirror already matched and does not reopen the popup.
        cx.subscribe(&coin, |t, e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = e.read(cx).value().to_string();
                if t.coin_query != value {
                    t.coin_query = value;
                    t.coin_popup_open = !t.coin_query.trim().is_empty();
                }
                t.request_requery(cx);
            }
        })
        .detach();
        // A non-empty manual date switches to All so a preset cannot silently take precedence.
        for st in [&from, &to] {
            cx.subscribe(st, |t, e, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    if !e.read(cx).value().trim().is_empty() {
                        t.period = Period::All;
                    }
                    t.request_requery(cx);
                }
            })
            .detach();
        }
        // Backend observation requests a refresh only when the report writer's generation changes.
        // Filter edits notify and query immediately; generation-driven refreshes use the throttle.
        cx.observe(&backend, |this, _b, cx| {
            if let Some(g) = &this.generation {
                let v = g.load(Ordering::Relaxed);
                if v != this.last_gen {
                    this.last_gen = v;
                    this.requery_on_generation(cx);
                }
            }
        })
        .detach();

        let mut this = Self {
            backend,
            group,
            generation,
            last_gen,
            conn,
            cores,
            cols: Rc::new(init_cols),
            data: LoadState::default(),
            sort_key,
            sort_desc,
            sel_cores: HashSet::new(),
            coin,
            coin_query: String::new(),
            coin_popup_open: false,
            from,
            to,
            side: SideFilter::All,
            period: Period::Today,
            kind: ReportKind::Real,
            deleted_only: false,
            delete_mode: false,
            pending_deleted: std::collections::HashMap::new(),
            needs_query: true,
            query_inflight: false,
            query_seq: 0,
            last_query_start: None,
            throttle_armed: false,
            last_cores_at: None,
            visible,
            table_state,
            widths_id,
            detached: false,
            last_widths: std::collections::HashMap::new(),
            dock: None,
            focus: cx.focus_handle(),
        };
        // A per-context shared-storage column set overrides the one loaded from `app_meta`. Without
        // one for this mode, the `app_meta` set remains as a migration seed.
        this.apply_ctx_columns(cx);
        this.schedule_requery(cx);
        this
    }

    /// Return retained table state for the detached window's automatic-width reset button.
    pub(crate) fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Reduce visible column widths toward the table viewport budget.
    ///
    /// The fork rescales columns at render to fit the viewport — `auto_width_columns` upscales to
    /// fill spare room, `downscale_columns_to_available` shrinks down to the floor — so neighbors
    /// "float" when one column is dragged; keeping the stored sum within the viewport stops that
    /// rescale from engaging. The decision is delegated to [`plan_clamp`], kept pure so
    /// its termination invariant is unit-testable. `None` means no write and no notify: this runs
    /// from an `observe` on the table state and its own `notify()` re-enters that observe, so a
    /// notify on a pass that made no progress (an infeasible `40px × visible > viewport` layout —
    /// e.g. every column shown in a narrow detached window) would spin `flush_effects` forever.
    fn clamp_table_widths(&mut self, state: &Entity<MoonDataTableState>, cx: &mut App) {
        let (cur, viewport) = {
            let s = state.read(cx);
            (s.column_widths.clone(), s.viewport_width())
        };
        let prev = std::mem::replace(&mut self.last_widths, cur.clone());
        let visible: Vec<String> = self
            .cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .cloned()
            .collect();
        if let Some(next) = plan_clamp(&cur, &visible, &prev, viewport) {
            state.update(cx, |s, c| {
                s.column_widths = next;
                c.notify();
            });
            self.last_widths = state.read(cx).column_widths.clone();
        }
    }

    /// Switch a newly created detached panel to the `:win` column-storage context.
    ///
    /// Reload widths and visible columns for detached mode so docked tabs and windows keep separate
    /// layouts, and enable the detached-only manual date controls.
    pub(crate) fn mark_table_detached(&mut self, cx: &mut Context<Self>) {
        self.detached = true;
        self.widths_id = crate::persistence::table_persist::ctx_id("report-table", true);
        let mut saved =
            crate::persistence::table_persist::saved(self.backend.read(cx), &self.widths_id);
        complete_widths(&mut saved, &self.cols);
        self.table_state.update(cx, |s, c| {
            s.column_widths = saved;
            c.notify();
        });
        self.apply_ctx_columns(cx);
        cx.notify();
    }

    /// Apply a saved per-context visible-column set for `widths_id`, when non-empty.
    ///
    /// Docked `:dock` and detached `:win` modes have distinct shared-storage entries. An empty set
    /// leaves the current selection intact rather than producing an empty table.
    fn apply_ctx_columns(&mut self, cx: &App) {
        if let Some(keys) =
            crate::persistence::table_persist::visible(self.backend.read(cx), &self.widths_id)
        {
            let set: HashSet<String> = keys.into_iter().collect();
            if !set.is_empty() {
                self.visible = set;
            }
        }
    }

    /// Save visible columns in runtime table order to per-context storage under `widths_id`.
    ///
    /// Called through [`Self::persist_visible`] after column-menu changes. An empty set is not
    /// written, so an older non-empty per-context entry remains available and can override the empty
    /// `app_meta` set when the panel is recreated.
    fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .map(|c| c.to_string())
            .collect();
        if !keys.is_empty() {
            crate::persistence::table_persist::set_visible(
                &self.backend,
                &self.widths_id,
                keys,
                cx,
            );
        }
    }

    /// Return visible columns in runtime-schema order for stable rendering and persistence.
    pub(super) fn visible_cols(&self) -> Vec<&str> {
        self.cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .map(|c| c.as_str())
            .collect()
    }

    /// Return whether every runtime column is enabled in the Columns menu.
    pub(super) fn all_columns_on(&self) -> bool {
        !self.cols.is_empty() && self.cols.iter().all(|c| self.visible.contains(c.as_str()))
    }

    /// Persist visible columns to `app_meta` and, when non-empty, the dock/window table descriptor.
    ///
    /// `app_meta` records an empty set, but [`Self::save_ctx_columns`] deliberately leaves any older
    /// per-context descriptor unchanged in that case.
    fn persist_visible(&self, cx: &mut App) {
        if let Some(conn) = &self.conn {
            db::save_visible(conn, &self.visible_cols());
        }
        self.save_ctx_columns(cx);
    }

    fn filter(&self, cx: &App) -> ReportFilter {
        // A preset overrides the manual date only on the edge it SETS itself. Every preset
        // but "All" sets the lower one; only "Yesterday" sets the upper one — for the rest
        // `to = None`, and then the upper edge comes from the "To:" field if it holds a date.
        let (pfrom, pto) = self.period.range();
        let date_from = pfrom.or_else(|| db::parse_ymd(&self.from.read(cx).value()));
        let date_to = pto.or_else(|| db::parse_ymd(&self.to.read(cx).value()).map(|d| d + 86_399));
        ReportFilter {
            core_uids: self.sel_cores.iter().copied().collect(),
            date_from,
            date_to,
            // Normalize Russian-layout keystrokes for the SQL filter as well as the search popup;
            // otherwise Cyrillic input would reach `coin LIKE` unchanged and return no matches.
            coin: crate::controls::coin_search::normalize_layout(&self.coin.read(cx).value())
                .into_owned(),
            side: self.side,
            emulator: self.kind.to_filter(),
            deleted_only: self.deleted_only,
        }
    }

    fn request_requery(&mut self, cx: &mut Context<Self>) {
        self.needs_query = true;
        self.schedule_requery(cx);
        cx.notify();
    }

    /// Request a writer-generation refresh at most once every five seconds.
    ///
    /// A trailing timer waits out the remaining interval so the final generation change is not
    /// lost. User filter edits bypass this throttle.
    fn requery_on_generation(&mut self, cx: &mut Context<Self>) {
        const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
        let since = self
            .last_query_start
            .map(|t| t.elapsed())
            .unwrap_or(MIN_INTERVAL);
        if since >= MIN_INTERVAL {
            self.request_requery(cx);
            return;
        }
        if self.throttle_armed {
            return;
        }
        self.throttle_armed = true;
        let wait = MIN_INTERVAL - since;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(wait).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.throttle_armed = false;
                    this.request_requery(cx);
                });
            });
        })
        .detach();
    }

    fn schedule_requery(&mut self, cx: &mut Context<Self>) {
        if !self.needs_query || self.query_inflight {
            return;
        }
        self.needs_query = false;
        self.query_inflight = true;
        self.query_seq = self.query_seq.wrapping_add(1);
        self.last_query_start = Some(std::time::Instant::now());
        // Keep stale rows while a refresh is in flight to avoid flicker. A
        // completed `NotReady` or `Failed` result discards them through `LoadState`.
        self.data.begin();
        // Refresh the core list at most once per minute because it groups across the full database.
        let with_cores = self
            .last_cores_at
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(60))
            .unwrap_or(true);
        if with_cores {
            self.last_cores_at = Some(std::time::Instant::now());
        }

        let request_id = self.query_seq;
        let filter = self.filter(cx);
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;

        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move { run_report_query(filter, sort_key, sort_desc, with_cores) })
                .await;

            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.query_seq != request_id {
                        return;
                    }
                    this.query_inflight = false;
                    if this.needs_query {
                        this.schedule_requery(cx);
                        return;
                    }

                    match result {
                        Ok(read) => {
                            // An empty core result never replaces the cached list;
                            // this also preserves it when the query is skipped.
                            if !read.cores.is_empty() {
                                this.cores = read.cores;
                            }
                            let cols_changed = *this.cols != read.cols;
                            this.cols = Rc::new(read.cols);
                            this.data.apply(Ok(read.data));
                            // Complete the width map when schema membership changes
                            // so new columns stay stable while neighbors resize. A
                            // double-click reset intentionally removes one entry.
                            if cols_changed {
                                let cols = this.cols.clone();
                                this.table_state.update(cx, |s, c| {
                                    if !s.column_widths.is_empty() {
                                        complete_widths(&mut s.column_widths, &cols);
                                        c.notify();
                                    }
                                });
                            }
                        }
                        // Preserve schema and core choices across a failed read;
                        // only rows and totals disappear so the failure can render.
                        Err(e) => this.data.apply(Err(e)),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Toggle a core in the multi-selection.
    ///
    /// `None` is the All toggle: it builds the current core set, then clears the selection when that
    /// set is non-empty and its cardinality matches the selection's cardinality. This check does not
    /// compare membership. Otherwise it replaces the selection with the current core set.
    pub(super) fn toggle_core(&mut self, uid: Option<u64>, cx: &mut Context<Self>) {
        match uid {
            None => {
                let all: HashSet<u64> = self.cores.iter().map(|(u, _)| *u).collect();
                if !all.is_empty() && self.sel_cores.len() == all.len() {
                    self.sel_cores.clear();
                } else {
                    self.sel_cores = all;
                }
            }
            Some(uid) => {
                if !self.sel_cores.remove(&uid) {
                    self.sel_cores.insert(uid);
                }
            }
        }
        self.request_requery(cx);
    }

    /// Set the filter to only the clicked Core-cell UID, or clear it when already the sole selection.
    ///
    /// This matches Core-cell filtering in Orders and Assets.
    pub(super) fn filter_to_core(&mut self, uid: u64, cx: &mut Context<Self>) {
        if self.sel_cores.len() == 1 && self.sel_cores.contains(&uid) {
            self.sel_cores.clear();
        } else {
            self.sel_cores = HashSet::from([uid]);
        }
        self.request_requery(cx);
    }

    /// Toggle all runtime columns on, or retain only the first when all are already visible.
    ///
    /// Keeping one column prevents the table from becoming entirely empty.
    pub(super) fn toggle_all_columns(&mut self, cx: &mut Context<Self>) {
        if self.all_columns_on() {
            self.visible = self.cols.first().cloned().into_iter().collect();
        } else {
            self.visible = self.cols.iter().cloned().collect();
        }
        self.persist_visible(cx);
        cx.notify();
    }
    pub(super) fn set_side(&mut self, s: SideFilter, cx: &mut Context<Self>) {
        if self.side != s {
            self.side = s;
            self.request_requery(cx);
        }
    }
    pub(super) fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {
        if self.period != p {
            self.period = p;
            self.request_requery(cx);
        }
    }
    /// Close the coin-match popup without changing the typed report filter.
    ///
    /// Unlike chart tabs, dismissing this popup leaves its text intact.
    pub(super) fn close_coin_popup(&mut self, cx: &mut Context<Self>) {
        if self.coin_popup_open {
            self.coin_popup_open = false;
            cx.notify();
        }
    }

    pub(super) fn set_kind(&mut self, k: ReportKind, cx: &mut Context<Self>) {
        if self.kind != k {
            self.kind = k;
            self.request_requery(cx);
        }
    }
    /// Toggle the deleted-trades checkbox: off hides soft-deleted trades, on shows only them.
    pub(super) fn set_deleted_only(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.deleted_only != on {
            self.deleted_only = on;
            self.request_requery(cx);
        }
    }

    /// Whether the deletion-mode Save is armed: in mode with at least one uncommitted edit.
    pub(super) fn delete_dirty(&self) -> bool {
        self.delete_mode && !self.pending_deleted.is_empty()
    }

    /// The trash button: enter deletion mode, or — once in it — Save the edits when any exist, else
    /// leave the mode. This one control is both the toggle and the commit, per the panel spec. The
    /// `deleted` checkbox column is force-shown while the mode is on (see the render), so entering
    /// needs no persisted column change.
    pub(super) fn on_delete_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.delete_mode {
            self.delete_mode = true;
            cx.notify();
        } else if self.pending_deleted.is_empty() {
            self.delete_mode = false;
            cx.notify();
        } else {
            self.commit_deletions(window, cx);
        }
    }

    /// Record one checkbox edit. Stores the desired flag only when it DIFFERS from the database
    /// value `orig`, so toggling a row back to its original state disarms it. Legacy rows
    /// (`rec == 0`) have no `newrecid` and cannot be soft-deleted, so they are ignored.
    pub(super) fn set_row_deleted(
        &mut self,
        core: u64,
        rec: i64,
        want: bool,
        orig: bool,
        cx: &mut Context<Self>,
    ) {
        if rec == 0 {
            return;
        }
        if want == orig {
            self.pending_deleted.remove(&(core, rec));
        } else {
            self.pending_deleted.insert((core, rec), want);
        }
        cx.notify();
    }

    /// Send the pending edits to their cores, grouped into one soft-delete and one restore batch
    /// per core. Edits whose core accepted the send are dropped and the mode is left; edits whose
    /// core rejected it (offline / removed from the session set) are KEPT so the amber Save stays
    /// armed and the intent is not silently lost, with a toast reporting the failure. The rows
    /// change on screen only when each core's `ReportEvent::RowsDeleted` echo lands and the replica
    /// refreshes — not optimistically.
    fn commit_deletions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // (to_delete, to_restore) newrecid lists per core.
        let mut per_core: std::collections::HashMap<u64, (Vec<i64>, Vec<i64>)> =
            std::collections::HashMap::new();
        for (&(core, rec), &want) in self.pending_deleted.iter() {
            let entry = per_core.entry(core).or_default();
            if want {
                entry.0.push(rec);
            } else {
                entry.1.push(rec);
            }
        }
        let mut failed: std::collections::HashSet<u64> = std::collections::HashSet::new();
        self.backend.update(cx, |b, _| {
            for (core, (del, res)) in per_core {
                for (deleted, recs) in [(true, del), (false, res)] {
                    if !recs.is_empty()
                        && b.session
                            .set_report_rows_deleted(core, deleted, Vec::new(), recs)
                            .is_err()
                    {
                        failed.insert(core);
                    }
                }
            }
        });
        // Keep only the edits whose core rejected the send; the rest reached their core.
        self.pending_deleted
            .retain(|(core, _), _| failed.contains(core));
        if failed.is_empty() {
            self.delete_mode = false;
        } else {
            window.push_notification(
                MoonNotification::error(t!("report.delete_send_failed").to_string()),
                cx,
            );
        }
        cx.notify();
    }
    /// Toggle a column by name and persist it using [`Self::persist_visible`].
    ///
    /// Hiding the last column writes an empty `app_meta` set but does not erase a prior per-context
    /// entry, which may restore that older non-empty set when the panel is recreated.
    pub(super) fn toggle_column(&mut self, name: String, cx: &mut Context<Self>) {
        if self.visible.contains(name.as_str()) {
            self.visible.remove(&name);
        } else {
            self.visible.insert(name);
        }
        self.persist_visible(cx);
        cx.notify();
    }
    /// Prompt for a destination, then export the current filter and sort order.
    ///
    /// `all_cols` selects the full runtime schema instead of visible columns. A
    /// `NotReady` or `Failed` read aborts before any file write; completion or
    /// failure is reported in the originating window.
    fn export_report(
        &mut self,
        fmt: export::Format,
        all_cols: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cols: Vec<String> = if all_cols {
            (*self.cols).clone()
        } else {
            self.cols
                .iter()
                .filter(|c| self.visible.contains(c.as_str()))
                .cloned()
                .collect()
        };
        if cols.is_empty() {
            log::warn!("отчёт: экспорт без колонок (таблица пуста?)");
            return;
        }
        let filter = self.filter(cx);
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;
        let suggested = export::suggested_name(&filter, fmt, all_cols);
        let handle = window.window_handle();
        let rx = cx.prompt_for_new_path(&export::default_dir(), Some(&suggested));
        cx.spawn(async move |_this, cx| {
            // A cancelled or closed destination dialog requires no notification.
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    export::run(&path, fmt, &cols, &filter, &sort_key, sort_desc).map(|n| (n, path))
                })
                .await;
            let note = match result {
                Ok((n, path)) => {
                    log::info!("отчёт: экспортировано {n} строк → {}", path.display());
                    MoonNotification::success(t!("report.export.ok", n = n).to_string())
                }
                Err(e) => {
                    log::error!("отчёт: экспорт не выполнен: {e:#}");
                    // Do not auto-hide: the user must notice that the requested
                    // export did not complete.
                    MoonNotification::error(format!("{e}"))
                        .title(t!("report.export.fail").to_string())
                        .autohide(false)
                }
            };
            let _ = cx.update(|app| {
                let _ = handle.update(app, |_, window, app| {
                    use moon_ui::MoonWindowExt as _;
                    window.push_notification(note, app);
                });
            });
        })
        .detach();
    }

    /// Render the report table from currently renderable data.
    ///
    /// `vis` indexes [`Self::cols`]. Empty rows keep the header and show the
    /// panel overlay. Loading with stale data may reach this function; loading
    /// without data, `NotReady`, and `Failed` are replaced by `note_el`.
    fn table_el(
        &self,
        data: Arc<ReportData>,
        vis: &[usize],
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible = Rc::new(vis.to_vec());
        let row_count = data.rows.len();
        let view = cx.entity();
        // Keep a separate handle for row click callbacks; sorting also captures
        // the original entity handle.
        let view_row = view.clone();
        let backend = self.backend.clone();
        let table_state = self.table_state.clone();
        let row_cols = self.cols.clone();
        let cols = columns::report_columns(&self.cols, vis);
        // Deletion-mode inputs for the `deleted` checkbox cell, snapshotted for the row builder.
        let delete_mode = self.delete_mode;
        let pending = Rc::new(self.pending_deleted.clone());
        // With no explicit filter, the coin menu may act on every core currently
        // known to the report selector.
        let selected_cores: Rc<Vec<u64>> = Rc::new(if self.sel_cores.is_empty() {
            self.cores.iter().map(|(u, _)| *u).collect()
        } else {
            self.sel_cores.iter().copied().collect()
        });
        // Clip resized columns at the shared table host; an empty successful
        // result keeps the header and uses the panel overlay.
        crate::panels::common::data_table_host(
            "rep-table-host",
            row_count == 0,
            t!("report.empty").to_string(),
            p,
            cx,
            MoonDataTable::new("report-table", row_count, move |ri, _window, _app| {
                columns::report_data_row(
                    ri,
                    &row_cols,
                    &data,
                    &visible,
                    &selected_cores,
                    &backend,
                    &view_row,
                    delete_mode,
                    &pending,
                    p,
                )
            })
            .state(&table_state)
            .columns(cols)
            .header_height(design::TABLE_HEAD_H)
            .row_height(design::TABLE_ROW_H)
            .on_sort(move |key, ascending, _window, app| {
                let key = key.to_string();
                view.update(app, |t, cx| t.set_report_sort(&key, !ascending, cx));
            }),
        )
        .into_any_element()
    }

    fn set_report_sort(&mut self, col: &str, sort_desc: bool, cx: &mut Context<Self>) {
        if self.sort_key == col && self.sort_desc == sort_desc {
            return;
        }
        self.sort_key = col.to_string();
        self.sort_desc = sort_desc;
        self.table_state.update(cx, |state, _| {
            state.set_sort(col.to_string(), !sort_desc);
        });
        if let Some(conn) = &self.conn {
            db::save_sort(conn, &self.sort_key, self.sort_desc);
        }
        self.request_requery(cx);
    }
}

impl EventEmitter<PanelEvent> for ReportPanel {}
impl Focusable for ReportPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ReportPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn panel_name(&self) -> &'static str {
        "Report"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("Report", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![
            crate::persistence::table_persist::reset_button(
                "report-reset-widths",
                &self.table_state,
            ),
            crate::panels::detach_button(
                "Report",
                self.group.clone(),
                self.backend.clone(),
                self.dock.clone(),
            ),
        ])
    }
}

impl Render for ReportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let border = rgb(p.border);

        // Coin field using the shared chart-style search widget, such as `BTC - Bybit1`.
        // Unlike chart search, selecting a result does not open that market. It inserts the base
        // token into the textual report filter, backed by SQL `coin LIKE`, and requests a refresh.
        // Partial manual input still works; the popup is only a suggestion layer.
        let coin_popup = self.coin_popup_open.then(|| {
            let results = {
                let b = self.backend.read(cx);
                crate::controls::coin_search::search(
                    b,
                    &self.group,
                    None,
                    &self.coin.read(cx).value(),
                )
            };
            let view = cx.entity();
            let coin_input = self.coin.clone();
            crate::controls::coin_search::render_popup(
                "rep-coin-search",
                results,
                &HashSet::new(),
                false,
                p,
                cx,
                move |_core, market, window, app| {
                    // The filter is textual, so use the market base, for example `BTCUSDT` to `BTC`.
                    // Core is selected separately. Update the mirror before `set_value` so the
                    // resulting Change event does not reopen the popup.
                    let base = moon_core::symbol::coin_of_market(&market).to_string();
                    view.update(app, |this, cx| {
                        this.coin_query = base.clone();
                        this.coin_popup_open = false;
                        cx.notify();
                    });
                    coin_input.update(app, |inp, c| inp.set_value(base.clone(), window, c));
                    view.update(app, |this, cx| this.request_requery(cx));
                },
                |_core, _market, _app| {},
                |_app| {},
            )
            .absolute()
            .top_full()
            .left_0()
            .mt(px(2.0))
        });
        // Catch clicks outside the popup to dismiss it. This layer sits below the filters in z-order,
        // so result rows handle their own clicks while clicks elsewhere close the list.
        let coin_dismiss = self.coin_popup_open.then(|| {
            div()
                .id("rep-coin-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                        this.close_coin_popup(cx);
                        cx.stop_propagation();
                    }),
                )
        });
        let coin_field = div()
            .relative()
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("rep-coin")
                        .state(&self.coin)
                        .small()
                        .cleanable(true),
                ),
            )
            .children(coin_popup);

        // Filters.
        let filters = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.core_combo(cx))
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.filter.coin").to_string()),
            )
            .child(coin_field)
            .child(self.side_combo(cx))
            .child(self.period_combo(cx))
            .child(self.kind_combo(cx))
            // Show manual From/To dates only in detached windows. Docked tabs rely on period presets
            // to keep the core, coin, side, period, order-kind, deleted, and column controls compact.
            .when(self.detached, |f| {
                f.child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("report.filter.from").to_string()),
                )
                .child(
                    div()
                        .w(px(110.0))
                        .child(MoonInput::new("rep-from").state(&self.from).small()),
                )
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("report.filter.to").to_string()),
                )
                .child(
                    div()
                        .w(px(110.0))
                        .child(MoonInput::new("rep-to").state(&self.to).small()),
                )
            })
            .child(self.deleted_check(cx))
            // Deletion mode is a detached-window-only action (the compact docked filter row has no
            // space for per-row checkboxes), and only meaningful when the schema has a `deleted`
            // column to edit — otherwise the button would be a dead control.
            .when(
                self.detached && self.cols.iter().any(|c| c == "deleted"),
                |f| f.child(self.delete_mode_button(cx)),
            )
            .child(self.export_menu(cx))
            .child(self.columns_menu(cx));

        // Table. Deletion mode force-shows the `deleted` checkbox column without persisting it, so
        // the mode is self-contained and leaves the saved column set untouched on exit.
        let vis: Vec<usize> = self
            .cols
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                self.visible.contains(c.as_str()) || (self.delete_mode && c.as_str() == "deleted")
            })
            .map(|(i, _)| i)
            .collect();
        // Resolve `LoadState` before inspecting schema or visibility. A failed
        // or not-ready read can coexist with an empty cached schema and must show
        // its database note, not the user-preference message for hidden columns.
        // Loaded empty rows keep the table's header and overlay, so this view
        // deliberately never classifies `Note::Empty`.
        let table_el: AnyElement = match self.data.view(|_| false) {
            Err(note) => note_el("rep-table-note", note, 12.0, p, cx),
            // A successful read with no report columns means the core schema is
            // not available yet.
            Ok(_) if self.cols.is_empty() => note_el("rep-table-note", Note::NotReady, 12.0, p, cx),
            // Columns exist, so an empty visible set is a user preference.
            Ok(_) if vis.is_empty() => div()
                .p_3()
                .text_color(rgb(p.text_soft))
                .child(t!("report.all_cols_hidden").to_string())
                .into_any_element(),
            Ok(data) => self.table_el(data.clone(), &vis, p, cx),
        };

        // Exact totals over the full filtered period.
        // Without current data, never render +0.00 / 0 orders: those values are
        // indistinguishable from a genuinely empty period.
        let mut totals = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.totals").to_string()),
            );
        totals = match self.data.data() {
            Some(d) => {
                let (sum, count) = d.totals;
                let sum_col = if sum > 0.0 {
                    p.green
                } else if sum < 0.0 {
                    p.red
                } else {
                    p.text_soft
                };
                totals
                    .child(
                        // The total is in the pair's quote currency, usually USDT,
                        // so use two decimals and a neutral dollar marker.
                        div()
                            .font_bold()
                            .text_color(rgb(sum_col))
                            .child(format!("{sum:+.2} $")),
                    )
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_soft))
                            .child(t!("report.orders_count", count = count).to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_end()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_soft))
                            .child(t!("report.shown_top", n = d.rows.len()).to_string()),
                    )
            }
            None => {
                let failed = matches!(self.data, LoadState::Failed(_));
                let (text, color) = if failed {
                    (t!("common.db_read_failed_short").to_string(), p.orange)
                } else {
                    ("—".to_string(), p.text_soft)
                };
                totals.child(div().font_bold().text_color(rgb(color)).child(text))
            }
        };

        v_flex()
            .id("report-panel")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            // Set monospace on the panel root, as in Orders, Assets, Analytics, and Screener.
            // Detached panels do not inherit it from the shell header; without this, Report would
            // render in Inter and diverge from the docked view and monospace-based core-selector
            // width measurement.
            .font_family(design::mono())
            .bg(rgb(p.table_body))
            // Keep the popup dismiss layer below the filters in z-order; see `coin_dismiss`.
            .children(coin_dismiss)
            .child(filters)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(table_el)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(totals)
    }
}
