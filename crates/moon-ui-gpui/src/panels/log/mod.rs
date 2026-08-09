//! Log panel ported from egui's `src/dock/log_panel.rs`, with source and file selection, text and
//! coin filters, and an errors-only mode.
//!
//! Sources are live aggregates of all in-scope cores or one reported exchange, the local
//! application's `applog` ring, and each configured core's `CoreData.log` ring. Local and
//! single-core sources can show either Live or a rotated `logs/<date>_<source>.log` file; aggregate
//! sources are Live-only. `MoonVirtualList` virtualizes rows, and effective follow mode keeps
//! filtered output at the tail.
//!
//! A live source is read INCREMENTALLY: each line is parsed once, when it arrives, and appended to
//! a buffer that outlives the revision that brought it. Only a change the buffer cannot absorb — a
//! new source or file, an exchange losing a member, a core added, removed or renamed — rereads
//! everything.
//!
//! State, row collection, filtering, and lifecycle live here; source and file selectors are in
//! [`controls`]; the read cursors behind the incremental path in [`feed`]; rebuild signatures and
//! aggregation in [`render`]; one row's elements in [`row`]; the panel's own element tree in
//! [`view`]. Line classification, the row-range selection, the copy commands and the horizontal
//! viewport are shared with the Report's trade-log dialog and live in
//! [`crate::panels::line_list`].

mod buffer;
mod controls;
mod feed;
mod render;
mod row;
#[cfg(test)]
mod tests;
mod view;

use crate::panels::line_list::{self, RowSelection};
use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonScrollbarVisibility, MoonVirtualList, MoonVirtualListScrollHandle, Panel, PanelEvent,
    PanelState, StyledExt, h_flex, v_flex,
};

use rust_i18n::t;

use crate::Backend;
use crate::core_order::CoreOrder;
use crate::workspace::{EffectiveScopeLabel, RetainedCoreScope};
use moon_core::applog::{self, LogLine};
use moon_core::session::CoreId;
use std::collections::HashSet;

/// Maximum number of recent rows retained in a normal live or file snapshot.
const VIEW_LIMIT: usize = 5000;
/// Maximum number of rows taken from each core before building the aggregate.
const AGG_PER_CORE: usize = 2000;
/// Buffer cap while tail following is paused. Fresh rows accumulate beyond `VIEW_LIMIT` without
/// dropping the existing prefix, so the scroll position does not shift under the reader; returning
/// to effective follow mode trims back to `VIEW_LIMIT`.
const PAUSED_CAP: usize = 20_000;

/// Starts the revision-path stopwatch, but only when diagnostics are on.
fn diag_timer() -> Option<std::time::Instant> {
    crate::diag::timer()
}

/// Adds the elapsed time to `log_work_us`, which is what makes the panel's cost readable in
/// microseconds instead of inferred from process CPU shared with the charts.
fn record_work_us(timer: Option<std::time::Instant>) {
    crate::diag::record_us(&crate::diag::LOG_WORK_US, timer);
}

/// Selected source of log rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LogSource {
    Aggregate,
    Exchange(String),
    Local,
    Core(CoreId),
}

/// Return whether an exchange source's current membership requires replacing cached rows.
///
/// A non-exchange source has no exchange membership to invalidate. The first exchange snapshot and
/// every later membership change replace the buffer even while follow mode is paused.
///
/// Args:
///     previous: Membership recorded by the previous reload, if it was an exchange source.
///     current: Membership resolved for the current reload, if it is an exchange source.
///
/// Returns:
///     `true` when the current exchange membership is new or changed.
fn exchange_membership_changed(
    previous: Option<&HashSet<CoreId>>,
    current: Option<&HashSet<CoreId>>,
) -> bool {
    current.is_some() && previous != current
}

/// Whether to show live in-memory rows or a named rotated file from disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LogFile {
    Live,
    Named(String),
}

/// Resolve Auto's temporary source/file overlay without changing retained Classic state.
///
/// Args:
///     workspace_owned: Whether Auto currently owns this group panel's selectors.
///     workspace_core: Selected Auto core, or `None` for Auto Overview.
///     retained_source: User-selected Classic source.
///     retained_file: User-selected Classic file.
///
/// Returns:
///     Effective source, effective file, and the ownership flag used by controls.
fn resolve_workspace_log_selection(
    workspace_owned: bool,
    workspace_core: Option<CoreId>,
    retained_source: &LogSource,
    retained_file: &LogFile,
) -> (LogSource, LogFile, bool) {
    if workspace_owned {
        let source = workspace_core
            .map(LogSource::Core)
            .unwrap_or(LogSource::Aggregate);
        (source, LogFile::Live, true)
    } else {
        (retained_source.clone(), retained_file.clone(), false)
    }
}

