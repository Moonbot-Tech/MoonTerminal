//! Log panel ported from egui's `src/dock/log_panel.rs`, with source and file selection, text and
//! coin filters, and an errors-only mode.
//!
//! Sources are the live aggregate of in-scope core logs, the local application's `applog` ring, and
//! each configured core's `CoreData.log` ring. Local and single-core sources can show either Live or
//! a rotated `logs/<date>_<source>.log` file; Aggregate is Live-only. `MoonVirtualList` virtualizes
//! rows, and effective follow mode keeps filtered output at the tail.
//!
//! State, row collection, filtering, and lifecycle live here; source and file selectors are in
//! [`controls`]; signatures, aggregation, classification, and row rendering are in [`render`].

mod controls;
mod render;

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
use moon_core::session::{CoreId, CoreStore};

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
    Local,
    Core(CoreId),
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
    /// Filtered render rows indexed by the virtual list. Classification and coin detection are
    /// precomputed in `apply_filter` rather than repeated for each rendered frame.
    lines: Vec<render::LineView>,
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
    /// Last observed combined log signature. Rebuilding only on a revision change avoids repeatedly
    /// cloning bounded source snapshots while scoped logs are idle. Aggregate may clone up to
    /// `AGG_PER_CORE` rows from every scoped core before its final `VIEW_LIMIT` truncation.
    last_sig: u64,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl LogPanel {
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
                t.apply_filter(cx);
                cx.notify();
            }
        })
        .detach();
        // Reload only when the combined local and in-scope core log revision changes. This may wake
        // a selected source even when the new row belongs to another source in the same scope.
        cx.observe(&backend, |this, backend, cx| {
            let sig = render::log_sig(backend.read(cx), &this.group);
            if sig != this.last_sig {
                this.last_sig = sig;
                this.reload_rows(backend.read(cx), cx);
                cx.notify();
            }
        })
        .detach();
        let mut this = Self {
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
            lines: Vec::new(),
            total: 0,
            live: true,
            scroll_pause: false,
            scroll_gen: 0,
            scroll: MoonVirtualListScrollHandle::new(),
            last_sig: 0,
            dock: None,
            focus: cx.focus_handle(),
        };
        let backend_for_initial_load = this.backend.clone();
        this.reload_rows(backend_for_initial_load.read(cx), cx);
        this
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

    /// Collects rows for the current selection. Live reads the local ring, one core ring, or the
    /// merged core aggregate; Named reads and then caches at most `VIEW_LIMIT` rows from disk.
    fn gather(&mut self, store: &CoreStore, sources: &[LogSourceItem]) -> Vec<LogLine> {
        match &self.file {
            LogFile::Live => {
                self.loaded_name = None;
                match &self.source {
                    LogSource::Local => applog::snapshot(VIEW_LIMIT),
                    LogSource::Core(id) => store
                        .core(*id)
                        .map(|c| c.log_snapshot(VIEW_LIMIT))
                        .unwrap_or_default(),
                    LogSource::Aggregate => render::aggregate(store, sources),
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
            .filter(|l| {
                query.is_empty()
                    || l.msg.to_lowercase().contains(&query)
                    || l.target.to_lowercase().contains(&query)
            })
            .filter(|l| {
                coin.as_ref()
                    .is_none_or(|c| l.msg.to_lowercase().contains(c))
            })
            .filter_map(|l| {
                let cl = render::classify(l);
                if errors_only && !render::is_error(cl.sev) {
                    return None;
                }
                Some(render::LineView::from_parts(l, cl, &known))
            })
            .collect();
        if self.following() && !self.lines.is_empty() {
            self.scroll
                .scroll_to_item(self.lines.len() - 1, ScrollStrategy::Bottom);
        }
    }

    /// Returns effective tail-following state: Live intent enabled and no temporary scroll pause.
    fn following(&self) -> bool {
        self.live && !self.scroll_pause
    }

    /// Resumes Live intent, clears the scroll pause, reloads the current selection, and scrolls to
    /// the filtered tail through `reload_rows` and `apply_filter`.
    fn resume_live(&mut self, cx: &mut Context<Self>) {
        self.scroll_pause = false;
        self.live = true;
        let backend = self.backend.clone();
        self.last_sig = render::log_sig(backend.read(cx), &self.group);
        self.reload_rows(backend.read(cx), cx);
    }

    /// Handles a wheel event over the list. With Live intent enabled it temporarily unchecks
    /// effective follow and schedules resumption five seconds after the latest scroll. A manually
    /// disabled Live setting ignores scrolling.
    fn on_user_scroll(&mut self, cx: &mut Context<Self>) {
        if !self.live {
            return;
        }
        self.scroll_gen = self.scroll_gen.wrapping_add(1);
        let want_gen = self.scroll_gen;
        if !self.scroll_pause {
            self.scroll_pause = true;
            cx.notify(); // Reflect the temporarily unchecked follow control.
        }
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(std::time::Duration::from_secs(5)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |t, cx| {
                    // Resume only if no newer scroll or manual Live toggle invalidated this timer.
                    if t.scroll_gen == want_gen && t.live && t.scroll_pause {
                        t.resume_live(cx);
                        cx.notify();
                    }
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
            self.apply_filter(cx);
            cx.notify();
        }
    }

    /// Handles a ticker right-click by requesting its chart on Main. A Core source searches only
    /// that core; Aggregate first resolves the row's `target` to a configured core; unresolved
    /// Aggregate and Local rows scan configured cores in scope. Each candidate uses market search
    /// for `base` and the first result rather than guessing a quote suffix. Main is not activated.
    pub(super) fn open_coin_chart(&mut self, base: String, target: String, cx: &mut Context<Self>) {
        let resolved = {
            let b = self.backend.read(cx);
            let ms = b.session.market_source();
            let core = match &self.source {
                LogSource::Core(id) => Some(*id),
                LogSource::Aggregate => b
                    .config
                    .servers
                    .iter()
                    .find(|s| s.name == target)
                    .map(|s| s.id),
                LogSource::Local => None,
            };
            let scoped = !self.group.is_empty();
            let candidates: Vec<CoreId> = match core {
                Some(id) => vec![id],
                None => b
                    .config
                    .servers
                    .iter()
                    .filter(|s| !scoped || s.group == self.group)
                    .map(|s| s.id)
                    .collect(),
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

    fn reload_rows(&mut self, b: &Backend, cx: &App) {
        let sources = self.sources(b);
        let is_agg = matches!(self.source, LogSource::Aggregate);
        if !is_agg {
            let label = self.file_label(&sources);
            self.refresh_available_files(&label);
        }
        let fresh = self.gather(b.session.store(), &sources);
        if self.following() {
            // Effective follow replaces the buffer with the current bounded snapshot.
            self.raw_lines = fresh;
        } else {
            // While following is paused, append only unseen suffix rows and retain the existing
            // prefix so the scroll position stays stable, up to `PAUSED_CAP`.
            self.merge_paused(fresh);
        }
        self.apply_filter(cx);
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
        }
    }

    /// Resets effective following after an explicit source or file change and invalidates pending
    /// scroll-resume timers so `merge_paused` cannot combine rows from different selections.
    fn reset_to_live(&mut self) {
        self.live = true;
        self.scroll_pause = false;
        self.scroll_gen = self.scroll_gen.wrapping_add(1);
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

impl Render for LogPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);

        let sources = self.sources(self.backend.read(cx));
        let is_agg = matches!(self.source, LogSource::Aggregate);
        let total = self.total;

        // Build the wrapping filter and follow controls.
        let mut controls = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1();
        controls = controls.child(self.source_combo(&sources, cx));
        if !is_agg {
            controls = controls
                .child(
                    div()
                        .text_size(crate::design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("log.file").to_string()),
                )
                .child(self.file_combo(&self.available_files, cx));
        }
        controls = controls
            .child(
                div().w(px(180.0)).child(
                    MoonInput::new("log-query")
                        .state(&self.query)
                        .small()
                        .cleanable(true),
                ),
            )
            .child(
                MoonCheckbox::new("log-errors-only")
                    .label(t!("log.errors_only").to_string())
                    .checked(self.errors_only)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|t, ch: &bool, _, cx| {
                        if t.errors_only != *ch {
                            t.errors_only = *ch;
                            t.apply_filter(cx);
                            cx.notify();
                        }
                    })),
            )
            .child(
                MoonCheckbox::new("log-live")
                    .label(t!("log.follow_tail").to_string())
                    .checked(self.following())
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|t, ch: &bool, _, cx| {
                        // A manual toggle invalidates any delayed automatic resumption.
                        t.scroll_gen = t.scroll_gen.wrapping_add(1);
                        if *ch {
                            t.resume_live(cx); // Reload and return to the current selection's tail.
                        } else {
                            // Manual disable freezes following until the user enables it again.
                            t.live = false;
                            t.scroll_pause = false;
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_size(crate::design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("log.count", shown = self.lines.len(), total = total).to_string()),
            );
        // Show a removable chip for the active coin filter.
        if let Some(coin) = self.coin_filter.clone() {
            controls = controls.child(
                div()
                    .id("log-coin-chip")
                    .flex_none()
                    .cursor_pointer()
                    .px_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(p.blue))
                    .text_size(crate::design::t_body(cx))
                    .text_color(rgb(p.blue))
                    .child(format!("{coin} ✕"))
                    .on_click(cx.listener(|t, _, _, cx| t.set_coin_filter(None, cx))),
            );
        }

        // Build the tail-oriented virtualized list or its empty-state message.
        let weak = cx.entity().downgrade();
        let body: AnyElement = if self.lines.is_empty() {
            let msg = if total == 0 {
                t!("dock.log.empty").to_string()
            } else {
                t!("log.empty_filtered").to_string()
            };
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(p.text_soft))
                .child(msg)
                .into_any_element()
        } else {
            let scroll = self.scroll.clone();
            let query = self.query.read(cx).value().trim().to_lowercase();
            let list_el = MoonVirtualList::new(
                "log-virtual-rows",
                self.lines.len(),
                // Scale row height with the font because MoonVirtualList accepts raw pixels; a
                // fixed 18 px row clipped text at the +6 font setting.
                crate::design::fit_h_value(cx, 18.0, 14.0, 2.0),
                move |ix, _w, app| {
                    weak.upgrade()
                        .and_then(|e| {
                            e.read(app)
                                .lines
                                .get(ix)
                                .map(|line| render::log_row(line, &query, &weak, p, app))
                        })
                        .unwrap_or_else(|| div().into_any_element())
                },
            )
            .track_scroll(&scroll)
            .surface(false)
            .border(false)
            .radius(0.0)
            .scrollbar_visibility(MoonScrollbarVisibility::Hover);
            div()
                .flex_1()
                .w_full()
                .min_h_0()
                .child(list_el)
                // Any wheel event over the list pauses effective following and starts its timer.
                .on_scroll_wheel(cx.listener(|t, _e: &ScrollWheelEvent, _w, cx| {
                    t.on_user_scroll(cx);
                }))
                .into_any_element()
        };

        v_flex()
            .id("log-panel")
            .size_full()
            .track_focus(&self.focus)
            // Set the monospace font on this root, as Orders, Assets, and Report do. A detached
            // panel does not inherit it from the dock header; without this, it would render in Inter
            // and disagree with both the docked view and selector-width measurements in controls.rs.
            .font_family(crate::design::mono())
            .child(controls)
            .child(div().w_full().h(px(1.0)).bg(rgb(p.border)))
            .child(body)
    }
}
