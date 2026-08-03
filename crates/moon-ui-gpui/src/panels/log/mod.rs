//! Log panel ported from egui's `src/dock/log_panel.rs`, with source and file selection, text and
//! coin filters, and an errors-only mode.
//!
//! Sources are live aggregates of all in-scope cores or one reported exchange, the local
//! application's `applog` ring, and each configured core's `CoreData.log` ring. Local and
//! single-core sources can show either Live or a rotated `logs/<date>_<source>.log` file; aggregate
//! sources are Live-only. `MoonVirtualList` virtualizes rows, and effective follow mode keeps
//! filtered output at the tail.
//!
//! State, row collection, filtering, and lifecycle live here; source and file selectors are in
//! [`controls`]; rebuild signatures and aggregation in [`render`]; one row's elements in [`row`];
//! the panel's own element tree in [`view`]. Line classification, the row-range selection, the copy
//! commands and the horizontal viewport are shared with the Report's trade-log dialog and live in
//! [`crate::panels::line_list`].

mod controls;
mod render;
mod row;
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
use moon_core::applog::{self, LogLine};
use moon_core::session::CoreId;
use std::collections::HashSet;

/// Maximum number of recent rows retained in a normal live or file snapshot.
const VIEW_LIMIT: usize = 5000;
/// Maximum number of rows taken from each core before building the aggregate.
const AGG_PER_CORE: usize = 2000;
/// Buffer cap while tail following is paused. Fresh rows are appended beyond `VIEW_LIMIT` without
/// replacing the existing prefix so the scroll position does not shift; returning to effective
/// follow mode replaces it with a normal bounded snapshot.
const PAUSED_CAP: usize = 20_000;

