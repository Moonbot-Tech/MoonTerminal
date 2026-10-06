//! Independent, automatically refreshed desktop Profit Monitor window.

/// Profit Monitor actions.
mod actions;
/// Profit Monitor body views.
mod body_views;
/// Profit Monitor context.
mod context;
/// Profit Monitor header.
mod header;
/// Profit Monitor reload.
mod reload;
/// Profit Monitor render.
mod render;

use body_views::*;
use context::*;
use header::*;

mod broadcast;
mod format;
mod line;
mod model;
mod rows;
mod sections;
mod settings;
mod table;
mod window;

// The window's own lifecycle lives in `window.rs`; the toolbar and startup reach it through here,
// so the module path callers use does not change when the file it lives in does.
pub(crate) use window::{ProfitMonitorOpenRequest, open, restore};

#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::analytics::{
    PreviousPeriodBasis, ProfitMonitorCore, ProfitMonitorCurrency, ProfitMonitorSummary, Query,
};
use moon_core::db::valuation::ValuationMode;
use moon_core::db::{FailKind, ProfitMetric, ProfitUnit, ReadFail, SideFilter};
use moon_core::session::CoreId;
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, MoonVirtualListScrollHandle, MoonWindowFrame, h_flex, v_flex,
};
use rust_i18n::t;

use super::ProfitLoadState;
use super::refresh::{
    BusyRetryBudget, RefreshGate, RefreshPlan, RefreshUrgency, report_result_is_stale,
};
use crate::controls::core_run::{RunSlots, run_scope_rev};
use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use format::{ColumnFloor, ProfitColumn};
use model::{
    ContextChange, MonitorLayout, MonitorPeriod, MonitorSort, MonitorSortColumn, context_change,
    duration_until_period_refresh, duration_until_wall_clock_boundary, next_sort,
    retain_last_known_venues, scoped_query_core_ids, sort_rows,
};
use moon_core::session::core_order::CoreOrder;
use rows::{GroupMode, LiveContext, MonitorRow, RowLabels, fold_total, grouped_rows};
use sections::{MonitorEntry, SectionLabels};
use settings::MonitorPrefs;
use table::{centered_alert, centered_message};