/// One source-selector entry with its UI label and sanitized log-file label.
pub(super) struct LogSourceItem {
    pub(super) source: LogSource,
    pub(super) display: String,
    pub(super) file_label: String,
}

/// Stateful dock, detached-window, or group-window Log panel.
pub struct LogPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    pub(super) source: LogSource,
    pub(super) file: LogFile,
    /// Source and file that currently own the incremental buffer.
    loaded_selection: Option<(LogSource, LogFile)>,
    errors_only: bool,
    /// Coin substring filter set by clicking a detected ticker; `None` disables it.
    coin_filter: Option<String>,
    query: Entity<MoonInputState>,
    /// Named-file cache, avoiding disk reads during rendering and repeated backend observations.
    loaded_name: Option<String>,
    loaded_lines: Vec<LogLine>,
    /// Cached file list for the selected source; rendering never scans the filesystem for the menu.
    available_files_label: Option<String>,
    available_files: Vec<String>,
    /// Parsed rows for the current source and the filtered view over them.
    ///
    /// Live sources APPEND to this: each row is parsed once, when its line arrives, and then stays
    /// until the cap evicts it. Re-reading and re-parsing the whole source on every backend
    /// revision is what used to make an open Log tab cost ~25 ms four times a second regardless of
    /// how many rows the filters kept.
    buf: buffer::RowBuffer,
    /// Read positions in the live sources feeding the buffer.
    cursors: feed::LiveCursors,
    /// Exchange membership the buffer was filled from; a membership change invalidates it.
    exchange_membership: Option<HashSet<CoreId>>,
    /// Identity of the source list the buffer's rows were labelled from. See
    /// [`LogPanel::sources_sig`].
    sources_sig: u64,
    /// Rows selected for copying, as positions in the visible list.
    selection: RowSelection,
    /// User intent to follow the tail. Turning Live off manually prevents automatic resumption until
    /// the user enables it again.
    live: bool,
    /// Temporary follow pause set by wheel scrolling and cleared five seconds after the latest
    /// scroll. While true, filtering does not jump to the tail.
    scroll_pause: bool,
    /// Generation guarding delayed follow resumption. A new scroll or manual Live toggle invalidates
    /// earlier timers, so the five-second delay starts at the latest scroll.
    scroll_gen: u64,
    /// Whether a left button is held down inside the list, which defers follow resumption.
    ///
    /// Dragging a scrollbar produces no further events the pause timer could restart from — unlike
    /// the wheel, which re-arms on each notch — so a gesture longer than the five seconds would
    /// resume following mid-drag and jump the list to the tail under the hand holding it.
    press_held: bool,
    scroll: MoonVirtualListScrollHandle,
    /// Sideways offset of the row viewport, kept across frames so it survives every rebuild.
    hscroll: ScrollHandle,
    /// Visibility and selected-source revision gate for expensive row rebuilding.
    ///
    /// A hidden dock tab records no intermediate revisions; activation catches it up once.
    refresh: render::RefreshGate,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl LogPanel {
    /// Creates a deferred Log panel for `group`.
    ///
    /// Dock activity or the first detached-window render performs the initial load, so constructing
    /// a hidden default tab does not aggregate every core before the user opens it.
    ///
    /// Args:
    ///     backend: Shared state containing log sources and the selected display zone.
    ///     group: Optional core-group scope for this panel instance.
    ///     window: Window used to construct the search input.
    ///     cx: Panel context used to subscribe to data and display-zone revisions.
    ///
    /// Returns:
    ///     A deferred Log panel ready for its first visible reload.
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query =
            cx.new(|cx| MoonInputState::new(window, cx).placeholder(t!("log.search").to_string()));
        cx.subscribe(&query, |t, _e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                t.selection.clear();
                t.apply_filter(cx);
                cx.notify();
            }
        })
        .detach();
        // Reload only while visible and only when the effective live source's revision changes.
        cx.observe(&backend, |this, backend, cx| {
            let (source, file, _) = this.effective_selection(backend.read(cx));
            if !this.refresh.is_active() || !matches!(file, LogFile::Live) {
                return;
            }
            let sig = render::log_sig(backend.read(cx), &this.group, &source);
            if this.refresh.observe(sig) {
                cx.notify();
            }
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            let (source, file, _) = this.effective_selection(this.backend.read(cx));
            if this.loaded_selection.as_ref() == Some(&(source, file)) {
                return;
            }
            // A scope switch must never append into a buffer loaded for the prior effective pair.
            this.loaded_selection = None;
            if this.refresh.is_active() {
                let backend = this.backend.clone();
                this.reload_rows(backend.read(cx), cx);
                cx.notify();
            } else {
                this.refresh.request_reload();
            }
        })
        .detach();
        let display_zone =
            crate::chrome::clock::resolved_header_clock_zone(backend.read(cx).header_clock_zone());
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _revision, cx| {
            let zone = crate::chrome::clock::resolved_header_clock_zone(
                this.backend.read(cx).header_clock_zone(),
            );
            this.buf.rezone(zone);
            this.after_view_change();
            cx.notify();
        })
        .detach();
        Self {
            backend,
            group,
            source: LogSource::Aggregate,
            file: LogFile::Live,
            loaded_selection: None,
            errors_only: true,
            coin_filter: None,
            query,
            loaded_name: None,
            loaded_lines: Vec::new(),
            available_files_label: None,
            available_files: Vec::new(),
            buf: buffer::RowBuffer::new(display_zone),
            cursors: feed::LiveCursors::default(),
            exchange_membership: None,
            sources_sig: 0,
            selection: RowSelection::default(),
            live: true,
            scroll_pause: false,
            scroll_gen: 0,
            press_held: false,
            scroll: MoonVirtualListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
            refresh: render::RefreshGate::default(),
            dock: None,
            focus: cx.focus_handle(),
        }
    }

    /// Builds source-selector entries, ported from `App::build_log_sources`. A nonempty group limits
    /// configured cores and the aggregate to that group; an empty group includes every configured
    /// core. Aggregate and Local remain the first two entries, followed by canonically ordered cores.
    fn sources(&self, b: &Backend) -> Vec<LogSourceItem> {
        let scoped = !self.group.is_empty();
        let mut v = vec![
            LogSourceItem {
                source: LogSource::Aggregate,
                display: if scoped {
                    t!("log.source.group").to_string()
                } else {
                    t!("log.source.all").to_string()
                },
                file_label: String::new(),
            },
            LogSourceItem {
                source: LogSource::Local,
                display: t!("log.source.local").to_string(),
                file_label: "app".into(),
            },
        ];
        // Sort configured cores without changing membership; pseudo-items remain pinned above.
        let mut cores: Vec<(CoreId, String)> = b
            .config
            .servers
            .iter()
            .filter(|s| !scoped || s.group == self.group)
            .map(|s| (s.id, s.name.clone()))
            .collect();
        CoreOrder::new(&b.config).sort_by(&mut cores, |(id, _)| *id);
        for (id, name) in cores {
            v.push(LogSourceItem {
                source: LogSource::Core(id),
                file_label: applog::sanitize_label(&name),
                display: name,
            });
        }
        v
    }

    /// Resolve the non-mutating source and file that own this panel right now.
    ///
    /// Args:
    ///     b: Backend providing the current workspace mode and selected core.
    ///
    /// Returns:
    ///     Effective source, effective file, and whether Auto owns the selectors.
    fn effective_selection(&self, b: &Backend) -> (LogSource, LogFile, bool) {
        let scope = b.effective_workspace_scope(&self.group, RetainedCoreScope::All);
        let workspace_core = match scope.label() {
            EffectiveScopeLabel::Core(core) => Some(core),
            EffectiveScopeLabel::All
            | EffectiveScopeLabel::Selection(_)
            | EffectiveScopeLabel::Overview => None,
        };
        resolve_workspace_log_selection(
            scope.is_workspace_owned(),
            workspace_core,
            &self.source,
            &self.file,
        )
    }

    /// Limit data-facing source metadata to the effective Auto core.
    ///
    /// Args:
    ///     b: Backend providing the configured group core universe.
    ///     source: Effective source whose rows are being queried.
    ///     workspace_owned: Whether Auto currently owns the source selector.
    ///
    /// Returns:
    ///     Source metadata needed by the current query and its cache signature.
    fn data_sources(
        &self,
        b: &Backend,
        source: &LogSource,
        workspace_owned: bool,
    ) -> Vec<LogSourceItem> {
        let mut sources = self.sources(b);
        if workspace_owned && let LogSource::Core(core) = source {
            sources.retain(|item| matches!(item.source, LogSource::Core(id) if id == *core));
        }
        sources
    }

    /// Resolve the sanitized file label for one effective source.
    ///
    /// Args:
    ///     sources: Source metadata for the current data query.
    ///     source: Effective source whose rotated files are listed.
    ///
    /// Returns:
    ///     Sanitized source label, or the local application fallback.
    pub(super) fn file_label(sources: &[LogSourceItem], source: &LogSource) -> String {
        sources
            .iter()
            .find(|s| s.source == *source)
            .map(|s| s.file_label.clone())
            .unwrap_or_else(|| "app".into())
    }

    fn refresh_available_files(&mut self, label: &str) {
        if self.available_files_label.as_deref() == Some(label) {
            return;
        }
        self.available_files = applog::list_files(label);
        self.available_files_label = Some(label.to_string());
    }

    /// Reads the current selection in full, for the first load and after any change of source.
    ///
    /// Live reads the local ring, one core ring, every scoped core, or the selected exchange's
    /// current membership. Named reads and then caches at most `VIEW_LIMIT` rows from disk.
    ///
    /// Args:
    ///     b: Backend providing live core stores and file-independent source state.
    ///     source: Effective source to snapshot without changing the retained selector.
    ///     file: Effective live or named file selection.
    ///     sources: Canonically ordered source entries in the panel scope.
    ///     exchange_membership: Pre-resolved membership for an Exchange source.
    ///
    /// Returns:
    ///     The selected source's bounded live snapshot or cached named-file rows.
    fn snapshot(
        &mut self,
        b: &Backend,
        source: &LogSource,
        file: &LogFile,
        sources: &[LogSourceItem],
        exchange_membership: Option<&HashSet<CoreId>>,
    ) -> Vec<LogLine> {
        match file {
            LogFile::Live => {
                self.loaded_name = None;
                match source {
                    LogSource::Local => {
                        // Rows and cursor under ONE lock: feed threads push into this ring, so
                        // taking the cursor separately would mark a line that landed in between as
                        // already delivered and lose it for good.
                        let (lines, cursor) = applog::snapshot_with_cursor(VIEW_LIMIT);
                        self.cursors.set_local(cursor);
                        lines
                    }
                    LogSource::Core(id) => b
                        .session
                        .store()
                        .core(*id)
                        .map(|c| c.log_snapshot(VIEW_LIMIT))
                        .unwrap_or_default(),
                    LogSource::Aggregate => render::aggregate(b.session.store(), sources, None),
                    LogSource::Exchange(_) => render::aggregate(
                        b.session.store(),
                        sources,
                        Some(
                            exchange_membership
                                .expect("exchange source reload must resolve membership"),
                        ),
                    ),
                }
            }
            LogFile::Named(name) => {
                if self.loaded_name.as_deref() != Some(name.as_str()) {
                    self.loaded_lines = applog::read_file(name, VIEW_LIMIT);
                    self.loaded_name = Some(name.clone());
                }
                self.loaded_lines.clone()
            }
        }
    }

    /// Rows the buffer may hold before its oldest are dropped.
    ///
    /// Following the tail keeps exactly what a full snapshot would show. A paused panel keeps more
    /// so the rows under the user's eyes do not move, which is what `PAUSED_CAP` is for; resuming
    /// trims back down.
    fn cap(&self) -> usize {
        if self.following() {
            VIEW_LIMIT
        } else {
            PAUSED_CAP
        }
    }

    /// The filter terms one pass matches against, lowercased once here rather than per row.
    fn filters<'a>(&self, query: &'a str, coin: Option<&'a str>) -> buffer::Filters<'a> {
        buffer::Filters {
            errors_only: self.errors_only,
            query,
            coin,
        }
    }

    /// Runs `f` with the current filter terms, which have to outlive the borrow they are read into.
    fn with_filters<R>(&mut self, cx: &App, f: impl FnOnce(&mut Self, buffer::Filters) -> R) -> R {
        let query = self.query.read(cx).value().trim().to_lowercase();
        let coin = self.coin_filter.as_deref().map(str::to_lowercase);
        let filters = self.filters(&query, coin.as_deref());
        f(self, filters)
    }

    /// Rebuilds the filtered view over the whole buffer.
    ///
    /// The path for anything that can change which EXISTING rows qualify — a query edit, the
    /// errors-only toggle, a coin chip. It re-scans the buffer but parses nothing, and it runs on a
    /// user action, never on a backend revision.
    fn apply_filter(&mut self, cx: &App) {
        self.with_filters(cx, |t, filters| t.buf.refilter(filters));
        self.after_view_change();
    }

    /// Settles the selection and the tail after the visible list changed.
    ///
    /// Both halves have to happen wherever the view is rewritten: a rebuild can drop rows the user
    /// had selected, and endpoints past the end address other lines now, so the selection goes
    /// rather than moving to strangers.
    fn after_view_change(&mut self) {
        self.selection.clamp_to(self.buf.visible());
        if self.following() && self.buf.visible() > 0 {
            self.scroll
                .scroll_to_item(self.buf.visible() - 1, ScrollStrategy::Bottom);
        }
    }

    /// Takes `fresh` into the buffer and settles the selection against what moved.
    fn append_rows(&mut self, fresh: Vec<LogLine>, cx: &App) {
        let cap = self.cap();
        let disturbance = self.with_filters(cx, |t, filters| t.buf.ingest(fresh, cap, filters));
        // Rows that moved UNDER the selection would slide it onto lines the user never picked, and
        // `clamp_to` cannot see that — the list is still long. Rows that moved below it change
        // nothing it addresses, and on a multi-core source that is the common case, so the position
        // is compared rather than the mere fact.
        if let buffer::Disturbance::Moved { from } = disturbance
            && self.selection.range().is_some_and(|rows| rows.end > from)
        {
            self.selection.clear();
        }
        self.after_view_change();
    }

    /// Handles a press on row `ix`, starting or extending the selection.
    ///
    /// Pressing also pauses tail following the way a wheel scroll does: selected rows are addressed
    /// by position in the visible list, and following raises the cap, which evicts the oldest rows
    /// and shifts every position under the selection. Pausing raises the cap instead, so the
    /// indices stay put. An already paused panel is left alone — its timer defers itself while the
    /// selection lives, so a burst of clicks does not each spawn one.
    pub(super) fn on_row_press(&mut self, ix: usize, shift: bool, cx: &mut Context<Self>) {
        if !self.scroll_pause {
            self.pause_follow(cx);
        }
        if shift {
            self.selection.shift_press(ix);
        } else {
            self.selection.press(ix);
        }
        cx.notify();
    }

    /// Extends an in-flight selection to row `ix`.
    pub(super) fn on_row_drag(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.selection.drag_to(ix) {
            cx.notify();
        }
    }

    /// Copies the current selection, or row `ix` alone when it sits outside one.
    pub(super) fn copy_row_or_selection(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows = self.selection.range_for(ix);
        self.copy_view_rows(rows, cx);
    }

    /// Copies a range of the VISIBLE list, resolving each position through `view`.
    fn copy_view_rows(&self, rows: std::ops::Range<usize>, cx: &mut App) {
        let all = self.buf.rows();
        line_list::copy_rows(
            self.buf.visible_rows(),
            rows,
            |&ix| all.get(ix).map(row::row_copy_text).unwrap_or_default(),
            cx,
        );
    }

    /// Handles the panel's copy, select-all, and clear-selection keys.
    ///
    /// Only the panel root acts. Key events bubble from whatever holds focus, so without this check
    /// a Ctrl+C meant for the search field would also overwrite the clipboard with log rows.
    fn on_key(&mut self, ev: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        if ev.keystroke.key.as_str() == "escape" {
            self.clear_selection(cx);
            return;
        }
        match line_list::handle_list_key(&mut self.selection, ev, self.buf.visible()) {
            Some(line_list::ListKey::Copy(rows)) => {
                self.copy_view_rows(rows, cx);
            }
            Some(line_list::ListKey::SelectedAll) => {
                // Selecting everything holds the tail the way a press does.
                if !self.scroll_pause {
                    self.pause_follow(cx);
                }
                cx.notify();
            }
            None => {}
        }
    }

    /// Ends a held press: the selection gesture stops and follow resumption is free to run again.
    ///
    /// The pending timer is what resumes following, and it re-arms itself while the press is held,
    /// so releasing needs no timer of its own — the next expiry, at most one delay away, finds the
    /// hold gone.
    pub(super) fn release_press(&mut self) {
        self.selection.release();
        self.press_held = false;
    }

    /// Drops the selection; the deferred resume timer picks the panel up on its own.
    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        self.selection.clear();
        cx.notify();
    }

    /// Returns effective tail-following state: Live intent enabled and no temporary scroll pause.
    fn following(&self) -> bool {
        self.live && !self.scroll_pause
    }

    /// Resumes Live intent and queues the current selection for reload on its next actual render.
    ///
    /// Deferring the heavy work keeps a delayed scroll timer from aggregating logs after the panel
    /// has moved behind another tab or its entire outer dock has been hidden.
    fn resume_live(&mut self) {
        self.scroll_pause = false;
        self.live = true;
        self.refresh.request_reload();
    }

    /// Temporarily unchecks effective follow and schedules resumption five seconds later.
    ///
    /// Wheel scrolling and starting a row selection both use this: each is a signal that the user is
    /// reading the rows currently on screen rather than the tail. A manually disabled Live setting
    /// has nothing to pause.
    fn pause_follow(&mut self, cx: &mut Context<Self>) {
        if !self.live {
            return;
        }
        self.scroll_gen = self.scroll_gen.wrapping_add(1);
        let want_gen = self.scroll_gen;
        if !self.scroll_pause {
            self.scroll_pause = true;
            cx.notify(); // Reflect the temporarily unchecked follow control.
        }
        self.arm_resume(want_gen, cx);
    }

    /// Waits five seconds and resumes tail following, unless something newer invalidated this timer.
    ///
    /// A held selection or a held mouse button defers the resume rather than cancelling it: resuming
    /// trims the buffer back to the follow cap and the selection addresses rows by index, so the
    /// wait is simply restarted, and a scrollbar drag has no event of its own to restart it with.
    /// Giving up
    /// instead would leave following switched off for good whenever a selection was dropped by a
    /// path other than the one that armed this timer — a filter change, a reload, a cleared chip.
    fn arm_resume(&mut self, want_gen: u64, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(std::time::Duration::from_secs(5)).await;
            cx.update(|cx| {
                this.update(cx, |t, cx| {
                    if t.scroll_gen != want_gen || !t.live || !t.scroll_pause {
                        return; // A newer scroll, press, or manual Live toggle owns the state now.
                    }
                    if !t.selection.is_empty() || t.press_held {
                        t.arm_resume(want_gen, cx);
                        return;
                    }
                    t.resume_live();
                    cx.notify();
                })
                .ok();
            });
        })
        .detach();
    }

    /// Sets or clears the coin substring filter selected from a ticker in a row.
    pub(super) fn set_coin_filter(&mut self, coin: Option<String>, cx: &mut Context<Self>) {
        if self.coin_filter != coin {
            self.coin_filter = coin;
            self.selection.clear();
            self.apply_filter(cx);
            cx.notify();
        }
    }

    /// Selects the core a row's source name belongs to, as if it were picked from the source list.
    ///
    /// This is the row core-cell gesture. Classic narrows the retained source through the same
    /// selector the user can open; Auto ignores it because only the Shell rail selects a workspace
    /// core. There is deliberately no toggle-back on a second Classic click: once a core's own ring
    /// is shown, its lines carry no source label to click again.
    ///
    /// Only real cores are matched — never the `All`/`Local` pseudo-entries, whose labels a core
    /// could otherwise be named after. A name matching no configured core does nothing: it belongs
    /// to a core removed or renamed since the line was buffered.
    ///
    /// Args:
    ///     name: Display name shown in the row's source column.
    ///     cx: Panel context used to read the configured sources and reload the selection.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn select_source_by_name(&mut self, name: &str, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let core = self
            .sources(backend.read(cx))
            .into_iter()
            .find_map(|item| match item.source {
                LogSource::Core(core) if item.display == name => Some(core),
                _ => None,
            });
        if let Some(core) = core {
            self.set_source(LogSource::Core(core), cx);
        }
    }

    /// Handles a ticker right-click by requesting its chart on Main.
    ///
    /// A Core source searches only that core. Aggregate first resolves the row's `target` to a
    /// configured core, while Exchange does the same strictly inside its current membership.
    /// Unresolved aggregate sources and Local scan their allowed cores. Each candidate uses market
    /// search for `base` and the first result rather than guessing a quote suffix. Main is not
    /// activated.
    ///
    /// Args:
    ///     base: Detected base ticker from the clicked log line.
    ///     target: Core display name attached to the aggregate log row.
    ///     cx: Panel context used to read market data and publish the chart request.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn open_coin_chart(&mut self, base: String, target: String, cx: &mut Context<Self>) {
        let resolved = {
            let b = self.backend.read(cx);
            let (source, _, _) = self.effective_selection(b);
            let ms = b.session.market_source();
            let scoped = !self.group.is_empty();
            let scoped_candidates = || {
                b.config
                    .servers
                    .iter()
                    .filter(|server| !scoped || server.group == self.group)
                    .map(|server| server.id)
                    .collect::<Vec<_>>()
            };
            let candidates = match &source {
                LogSource::Core(id) => vec![*id],
                LogSource::Exchange(exchange) => {
                    let members = render::exchange_core_ids(b, &self.group, exchange);
                    render::exchange_chart_candidates(
                        b.config
                            .servers
                            .iter()
                            .map(|server| (server.id, server.name.as_str())),
                        &members,
                        &target,
                    )
                }
                LogSource::Aggregate => b
                    .config
                    .servers
                    .iter()
                    .find(|server| (!scoped || server.group == self.group) && server.name == target)
                    .map(|server| vec![server.id])
                    .unwrap_or_else(scoped_candidates),
                LogSource::Local => scoped_candidates(),
            };
            candidates.into_iter().find_map(|id| {
                ms.search_markets(id, &base, 1)
                    .into_iter()
                    .next()
                    .map(|market| (id, market))
            })
        };
        let Some((core, market)) = resolved else {
            return; // No candidate core resolved the coin to a market; leave the UI unchanged.
        };
        let workspace_group = (!self.group.is_empty()).then(|| self.group.clone());
        self.backend.update(cx, |b, bcx| {
            if b.open_on_main_if_authorized(workspace_group.as_deref(), (core, market), false) {
                bcx.notify();
            }
        });
    }

    /// Resolve current membership for an effective Exchange source.
    ///
    /// Args:
    ///     b: Backend providing the exchange membership map.
    ///     source: Effective source whose membership is requested.
    ///
    /// Returns:
    ///     Current Exchange members, or `None` for every other source.
    fn resolve_membership(&self, b: &Backend, source: &LogSource) -> Option<HashSet<CoreId>> {
        match source {
            LogSource::Exchange(exchange) => {
                Some(render::exchange_core_ids(b, &self.group, exchange))
            }
            LogSource::Aggregate | LogSource::Local | LogSource::Core(_) => None,
        }
    }

    /// Discards the buffer and reads the selected source in full.
    ///
    /// This runs when the panel has nothing it can extend: the first load, a source or file change,
    /// and an exchange membership change — the last because rows written by a departed core must not
    /// remain under the selected exchange's label.
    ///
    /// Args:
    ///     b: Backend providing source revisions, live memberships, and log rows.
    ///     cx: Application context used to rebuild the filtered view.
    ///
    /// Returns:
    ///     Nothing.
    fn reload_rows(&mut self, b: &Backend, cx: &App) {
        crate::diag::bump(&crate::diag::LOG_RELOAD);
        let timer = diag_timer();
        let (source, file, workspace_owned) = self.effective_selection(b);
        self.refresh
            .record_reload(render::log_sig(b, &self.group, &source));
        let sources = self.data_sources(b, &source, workspace_owned);
        let is_agg = matches!(source, LogSource::Aggregate | LogSource::Exchange(_));
        if !is_agg {
            let label = Self::file_label(&sources, &source);
            self.refresh_available_files(&label);
        }
        let membership = self.resolve_membership(b, &source);
        self.exchange_membership = membership.clone();
        self.sources_sig = Self::sources_sig(&sources);
        self.loaded_selection = Some((source.clone(), file.clone()));
        // Row indices no longer address the same lines, so a selection cannot survive this.
        self.selection.clear();
        self.buf.clear();
        self.cursors.clear();
        let fresh = self.snapshot(b, &source, &file, &sources, membership.as_ref());
        if matches!(file, LogFile::Live) {
            // The core store is only mutated on this thread, so its counters and the snapshot
            // describe one instant. The local ring is not — `snapshot` took its cursor under the
            // ring lock and recorded it already, which is why this cannot do it here.
            self.cursors
                .seek_to_end(b, &source, &sources, membership.as_ref());
        }
        self.append_rows(fresh, cx);
        // An empty source still needs its view settled: `append_rows` has nothing to do, and the
        // selection cleared above has to reach the scroll handle.
        if self.buf.total() == 0 {
            self.after_view_change();
        }
        record_work_us(timer);
    }

    /// Identity of the source list: which cores it holds and what they are called.
    ///
    /// Rows carry a core's DISPLAY NAME, copied at the moment they were pulled. A rename or a
    /// removal therefore leaves already-buffered rows labelled with a name that no longer selects
    /// anything, and appending cannot fix rows it is not touching — so a change here forces the full
    /// reload that relabels them. The old per-revision rebuild got this for free.
    fn sources_sig(sources: &[LogSourceItem]) -> u64 {
        sources.iter().fold(0u64, |sig, item| {
            let id = match item.source {
                LogSource::Core(id) => id,
                _ => 0,
            };
            item.display.bytes().fold(
                sig.wrapping_mul(31).wrapping_add(id).wrapping_mul(31),
                |sig, byte| sig.wrapping_mul(131).wrapping_add(u64::from(byte)),
            )
        })
    }

    /// Extends the buffer with whatever the live source produced since the last read.
    ///
    /// This is the ordinary path, taken on every backend revision while the tab is open. It costs
    /// one parse per NEW line; a named file has no live tail, and an exchange whose membership
    /// changed cannot be extended at all and falls back to a full reload.
    ///
    /// Args:
    ///     b: Backend providing source revisions, live memberships, and log rows.
    ///     cx: Application context used to rebuild the filtered view.
    ///
    /// Returns:
    ///     Nothing.
    fn pull_rows(&mut self, b: &Backend, cx: &App) {
        let timer = diag_timer();
        let (source, file, workspace_owned) = self.effective_selection(b);
        if self.loaded_selection.as_ref() != Some(&(source.clone(), file.clone())) {
            self.reload_rows(b, cx);
            return;
        }
        let membership = self.resolve_membership(b, &source);
        let sources = self.data_sources(b, &source, workspace_owned);
        // Two changes the buffer cannot absorb, because both invalidate rows it is not touching:
        // an exchange losing a member (its rows must not stay under the exchange's label) and a
        // core being added, removed or renamed (its rows carry the old name).
        if exchange_membership_changed(self.exchange_membership.as_ref(), membership.as_ref())
            || self.sources_sig != Self::sources_sig(&sources)
        {
            self.reload_rows(b, cx);
            return;
        }
        if !matches!(file, LogFile::Live) {
            // A rotated file has no tail to follow. Resuming follow still has to return the list to
            // the bottom, which is the only reason this path runs for a file at all.
            self.refresh
                .record_reload(render::log_sig(b, &self.group, &source));
            self.after_view_change();
            return;
        }
        // A selected core that left the store — deactivated, or its server removed — has no rows
        // any more. Keeping the ones already buffered would show a dead core's history under a
        // selector that now resolves to nothing, and count it in the row total.
        //
        // Only while following, though: a paused panel is one the user is READING, and emptying it
        // under them because a core went away loses the history they stopped to look at. It clears
        // when they return to the tail.
        if let LogSource::Core(id) = &source
            && b.session.store().core(*id).is_none()
            && self.buf.total() > 0
            && self.following()
        {
            self.reload_rows(b, cx);
            return;
        }
        // BEFORE the pull, and the order matters. For the local ring the signature and the cursor
        // come from different counters, so whichever is read second may include a line the other
        // missed. Recording first means the signature can only LAG the cursor: the gate fires once
        // more with nothing to read, which costs an empty pass. The reverse — recording after —
        // stamps a line as seen that the cursor has not yet returned, and the gate then sits on it
        // until some unrelated line arrives.
        self.refresh
            .record_reload(render::log_sig(b, &self.group, &source));
        crate::diag::bump(&crate::diag::LOG_PULL);
        let fresh = self.cursors.pull(b, &source, &sources, membership.as_ref());
        // Resuming follow lowers the cap, so the buffer can need evicting with nothing new in it,
        // and returning to the tail is this path's whole job when follow resumes.
        self.append_rows(fresh, cx);
        record_work_us(timer);
    }

    /// Changes visibility-driven refresh activity and catches up immediately when activation is
    /// newer than the last loaded source revision.
    ///
    /// Catching up EXTENDS the buffer once the panel has loaded before. Reloading instead would
    /// throw away what a paused panel is holding — up to `PAUSED_CAP` rows, the selection in them
    /// and the scroll position — every time the user visits another tab and comes back.
    fn set_refresh_active(&mut self, active: bool, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let (source, _, _) = self.effective_selection(backend.read(cx));
        let sig = render::log_sig(backend.read(cx), &self.group, &source);
        let loaded = self.refresh.has_loaded();
        if self.refresh.set_active(active, sig) {
            if loaded {
                self.pull_rows(backend.read(cx), cx);
            } else {
                self.reload_rows(backend.read(cx), cx);
            }
            cx.notify();
        }
    }

    /// Resets effective following after an explicit source or file change and invalidates pending
    /// scroll-resume timers, so a timer armed under the previous selection cannot fire against the
    /// new one and trim its buffer to the follow cap behind the user's back.
    fn reset_to_live(&mut self) {
        self.live = true;
        self.scroll_pause = false;
        self.scroll_gen = self.scroll_gen.wrapping_add(1);
        self.selection.clear();
    }

    /// Select a retained Classic source or ignore an Auto source shortcut owned by the Shell rail.
    ///
    /// Args:
    ///     s: Requested source from a selector or source-name shortcut.
    ///     cx: Panel context used to reload Classic data.
    ///
    /// Returns:
    ///     Nothing; every Auto request leaves workspace and retained state unchanged.
    pub(super) fn set_source(&mut self, s: LogSource, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let (_, _, workspace_owned) = self.effective_selection(backend.read(cx));
        if workspace_owned {
            return;
        }
        if self.source != s {
            self.source = s;
            // A source change returns to Live and invalidates both named-file caches.
            self.file = LogFile::Live;
            self.loaded_name = None;
            self.available_files_label = None;
            self.available_files.clear();
            self.reset_to_live();
            self.reload_rows(backend.read(cx), cx);
            cx.notify();
        }
    }
    /// Select a retained Classic file while keeping Auto permanently pinned to Live.
    ///
    /// Args:
    ///     f: Requested live or named file selection.
    ///     cx: Panel context used to reload the selected Classic file.
    ///
    /// Returns:
    ///     Nothing; Auto requests are ignored without mutating the retained file.
    pub(super) fn set_file(&mut self, f: LogFile, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        if self.effective_selection(backend.read(cx)).2 {
            return;
        }
        if self.file != f {
            self.file = f;
            self.reset_to_live();
            self.reload_rows(backend.read(cx), cx);
            cx.notify();
        }
    }
}

impl EventEmitter<PanelEvent> for LogPanel {}
impl Focusable for LogPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for LogPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    /// Enables live refresh only for the front dock tab and catches up when it becomes visible.
    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_refresh_active(active, cx);
    }
    fn panel_name(&self) -> &'static str {
        "Log"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("Log", &self.group)
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
        Some(vec![crate::panels::detach_button(
            "Log",
            self.group.clone(),
            self.backend.clone(),
            self.dock.clone(),
        )])
    }
}