/// Selected source of log rows.
#[derive(Clone, PartialEq)]
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
#[derive(Clone, PartialEq)]
pub(super) enum LogFile {
    Live,
    Named(String),
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
    /// Unfiltered rows for the current source and file, updated outside `render`.
    raw_lines: Vec<LogLine>,
    /// Exchange membership used by `raw_lines`; a membership change invalidates paused history.
    exchange_membership: Option<HashSet<CoreId>>,
    /// Filtered render rows indexed by the virtual list. Classification and coin detection are
    /// precomputed in `apply_filter` rather than repeated for each rendered frame.
    lines: Vec<render::LineView>,
    /// Character budget of the widest filtered row, sizing the horizontal scroll area.
    widest_chars: usize,
    /// Rows selected for copying, as indices into `lines`.
    selection: RowSelection,
    total: usize,
    /// User intent to follow the tail. Turning Live off manually prevents automatic resumption until
    /// the user enables it again.
    live: bool,
    /// Temporary follow pause set by wheel scrolling and cleared five seconds after the latest
    /// scroll. While true, filtering does not jump to the tail.
    scroll_pause: bool,
    /// Generation guarding delayed follow resumption. A new scroll or manual Live toggle invalidates
    /// earlier timers, so the five-second delay starts at the latest scroll.
    scroll_gen: u64,
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
        // Reload only while visible and only when the selected live source's revision changes.
        cx.observe(&backend, |this, backend, cx| {
            if !this.refresh.is_active() || !matches!(this.file, LogFile::Live) {
                return;
            }
            let sig = render::log_sig(backend.read(cx), &this.group, &this.source);
            if this.refresh.observe(sig) {
                cx.notify();
            }
        })
        .detach();
        Self {
            backend,
            group,
            source: LogSource::Aggregate,
            file: LogFile::Live,
            errors_only: true,
            coin_filter: None,
            query,
            loaded_name: None,
            loaded_lines: Vec::new(),
            available_files_label: None,
            available_files: Vec::new(),
            raw_lines: Vec::new(),
            exchange_membership: None,
            lines: Vec::new(),
            widest_chars: 0,
            selection: RowSelection::default(),
            total: 0,
            live: true,
            scroll_pause: false,
            scroll_gen: 0,
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

    pub(super) fn file_label(&self, sources: &[LogSourceItem]) -> String {
        sources
            .iter()
            .find(|s| s.source == self.source)
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

    /// Collects rows for the current selection. Live reads the local ring, one core ring, every
    /// scoped core, or the selected exchange's current membership. Named reads and then caches at
    /// most `VIEW_LIMIT` rows from disk.
    ///
    /// Args:
    ///     b: Backend providing live core stores and file-independent source state.
    ///     sources: Canonically ordered source entries in the panel scope.
    ///     exchange_membership: Pre-resolved membership for an Exchange source.
    ///
    /// Returns:
    ///     The selected source's bounded live snapshot or cached named-file rows.
    fn gather(
        &mut self,
        b: &Backend,
        sources: &[LogSourceItem],
        exchange_membership: Option<&HashSet<CoreId>>,
    ) -> Vec<LogLine> {
        match &self.file {
            LogFile::Live => {
                self.loaded_name = None;
                match &self.source {
                    LogSource::Local => applog::snapshot(VIEW_LIMIT),
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

    /// Rebuilds render-ready rows while sharing one lowercase message across all text predicates.
    fn apply_filter(&mut self, cx: &App) {
        let query = self.query.read(cx).value().trim().to_lowercase();
        let errors_only = self.errors_only;
        let coin = self.coin_filter.as_deref().map(str::to_lowercase);
        self.total = self.raw_lines.len();
        // Collect coin bases across the whole raw buffer so bare tickers such as `SPK` can be
        // recognized after appearing elsewhere in a market form such as `USDT-SPK`.
        let known = render::collect_coin_bases(&self.raw_lines);
        // Classify severity and detect the coin once here. Text parsing is expensive, while visible
        // row rendering runs every frame.
        self.lines = self
            .raw_lines
            .iter()
            .filter_map(|l| {
                let lower = l.msg.to_lowercase();
                let cl = line_list::classify_lower(l.level, &lower);
                if errors_only && !line_list::is_error(cl.sev) {
                    return None;
                }
                if !query.is_empty()
                    && !lower.contains(&query)
                    && !l.target.to_lowercase().contains(&query)
                {
                    return None;
                }
                if coin.as_ref().is_some_and(|coin| !lower.contains(coin)) {
                    return None;
                }
                Some(render::LineView::from_parts(l, cl, &known))
            })
            .collect();
        // Size the horizontal scroll area from the widest row that survived the filters. The cap is
        // what keeps ONE outlier from setting the width of the whole panel: a flattened backtrace or
        // a raw exchange JSON response is a single row tens of thousands of characters wide, and
        // sizing to it would shrink the scrollbar thumb to nothing for every ordinary line. Past the
        // cap a row is clipped and read through its copy instead.
        self.widest_chars = self
            .lines
            .iter()
            .map(row::row_width_chars)
            .max()
            .unwrap_or(0)
            .min(line_list::WIDEST_CHARS_CAP);
        // A rebuild can drop rows the user had selected; endpoints past the end address other
        // lines now, so the selection goes rather than moving to strangers.
        self.selection.clamp_to(self.lines.len());
        if self.following() && !self.lines.is_empty() {
            self.scroll
                .scroll_to_item(self.lines.len() - 1, ScrollStrategy::Bottom);
        }
    }

    /// Handles a press on row `ix`, starting or extending the selection.
    ///
    /// Pressing also pauses tail following the way a wheel scroll does: rows are addressed by index,
    /// and a following reload REPLACES the buffer, which would leave the selection pointing at
    /// different lines. While paused, reloads only append, so the indices stay put. An already
    /// paused panel is left alone — its timer defers itself while the selection lives, so a burst
    /// of clicks does not each spawn one.
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
        line_list::copy_rows(&self.lines, rows, row::row_copy_text, cx);
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
        match line_list::handle_list_key(&mut self.selection, ev, self.lines.len()) {
            Some(line_list::ListKey::Copy(rows)) => {
                line_list::copy_rows(&self.lines, rows, row::row_copy_text, cx);
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
    /// A held selection defers the resume rather than cancelling it: resuming replaces the buffer
    /// and the selection addresses rows by index, so the wait is simply restarted. Giving up
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
                    if !t.selection.is_empty() {
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
    /// This is the Report's core-cell gesture: the click narrows the panel to one core through the
    /// SAME control the user would otherwise open, and that control is then both the indicator and
    /// the way back. There is deliberately no toggle-back on a second click: only the aggregate and
    /// exchange sources label their rows with a core name, so once this switches to that core the
    /// name is no longer on screen to click (a core's own ring lines carry no source label).
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
            let candidates = match &self.source {
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
        self.backend.update(cx, |b, bcx| {
            b.open_on_main((core, market), false);
            bcx.notify();
        });
    }

    /// Reloads the selected source, records its revision, and reapplies the current filters.
    ///
    /// A paused buffer retains ordinary new suffix rows, but an exchange membership change replaces
    /// it so departed-core rows cannot remain under the selected exchange label.
    ///
    /// Args:
    ///     b: Backend providing source revisions, live memberships, and log rows.
    ///     cx: Application context used to rebuild filtered render rows.
    ///
    /// Returns:
    ///     Nothing.
    fn reload_rows(&mut self, b: &Backend, cx: &App) {
        self.refresh
            .record_reload(render::log_sig(b, &self.group, &self.source));
        let sources = self.sources(b);
        let is_agg = matches!(self.source, LogSource::Aggregate | LogSource::Exchange(_));
        if !is_agg {
            let label = self.file_label(&sources);
            self.refresh_available_files(&label);
        }
        let exchange_membership = match &self.source {
            LogSource::Exchange(exchange) => {
                Some(render::exchange_core_ids(b, &self.group, exchange))
            }
            LogSource::Aggregate | LogSource::Local | LogSource::Core(_) => None,
        };
        let membership_changed = exchange_membership_changed(
            self.exchange_membership.as_ref(),
            exchange_membership.as_ref(),
        );
        self.exchange_membership = exchange_membership.clone();
        let fresh = self.gather(b, &sources, exchange_membership.as_ref());
        if self.following() || membership_changed {
            // Effective follow replaces the buffer with the current bounded snapshot. Row indices
            // no longer address the same lines, so a selection cannot survive it.
            self.selection.clear();
            self.raw_lines = fresh;
        } else {
            // While following is paused, append only unseen suffix rows and retain the existing
            // prefix so the scroll position stays stable, up to `PAUSED_CAP`.
            self.merge_paused(fresh);
        }
        self.apply_filter(cx);
    }

    /// Changes visibility-driven refresh activity and catches up immediately when activation is
    /// newer than the last loaded source revision.
    fn set_refresh_active(&mut self, active: bool, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let sig = render::log_sig(backend.read(cx), &self.group, &self.source);
        if self.refresh.set_active(active, sig) {
            self.reload_rows(backend.read(cx), cx);
            cx.notify();
        }
    }

    /// Merges a fresh snapshot into the paused buffer by finding its last retained row using
    /// timestamp, message, and target, then appending the fresh suffix. If the boundary has fallen
    /// out of the bounded snapshot, the whole snapshot is appended and a history gap can remain.
    /// Drops the oldest prefix above `PAUSED_CAP`.
    fn merge_paused(&mut self, fresh: Vec<LogLine>) {
        match self.raw_lines.last() {
            None => self.raw_lines = fresh,
            Some(last) => {
                let boundary = fresh
                    .iter()
                    .rposition(|l| l.ts == last.ts && l.msg == last.msg && l.target == last.target);
                match boundary {
                    Some(pos) => self.raw_lines.extend(fresh.into_iter().skip(pos + 1)),
                    None => self.raw_lines.extend(fresh),
                }
            }
        }
        if self.raw_lines.len() > PAUSED_CAP {
            let drop = self.raw_lines.len() - PAUSED_CAP;
            self.raw_lines.drain(0..drop);
            // Dropping a prefix shifts every row index down. The selection addresses rows by index,
            // so keeping it would slide it onto lines the user never picked; `clamp_to` cannot see
            // this because the list is still long. A selection holds following paused, so this is
            // reachable: enough lines arrive under a selection held open on a busy aggregate.
            self.selection.clear();
        }
    }

    /// Resets effective following after an explicit source or file change and invalidates pending
    /// scroll-resume timers so `merge_paused` cannot combine rows from different selections.
    fn reset_to_live(&mut self) {
        self.live = true;
        self.scroll_pause = false;
        self.scroll_gen = self.scroll_gen.wrapping_add(1);
        self.selection.clear();
    }

    pub(super) fn set_source(&mut self, s: LogSource, cx: &mut Context<Self>) {
        if self.source != s {
            self.source = s;
            // A source change returns to Live and invalidates both named-file caches.
            self.file = LogFile::Live;
            self.loaded_name = None;
            self.available_files_label = None;
            self.available_files.clear();
            self.reset_to_live();
            let backend = self.backend.clone();
            self.reload_rows(backend.read(cx), cx);
            cx.notify();
        }
    }
    pub(super) fn set_file(&mut self, f: LogFile, cx: &mut Context<Self>) {
        if self.file != f {
            self.file = f;
            self.reset_to_live();
            let backend = self.backend.clone();
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