/// State of the independent Profit Monitor window.
pub(crate) struct ProfitMonitorView {
    backend: Entity<Backend>,
    /// Exact native identity used so an old release cannot clear a replacement singleton.
    window_id: WindowId,
    /// Cancellation authority for this window's current background taskbar-hide burst.
    taskbar_hide: crate::window::windowing::TaskbarHideTask,
    clock: Entity<MonitorClockView>,
    content: Entity<ProfitMonitorBodyView>,
    report_generation: Option<Arc<AtomicU64>>,
    valuation_generation: Option<Arc<AtomicU64>>,
    refresh: RefreshGate,
    busy_retries: BusyRetryBudget,
    clock_timer_generation: u64,
    db_active: bool,
    seq: u64,
    period: MonitorPeriod,
    zone: Tz,
    group: GroupMode,
    sort: Option<MonitorSort>,
    prefs: MonitorPrefs,
    settings_open: bool,
    valuation: ValuationMode,
    live: LiveContext,
    data: ProfitLoadState<ProfitMonitorSummary>,
    /// Native partitions published atomically with a split-currency snapshot.
    currencies: Vec<ProfitMonitorCurrency>,
    /// Cores the last SUCCESSFUL read actually named, kept apart from [`Self::data`].
    ///
    /// [`super::model::scoped_query_core_ids`] needs the previous read's core list to keep a
    /// data-only core — one absent from `config.servers`, which membership therefore has no
    /// authority to hide — inside a scoped query. Reading that list from `data` cannot work:
    /// `reload` resets `data` to `Loading` BEFORE building the replacement query, and
    /// `ProfitLoadState::data` also answers `None` for `Split` and `Failed`. Either way the
    /// carry-forward would arrive empty exactly when it is needed, the narrowed result would
    /// become the next read's universe, and the core's money would be gone for good rather than
    /// for one cycle.
    ///
    /// The sibling `analytics::toolbar::analytics_core_filter_ids` does not need this because its
    /// universe comes from `db::distinct_cores`, independent of any view state. This field is that
    /// independence, held locally: written only when a read applies, never cleared by `reload`.
    seen_data_cores: Vec<CoreId>,
    refresh_error: Option<ReadFail>,
    /// Run-state token of every configured core, folded with the pending-intent register.
    ///
    /// The monitor observes `Backend` for this and nothing else, so the throttled backend
    /// notification — which fires while the fleet is merely trading — costs one store lookup per
    /// configured core and repaints only when a run cell would actually draw differently. Zero
    /// while the run column is switched off, which is also when the fold is skipped entirely.
    run_rev: u64,
    /// Newest close date and trade count already on screen, per report core.
    ///
    /// This is the arrival detector's whole memory. `None` means "no baseline": the next snapshot
    /// only records, because a query change replaces every value at once and that is not fourteen
    /// new trades. Once a baseline exists, a core APPEARING is an arrival too — that is a core's
    /// first trade of the hour, the one a user is most likely watching for.
    ///
    /// The count is carried beside the date because close dates have one-second resolution: a
    /// second trade inside the same second moves the count and nothing else.
    seen_trades: Option<HashMap<CoreId, (i64, i64)>>,
    /// When each core's latest arrival was observed.
    ///
    /// The shared [`crate::pulse::Arrivals`] owns the stamps, their expiry and the "a timer is
    /// running" flag — the same machine the News feed uses, so the two highlights cannot drift.
    flash: crate::pulse::Arrivals<CoreId>,
    scroll: MoonVirtualListScrollHandle,
    /// Widest the profit column has been measured within the current period.
    ///
    /// The column is sized from the visible snapshot, so an ordinary refresh that shortens the
    /// longest amount by one digit would otherwise pull every name in the table sideways. This
    /// makes the width a RATCHET: it grows with the data and is released only when the thing being
    /// measured changes — the period, the grouping, a display preference, or the currency itself.
    /// Interior mutability because the body renders through `&self`, and the measurement is the
    /// only state that render itself produces.
    profit_width: Cell<ColumnFloor>,
    focus: FocusHandle,
}

impl ProfitMonitorView {
    /// Construct the monitor, arm taskbar suppression, and start its subscriptions and first read.
    ///
    /// Args:
    ///     backend: Shared terminal state.
    ///     window: Newly opened independent window.
    ///     cx: View context used for subscriptions and background work.
    ///
    /// Returns:
    ///     Fully initialized monitor state.
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_id = window.window_handle().window_id();
        // Apply the shared independent-window taskbar policy now and after every activation;
        // `hide_window_from_taskbar_soon` owns the delayed retry rationale.
        let taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window);
        cx.observe_window_activation(window, |this, window, _cx| {
            this.taskbar_hide.cancel();
            this.taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window);
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, cx| {
            let geom = crate::window::windowing::window_geom_rect(window, cx);
            this.backend.update(cx, |backend, _| {
                let geom = geom.keeping_display_of(backend.layout.profit_monitor_window);
                if backend.layout.profit_monitor_window != Some(geom) {
                    backend.layout.profit_monitor_window = Some(geom);
                    backend.layout_dirty = true;
                }
            });
        })
        .detach();

        // Closing the monitor by hand is a decision the next launch has to know about; quitting is
        // not. `quitting` separates them, exactly as the detached-panel windows do — during
        // shutdown the layout has already been flushed, so a release-time write here would replace
        // "the monitor was open" with "the monitor was closed" on every ordinary exit.
        // Deliberately NOT released here: the broadcast core filter outlives this window. Closing a
        // tool window must not silently change what five panels are showing, and the filter is not
        // invisible without the monitor — every panel that adopted it says "N cores" in its own
        // selector and can widen from there. The ⚙ checkbox is the one place that releases it,
        // because switching the feature off IS a request to stop filtering.
        cx.on_release(|this, app| {
            this.taskbar_hide.cancel();
            this.backend.update(app, |backend, cx| {
                // Everything here is guarded by the window id. A view released AFTER its
                // replacement registered — close and reopen inside one effect flush — would
                // otherwise clear the flag while a live monitor is on screen, and the next launch
                // would not reopen it.
                if backend
                    .profit_monitor_window
                    .is_none_or(|handle| handle.window_id() != this.window_id)
                {
                    return;
                }
                backend.profit_monitor_window = None;
                if !backend.quitting && backend.layout.profit_monitor_open {
                    backend.layout.profit_monitor_open = false;
                    backend.layout_dirty = true;
                }
                cx.notify();
            });
        })
        .detach();

        let clock = cx.new(|cx| MonitorClockView::new(backend.clone(), cx));
        let content_owner = cx.entity().downgrade();
        let content = cx.new(|_| ProfitMonitorBodyView {
            owner: content_owner,
        });
        let report_generation = backend
            .read(cx)
            .reports
            .as_ref()
            .map(|reports| reports.generation.clone());
        let valuation_generation = backend
            .read(cx)
            .valuation
            .as_ref()
            .map(|valuation| valuation.generation.clone());
        let generation = combined_generation(&report_generation, &valuation_generation);
        let period = backend
            .read(cx)
            .layout
            .profit_monitor_period
            .as_deref()
            .and_then(MonitorPeriod::from_id)
            .unwrap_or_default();
        let zone = moon_core::util::display_time::zone_or_utc(backend.read(cx).header_clock_zone());
        let group = backend
            .read(cx)
            .layout
            .profit_monitor_group
            .as_deref()
            .and_then(GroupMode::from_id)
            .unwrap_or_default();
        let sort = backend
            .read(cx)
            .layout
            .profit_monitor_sort
            .as_ref()
            .and_then(|(id, descending)| MonitorSort::from_layout(id, *descending));
        let prefs = MonitorPrefs::restore(&backend.read(cx).layout);
        // A preference carried over from a retired key is written back the moment it is read: the
        // key it came from has no writer in the current model, so leaving it unwritten would
        // re-apply the carry-over at every launch and undo the user's first edit to it.
        backend.update(cx, |backend, _| {
            if prefs.persist_migration(&mut backend.layout) {
                backend.layout_dirty = true;
            }
        });
        let valuation = backend.read(cx).valuation_mode();
        let live = capture_live_context(backend.read(cx));

        // The ONE Backend observation this window makes. Everything else it draws comes from the
        // report database or from the five-second context sample; the run controls are the only
        // part that has to follow live core state, and they are usually switched off.
        cx.observe(&backend, |this, _backend, cx| this.sync_run_state(cx))
            .detach();
        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _, cx| {
            this.observe_report_generation(cx);
        })
        .detach();
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _, cx| this.sync_context(cx))
            .detach();
        // A membership-only save advances `workspace_revision` without notifying `Backend`, so the
        // sync_run_state observation above never fires for it; observe it directly so a hidden core
        // and its money leave the table without waiting on the five-second context sampler.
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _, cx| this.sync_context(cx))
            .detach();
        broadcast::observe_core_filter(&backend, cx);
        let mut this = Self {
            backend,
            window_id,
            taskbar_hide,
            clock,
            content,
            report_generation,
            valuation_generation,
            refresh: RefreshGate::new(generation, std::time::Instant::now()),
            busy_retries: BusyRetryBudget::default(),
            clock_timer_generation: 0,
            db_active: false,
            seq: 0,
            period,
            zone,
            group,
            sort,
            prefs,
            settings_open: false,
            valuation,
            live,
            data: ProfitLoadState::default(),
            currencies: Vec::new(),
            seen_data_cores: Vec::new(),
            refresh_error: None,
            // Left at zero: the first backend notification fills it, and `sync_run_state` already
            // owns the "skip while the column is off" rule.
            run_rev: 0,
            seen_trades: None,
            flash: crate::pulse::Arrivals::default(),
            scroll: MoonVirtualListScrollHandle::new(),
            profit_width: Cell::default(),
            focus: cx.focus_handle(),
        };
        // Decode the logos before the first table frame needs them, off the render path — and only
        // when they will actually be drawn: someone who turned the icons off should not pay for
        // seven SVG rasters and the textures they retain for the rest of the session.
        if prefs.exchange_icons {
            cx.background_spawn(async { crate::media::exchange_logos::prewarm() })
                .detach();
        }
        this.reload(false, cx);
        this.start_clock_refresh(cx);
        this.start_context_refresh(cx);
        this
    }
}
