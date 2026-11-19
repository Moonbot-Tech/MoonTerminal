//! Screener state and window lifecycle.
//!
//! [`ScreenerView`] owns filters, sorting, cached-row rebuilds, window rendering, and the `open`
//! entry point. Column definitions, value sorting, and row rendering live in [`super::table`].

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButtonSize, MoonButtonVariant, MoonDataTable, MoonDataTableColumn,
    MoonDataTableState, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem,
    MoonPalette, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::market::listing::Listing;
use moon_core::session::CoreId;
use moon_core::venue::{CoreVenue, MarketKind};

use crate::controls::CoreComboHost;
use crate::panels::data_table_host;
use crate::workspace::scope_marker::{self, ScopeMarker};
use crate::{Backend, design};

use super::auto_figures::ScanQuery;
use super::table::{
    COLS, ColDef, Entry, EntryVenue, RowStyle, UNKNOWN_CHOICE, column_label, column_title, moon,
    moon_alpha, parse_vol, screener_row,
};

mod scan;

/// What the Type list offers, in menu order. `None` is an exchange this build cannot name.
const TYPE_CHOICES: [Option<MarketKind>; 4] = [
    Some(MarketKind::Spot),
    Some(MarketKind::Futures),
    Some(MarketKind::Quarterly),
    None,
];

/// What the Listed list offers, in menu order: the answers first, unknown last.
const LISTED_CHOICES: [Listing; 4] = [
    Listing::Both,
    Listing::SpotOnly,
    Listing::FuturesOnly,
    Listing::Unknown,
];

const SCREENER_HEADER_H: f32 = 32.0;

/// Version of the one-shot migration that adds late columns to stored visible sets; see
/// [`super::table::add_new_columns`] and `WindowLayout::screener_columns_migration`. The number
/// itself lives beside the columns it counts.
const SCREENER_COLUMNS_MIGRATION: u32 = super::table::COLUMNS_MIGRATION;

/// Owned counts reused across publications until the corresponding core receives new orders.
type OrderCounts = Arc<HashMap<String, u32>>;

/// Order totals signed by the core's table revision under the current fleet incarnation.
#[derive(Default)]
struct OrderCountMemo {
    fleet_epoch: u64,
    cores: HashMap<CoreId, (Option<u64>, OrderCounts)>,
}

impl OrderCountMemo {
    /// Retire counts when a removed core can reappear with a previously seen table revision.
    fn update_fleet_epoch(&mut self, epoch: u64) {
        if self.fleet_epoch != epoch {
            self.fleet_epoch = epoch;
            self.cores.clear();
        }
    }

    /// Return an owned snapshot for the publisher, scanning only a core with a new order batch.
    fn counts(
        &mut self,
        core: CoreId,
        table_revision: u64,
        orders: &[moon_core::feed::OrderRow],
    ) -> OrderCounts {
        let (revision, counts) = self.cores.entry(core).or_default();
        if *revision != Some(table_revision) {
            let mut next: HashMap<&str, u32> = HashMap::new();
            for order in orders {
                *next.entry(order.market.as_str()).or_default() += 1;
            }
            *counts = Arc::new(
                next.into_iter()
                    .map(|(market, count)| (market.to_owned(), count))
                    .collect(),
            );
            *revision = Some(table_revision);
        }
        Arc::clone(counts)
    }
}

impl crate::controls::CoreComboHost for ScreenerView {
    /// The Screener is a group-less singleton: no Auto workspace owns its selector.
    fn core_selection_pinned(&self, _cx: &App) -> bool {
        false
    }

    fn core_selection_mut(&mut self) -> &mut HashSet<u64> {
        &mut self.sel_cores
    }

    /// Apply the new selection and repaint.
    ///
    /// [`Self::rebuild`] does both halves this needs, as it does for the sort and the text boxes:
    /// it applies the controls to the rows already published at once, since those cover every
    /// core, and it starts a fresh publication with them rather than leaving the table on the old
    /// one until the trailing one-second refresh.
    fn after_core_selection_change(&mut self, cx: &mut Context<Self>) {
        self.rebuild(cx);
        cx.notify();
    }
}

/// State for the singleton Screener window.
pub struct ScreenerView {
    pub(super) backend: Entity<Backend>,
    /// Cached rows after the current source, coin, volume, and sort selections are applied.
    rows: Rc<Vec<usize>>,
    /// Source rows and their dense counting identities are published together.
    prepared: std::sync::Arc<super::prepared::Prepared>,
    /// A newer refresh retires the previous publication task.
    prepare_task: Option<Task<()>>,
    /// Publication identity independent of user controls, which apply to either snapshot.
    prepare_seq: u64,
    order_counts: OrderCountMemo,
    /// Distinct markets after source grouping but before the coin and volume filters, the footer's
    /// "total". Markets, not rows: a market has one row per core sharing its exchange.
    total: usize,
    /// Distinct markets among [`Self::rows`], the footer's "shown" — counted the same way.
    shown: usize,
    /// Workspace scope marker over every live session, or `None` when the preset is unresolved.
    /// A resolved full scope still keeps a marker whose facts are empty. Screener is a group-less
    /// singleton with no `EffectiveCoreScope` of its own, so this is built with
    /// `ScopeMarker::from_membership` instead of `Backend::new`.
    scope_marker: Option<ScopeMarker>,
    /// Live session count, the universe `scope_marker` was built over. Distinguishes "no session
    /// at all" (the original connect-a-core advice still applies) from "sessions exist but no
    /// rows survived filtering" (a different, non-actionable empty sentence).
    sessions_total: usize,
    /// Whether the current publication holds rows of at least one core the preset SHOWS.
    ///
    /// What tells "the bottom bar's controls left no row" from "nothing has arrived for what is
    /// on screen": rows of a core the preset hides are in the publication too, and counting them
    /// would blame the filters for a visible core that simply has no data yet.
    has_displayed_rows: bool,
    /// Whether the table is empty because the Listed list hides rows whose answer is still on
    /// its way; see `Prepared::hides_waiting_rows`.
    listed_waiting: bool,
    /// Case-insensitive market or coin filter; a trailing space selects an exact ticker.
    coin_input: Entity<MoonInputState>,
    /// Minimum 24-hour volume filter, accepting `k` and `m` suffixes.
    dvol_input: Entity<MoonInputState>,
    /// Sort key from `COLS` and descending/ascending direction.
    sort_key: String,
    sort_desc: bool,
    /// Session-only core filter, not persisted, as in Orders. Empty means every core.
    sel_cores: HashSet<CoreId>,
    /// Session-only: the market kinds ticked in the Type list. Empty means every kind; `None`
    /// among them is an exchange this build cannot name.
    type_sel: HashSet<Option<MarketKind>>,
    /// Session-only: the answers ticked in the Listed list. Empty means every answer.
    listed_sel: HashSet<Listing>,
    /// Whether the exchange logos have been rasterized off the render path; the Exchange column
    /// draws captions alone until then.
    exchange_logos_ready: bool,
    /// Visible `COLS` keys persisted in per-context table storage.
    ///
    /// Legacy `layout.screener_columns` is read only as a migration fallback.
    visible_cols: HashSet<String>,
    /// Retained table state for drag order, column widths, and the sort indicator.
    table_state: Entity<MoonDataTableState>,
    /// Order this table last observed. A resize or a header click notifies the same observer
    /// as a drag, and must not write an order that did not move.
    col_order_cache: Vec<SharedString>,
    /// One trailing refresh coalesces feed events without losing their final publication.
    refresh_task: Option<Task<()>>,
    /// Session-only: the latest Calculate press's auto-figure answers (`screener::auto_figures`).
    scan_book: Rc<super::auto_figures::ScanBook>,
    /// The run a press started; replacing it stops the old one at its next slice.
    scan_task: Option<Task<()>>,
    /// Session-only: the auto-figures list's choice, and the approach distance box beside it.
    scan_choice: scan::ScanChoice,
    scan_pct_input: Entity<MoonInputState>,
    focus: FocusHandle,
}

impl ScreenerView {
    /// Retain control state while the first market publication is prepared off-thread.
    pub(crate) fn new(
        backend: Entity<Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let coin_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("BTC"));
        let dvol_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("500k"));
        for input in [&coin_input, &dvol_input] {
            cx.subscribe(input, |this: &mut Self, _, ev: &MoonInputEvent, cx| {
                let _stall = crate::diag::stall::span(
                    crate::diag::stall::StallKind::Input,
                    "screener:input",
                );
                if matches!(ev, MoonInputEvent::Change) {
                    this.rebuild(cx);
                    cx.notify();
                }
            })
            .detach();
        }

        // Read by the approach choice of the auto-figures list alone, so a change re-selects the
        // published rows and starts no publication.
        let scan_pct_input = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(scan::DEFAULT_APPROACH_PCT.to_string())
        });
        cx.subscribe(
            &scan_pct_input,
            |this: &mut Self, _, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    this.scan_pct_changed(cx);
                }
            },
        )
        .detach();

        // Screener always runs in a separate window, so its width context is fixed to `:win`.
        let widths_id = crate::persistence::table_persist::ctx_id("screener-table", true);

        // A stored visible set is an explicit list, so a column added later is absent from it and
        // would stay hidden for everyone who ever arranged their columns. Add it once, and record
        // that in the same document as the sets, so a column the user then hides stays hidden.
        backend.update(cx, |b, _| {
            // Each stored set gains only the columns introduced AFTER the version it records, so
            // a column offered by an earlier migration and hidden since stays hidden.
            let applied = b.layout.screener_columns_migration.unwrap_or(0);
            if applied >= SCREENER_COLUMNS_MIGRATION {
                return;
            }
            if let Some(keys) = b.layout.table_visible_columns.get_mut(&widths_id) {
                super::table::add_new_columns(keys, applied);
            }
            if let Some(keys) = b.layout.screener_columns.as_mut() {
                super::table::add_new_columns(keys, applied);
            }
            b.layout.screener_columns_migration = Some(SCREENER_COLUMNS_MIGRATION);
            // Unconditionally: the marker must reach disk even when no set needed rewriting.
            b.layout_dirty = true;
        });

        // Prefer the shared per-context `table_visible_columns` entry for `screener-table:win`.
        // When absent, migrate from legacy `screener_columns`. Ignore unknown keys left by renamed
        // or removed columns; an empty valid set falls back to every current column.
        //
        // A layout saved before the PnL column was renamed keys it `session`, a key that is now
        // retired; the list, the sort and the widths all bring it current on read (see
        // `table::current_key`).
        let saved_cols = crate::persistence::table_persist::visible(backend.read(cx), &widths_id)
            .or_else(|| backend.read(cx).layout.screener_columns.clone())
            .map(super::table::migrate_legacy_keys);
        let visible_cols: HashSet<String> = match saved_cols {
            Some(list) => {
                let set: HashSet<String> = list
                    .iter()
                    .filter(|k| COLS.iter().any(|c| c.0 == k.as_str()))
                    .cloned()
                    .collect();
                if set.is_empty() {
                    COLS.iter().map(|c| c.0.to_string()).collect()
                } else {
                    set
                }
            }
            None => COLS.iter().map(|c| c.0.to_string()).collect(),
        };
        let restored_sort = super::table::restore_sort(
            super::table::migrate_legacy_sort(crate::persistence::table_persist::saved_sort(
                backend.read(cx),
                &widths_id,
            )),
            &visible_cols,
        );
        let saved_widths = super::table::migrate_legacy_widths(
            crate::persistence::table_persist::saved(backend.read(cx), &widths_id),
        );
        let screener_keys: Vec<&str> = COLS.iter().map(|column| column.0).collect();
        let saved_order = crate::persistence::table_persist::restored_order(
            backend.read(cx),
            &widths_id,
            &screener_keys,
        );
        let col_order_cache = saved_order.clone();
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.set_sort(restored_sort.0.clone(), !restored_sort.1);
            s.column_widths = saved_widths;
            s.column_order = saved_order;
            s
        });
        // Column resizing and dragging mutate table state. Order uses its own helper so a panel
        // that already stores order elsewhere is not given a second copy. The cache skips a
        // notification that did not move the columns.
        let widths_id_obs = widths_id.clone();
        cx.observe(&table_state, move |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &widths_id_obs, &state, cx);
            crate::persistence::table_persist::persist_order(
                &this.backend,
                &widths_id_obs,
                &state,
                &mut this.col_order_cache,
                cx,
            );
        })
        .detach();

        // Both order/session changes and market-only updates affect the prepared rows.
        cx.observe(&backend, |this, _, cx| {
            this.request_refresh(cx);
        })
        .detach();
        let market_data_revision = backend.read(cx).market_data_revision();
        cx.observe(&market_data_revision, |this, _, cx| {
            this.request_refresh(cx);
        })
        .detach();
        let market_catalog_revision = backend.read(cx).market_catalog_revision();
        cx.observe(&market_catalog_revision, |this, _, cx| {
            this.request_refresh(cx);
        })
        .detach();

        // A membership-only save advances `workspace_revision` without notifying `Backend`, so the
        // gate above never fires for it; observe the revision directly to pick up the new preset.
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            this.rebuild(cx);
            cx.notify();
        })
        .detach();

        // Persist window position and size in the layout so the window reopens in the same geometry.
        cx.observe_window_bounds(window, |this, window, cx| {
            let geom = crate::window::windowing::window_geom_rect(window, cx);
            this.backend.update(cx, |b, _| {
                let geom = geom.keeping_display_of(b.layout.screener_window);
                if b.layout.screener_window != Some(geom) {
                    b.layout.screener_window = Some(geom);
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        // Rasterize the exchange logos off the render path, as every other logo-drawing surface
        // does; the Exchange column shows its captions alone until this lands.
        cx.spawn(async move |view, cx| {
            cx.background_spawn(async { crate::media::exchange_logos::prewarm() })
                .await;
            let _ = view.update(cx, |this, cx| {
                this.exchange_logos_ready = true;
                cx.notify();
            });
        })
        .detach();

        let mut this = Self {
            backend,
            rows: Rc::new(Vec::new()),
            prepared: Default::default(),
            prepare_task: None,
            prepare_seq: 0,
            order_counts: OrderCountMemo::default(),
            total: 0,
            shown: 0,
            scope_marker: None,
            sessions_total: 0,
            has_displayed_rows: false,
            listed_waiting: false,
            coin_input,
            dvol_input,
            sort_key: restored_sort.0,
            sort_desc: restored_sort.1,
            sel_cores: HashSet::new(),
            type_sel: HashSet::new(),
            listed_sel: HashSet::new(),
            exchange_logos_ready: false,
            visible_cols,
            table_state,
            col_order_cache,
            refresh_task: None,
            scan_book: Rc::default(),
            scan_task: None,
            scan_choice: scan::ScanChoice::Any,
            scan_pct_input,
            focus: cx.focus_handle(),
        };
        this.rebuild(cx);
        this
    }

    /// Publish the latest snapshot within one second, even if no further events arrive.
    fn request_refresh(&mut self, cx: &mut Context<Self>) {
        if self.refresh_task.is_some() {
            return;
        }
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            let _ = this.update(cx, |this, cx| this.rebuild(cx));
        }));
    }

    /// Expose admitted refreshes to the native event-routing regression probe.
    pub(crate) fn firetest_prepare_sequence(&self) -> u64 {
        self.prepare_seq
    }

    /// Prepare a coherent feed publication off-thread; the current controls remain immediate.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let _stall =
            crate::diag::stall::span(crate::diag::stall::StallKind::Input, "screener:rebuild");
        self.refresh_task = None;
        crate::diag::bump(&crate::diag::SCREENER_REBUILD);
        self.filter_cached(cx);
        let b = self.backend.read(cx);
        let source = b.session.market_source();
        let mut groups: Vec<(CoreId, Vec<CoreId>)> = Vec::new();
        let mut names = HashMap::new();
        let mut orders = HashMap::new();
        // Captions are resolved here, on the UI thread, where the session's venue map and the
        // translator are; the publication below runs in the background and only copies them.
        let mut venues: HashMap<CoreId, EntryVenue> = HashMap::new();
        let core_venues = b.session.core_venues();
        let store = b.session.store();
        self.order_counts
            .update_fleet_epoch(store.time_offset_epoch());
        for session in b.session.sessions() {
            names.insert(session.id, SharedString::from(session.name.clone()));
            if let Some(venue) = core_venues.get(&session.id).filter(|v| v.is_nameable()) {
                venues.insert(session.id, entry_venue(venue));
            }
            if let Some(data) = store.core(session.id) {
                orders.insert(
                    session.id,
                    self.order_counts
                        .counts(session.id, data.orders_table_rev, &data.orders),
                );
            }
            let Some(provider) = source.provider_of(session.id) else {
                continue;
            };
            match groups.iter_mut().find(|(id, _)| *id == provider) {
                Some((_, members)) => members.push(session.id),
                None => groups.push((provider, vec![session.id])),
            }
        }
        self.prepare_seq = self.prepare_seq.wrapping_add(1);
        let seq = self.prepare_seq;
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;
        self.prepare_task = Some(cx.spawn(async move |this, cx| {
            let prepared = cx
                .background_spawn(async move {
                    let mut entries = Vec::new();
                    for (provider, members) in groups {
                        let mut rows = source.screener_rows(provider, &members);
                        for row in &mut rows {
                            row.orders = orders
                                .get(&row.core)
                                .and_then(|counts| counts.get(row.market.as_str()))
                                .copied()
                                .unwrap_or(0);
                        }
                        entries.extend(entries_for(rows, &names));
                    }
                    for entry in &mut entries {
                        if let Some(venue) = venues.get(&entry.row.core) {
                            entry.venue = venue.clone();
                        }
                    }
                    let mut prepared =
                        super::prepared::Prepared::new(entries, &sort_key, sort_desc);
                    // Read after the rows were signed: the kept book is the one they were signed
                    // with, or a newer one, and either says when the waiting rows settle.
                    prepared.listing_settles_ms = source.listing_settles_ms();
                    std::sync::Arc::new(prepared)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.prepare_seq != seq {
                    return;
                }
                this.prepared = prepared;
                // The source snapshot is unfiltered: completion always applies the LATEST
                // source, scope, text, volume and sort, never the controls a job started with.
                this.filter_cached(cx);
                cx.notify();
            });
        }));
    }

    /// Filter a prepared publication using the live preset and controls without cloning rows.
    fn filter_cached(&mut self, cx: &mut Context<Self>) {
        let coin = CoinQuery::new(&self.coin_input.read(cx).value())
            .listed_in(&self.listed_sel)
            .with_scan(self.scan_query(cx));
        let min_vol = parse_vol(self.dvol_input.read(cx).value().trim());
        let b = self.backend.read(cx);
        let venues = b.session.core_venues();
        let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton);
        let sessions = b.session.sessions();
        self.sessions_total = sessions.len();
        let displayed: HashMap<_, _> = sessions
            .iter()
            .enumerate()
            .map(|(rank, session)| {
                (
                    session.id,
                    b.core_displayed(preset, session.id).then_some(rank),
                )
            })
            .collect();
        self.scope_marker = ScopeMarker::from_membership(
            preset,
            sessions
                .iter()
                .map(|session| displayed[&session.id].is_some()),
        );
        // Every core in the publication has rows, so one displayed core among them is enough.
        self.has_displayed_rows = self
            .prepared
            .cores
            .iter()
            .any(|core| displayed.get(core).copied().flatten().is_some());
        // A selected core the preset has since hidden, whose session is gone, or which the Type
        // list no longer keeps in the table can be neither shown nor unticked: drop it. A
        // selection emptied this way is every core again, which is what the single-choice
        // selector did when its one core went away. The drop is not undone when the Type tick
        // is removed, exactly as it is not when the preset shows the core again.
        let type_sel = &self.type_sel;
        self.sel_cores.retain(|core| {
            displayed.get(core).copied().flatten().is_some()
                && type_admits(type_sel, kind_of(venues.get(core)))
        });
        let allowed: Vec<_> = self
            .prepared
            .cores
            .iter()
            .map(|core| {
                displayed
                    .get(core)
                    .copied()
                    .flatten()
                    .filter(|_| self.source_admits(*core, venues))
            })
            .collect();
        let (rows, total, shown) =
            self.prepared
                .select(&allowed, &coin, min_vol, &self.sort_key, self.sort_desc);
        self.listed_waiting =
            rows.is_empty() && self.prepared.hides_waiting_rows(&allowed, &coin, min_vol);
        self.rows = Rc::new(rows);
        self.total = total;
        self.shown = shown;
    }

    /// Apply the header order immediately and refresh its source data for the next publication.
    fn set_sort(&mut self, key: &str, desc: bool, cx: &mut Context<Self>) {
        let next = super::table::restore_sort(
            Some(moon_core::config::TableSortPreference {
                column: key.to_string(),
                ascending: !desc,
            }),
            &self.visible_cols,
        );
        self.apply_sort(next, true, cx);
    }

    /// Apply `(key, descending)` as the table's order and rebuild; `store` writes it over the stored
    /// preference (never for the auto-figures column, whose answers are session-only).
    fn apply_sort(&mut self, next: (String, bool), store: bool, cx: &mut Context<Self>) {
        self.sort_key = next.0;
        self.sort_desc = next.1;
        let indicator_key = self.sort_key.clone();
        let indicator_ascending = !self.sort_desc;
        self.table_state.update(cx, move |state, _| {
            state.set_sort(indicator_key, indicator_ascending)
        });
        // The auto-figures column sorts by answers that live for this window's session only, so
        // its sort is not stored: the next open would sort by an empty column. The sort stored
        // before it stays, and comes back on reopen.
        if store && self.sort_key != super::auto_figures::AUTO_FIGURES_KEY {
            let preference = (self.sort_key != "vol24" || !self.sort_desc).then(|| {
                moon_core::config::TableSortPreference {
                    column: self.sort_key.clone(),
                    ascending: !self.sort_desc,
                }
            });
            let id = crate::persistence::table_persist::ctx_id("screener-table", true);
            crate::persistence::table_persist::set_sort(&self.backend, &id, preference, cx);
        }
        self.rebuild(cx);
        cx.notify();
    }

    /// Toggle one core in the session-only filter; `None` is the All row and clears it.
    fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        if crate::controls::toggle_core_selection(&mut self.sel_cores, id) {
            self.after_core_selection_change(cx);
        }
    }

    /// Toggle every displayed core of one exchange section in the session-only filter.
    ///
    /// Ids the menu was rendered with are checked against the cores the selector lists NOW, so a
    /// section captured before a preset or kind change cannot select a core the table no longer
    /// lists.
    fn toggle_exchange_cores(&mut self, exchange_cores: Vec<CoreId>, cx: &mut Context<Self>) {
        let available: HashSet<CoreId> = {
            let b = self.backend.read(cx);
            let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton);
            let venues = b.session.core_venues();
            b.session
                .sessions()
                .iter()
                .map(|session| session.id)
                .filter(|&core| b.core_displayed(preset, core) && self.kind_lists(core, venues))
                .collect()
        };
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.after_core_selection_change(cx);
        }
    }

    /// Whether the source selector and the Type list let `core`'s rows through; the display
    /// preset is a separate test.
    ///
    /// The one reading of both controls: the row filter and the row's coin menu, whose "N cores"
    /// targets cover the cores the table shows, both ask here so the two cannot drift apart. An
    /// empty selection is every core. The kind is the core's, not the market's — a core is
    /// connected to one market kind of one exchange — which is why the Type list is decided here,
    /// per core, rather than per row.
    ///
    /// Args:
    ///     core: The core whose rows are asked about.
    ///     venues: The session's venue map, for the core's market kind.
    pub(super) fn source_admits(&self, core: CoreId, venues: &HashMap<CoreId, CoreVenue>) -> bool {
        (self.sel_cores.is_empty() || self.sel_cores.contains(&core))
            && self.kind_lists(core, venues)
    }

    /// Whether the Type list keeps `core` in the table at all.
    ///
    /// Asked by the core selector too: a core the list has taken out of the table must not stay
    /// tickable there, or ticking it alone would empty the table with nothing on screen to say why.
    fn kind_lists(&self, core: CoreId, venues: &HashMap<CoreId, CoreVenue>) -> bool {
        type_admits(&self.type_sel, kind_of(venues.get(&core)))
    }

    /// Tick or untick one market kind in the Type list; `None` is the All row and clears it.
    ///
    /// Applied to the rows already published and nothing more: the list chooses among them and
    /// changes nothing a publication reads, so starting one — as the sort and the text boxes do,
    /// to resample the feed — would only re-read every core for the same rows.
    fn toggle_type(&mut self, choice: Option<Option<MarketKind>>, cx: &mut Context<Self>) {
        toggle_choice(&mut self.type_sel, choice, TYPE_CHOICES.len());
        self.filter_cached(cx);
        cx.notify();
    }

    /// Tick or untick one answer in the Listed list; `None` is the All row and clears it. Applied
    /// to the published rows, as the Type list is.
    fn toggle_listed(&mut self, choice: Option<Listing>, cx: &mut Context<Self>) {
        toggle_choice(&mut self.listed_sel, choice, LISTED_CHOICES.len());
        self.filter_cached(cx);
        cx.notify();
    }

    /// Toggle a column and persist the visible keys in canonical `COLS` order.
    ///
    /// The last visible column cannot be hidden because that would empty the table.
    fn toggle_col(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.visible_cols.contains(key) {
            if self.visible_cols.len() == 1 {
                return;
            }
            self.visible_cols.remove(key);
        } else {
            self.visible_cols.insert(key.to_string());
        }
        self.persist_visible_cols(cx);
        self.resort_if_sort_hidden(cx);
    }

    /// Move the sort off a column that is no longer visible ([`super::table::sort_after_hiding`]):
    /// back to the stored preference when it was the auto-figures column, to the usual fallback
    /// otherwise.
    fn resort_if_sort_hidden(&mut self, cx: &mut Context<Self>) {
        if self.visible_cols.contains(&self.sort_key) {
            return;
        }
        let id = crate::persistence::table_persist::ctx_id("screener-table", true);
        let stored = super::table::migrate_legacy_sort(
            crate::persistence::table_persist::saved_sort(self.backend.read(cx), &id),
        );
        let (next, store) = super::table::sort_after_hiding(
            &self.sort_key,
            self.sort_desc,
            stored,
            &self.visible_cols,
        );
        self.apply_sort(next, store, cx);
    }

    /// Toggle every column on, or retain only the first when all are already visible.
    ///
    /// Keeping one column prevents the table from becoming empty.
    fn toggle_all_cols(&mut self, cx: &mut Context<Self>) {
        let all_on = COLS.iter().all(|c| self.visible_cols.contains(c.0));
        self.visible_cols = if all_on {
            std::iter::once(COLS[0].0.to_string()).collect()
        } else {
            COLS.iter().map(|c| c.0.to_string()).collect()
        };
        self.persist_visible_cols(cx);
        self.resort_if_sort_hidden(cx);
    }

    /// Persist visible column keys in canonical `COLS` order.
    fn persist_visible_cols(&self, cx: &mut Context<Self>) {
        let list: Vec<String> = COLS
            .iter()
            .filter(|c| self.visible_cols.contains(c.0))
            .map(|c| c.0.to_string())
            .collect();
        // Screener has only the `screener-table:win` context because it is always a window. Store an
        // explicit key list, including the complete list when all columns are visible. Legacy
        // `screener_columns` is no longer written and is used only during read-side migration.
        let id = crate::persistence::table_persist::ctx_id("screener-table", true);
        crate::persistence::table_persist::set_visible(&self.backend, &id, list, cx);
        cx.notify();
    }

    /// Request the row's market on Main for the row's own core, as in an Orders token click.
    ///
    /// The request does not activate or raise the owning window.
    fn open_chart(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(e) = self
            .rows
            .get(ix)
            .and_then(|&index| self.prepared.entries.get(index))
        else {
            return;
        };
        let core = e.row.core;
        let market = e.row.market.clone();
        self.backend.update(cx, |b, bcx| {
            b.open_on_main((core, market), false);
            bcx.notify();
        });
    }

    /// Render each virtual row from the same publication that owns its selected index.
    fn table(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let rows = self.rows.clone();
        let row_count = rows.len();
        let view = cx.entity();
        let table_rows = rows.clone();
        let prepared = self.prepared.clone();
        let sort_view = cx.entity();
        let dbl_view = cx.entity();
        let state_reset = self.table_state.clone();
        // Share one canonically ordered visible-column list between the header and row renderer.
        let visible: Rc<Vec<&'static ColDef>> = Rc::new(
            COLS.iter()
                .filter(|c| self.visible_cols.contains(c.0))
                .collect(),
        );
        let row_cols = visible.clone();
        // Captions and logo geometry are the same for every row of this build, so they are
        // resolved once here rather than per visible row per frame.
        let kind_caption = |kind: MarketKind| SharedString::from(t!(kind.label_key()).to_string());
        let row_style = Rc::new(RowStyle {
            logos_ready: self.exchange_logos_ready,
            logo_px: design::ui_px(cx, 13.0),
            logo_gap: design::ui_px(cx, 5.0),
            kinds: [
                kind_caption(MarketKind::Spot),
                kind_caption(MarketKind::Futures),
                kind_caption(MarketKind::Quarterly),
            ],
            both: t!("screener.listed_both").to_string().into(),
            spot_only: t!("screener.listed_spot_only").to_string().into(),
            futures_only: t!("screener.listed_futures_only").to_string().into(),
            scan_book: self.scan_book.clone(),
            scan_labels: Self::scan_labels(),
            scan_running: self.scan_book.progress().is_some(),
        });
        let scan_title = self.scan_column_title();

        data_table_host(
            "screener-table-host",
            row_count == 0,
            // Four-way choice, and the order matters: `scope_empty_text` takes precedence over
            // every fallback, so a hidden-by-preset Screener never reaches a sentence below it. Of
            // the genuine reasons, only "no session at all" asks for an action — a core hidden by
            // the preset, or connected with no book yet, must not tell the user to go connect
            // something that is already connected. And when rows WERE published and the controls
            // of the bottom bar left none of them — the Type and Listed lists, a text or volume
            // filter — the table says exactly that instead of claiming no data arrived. Unless
            // the Listed list is what emptied it and its answers are still on their way: then
            // "no rows match" would be false in a minute, and the table says how long instead.
            scope_marker::scope_empty_text(
                self.scope_marker.as_ref(),
                if self.sessions_total == 0 {
                    t!("screener.empty").to_string()
                } else if let Some(text) = self.scan_empty_text() {
                    text
                } else if let Some(seconds) = self.listed_wait_seconds() {
                    t!("screener.listed_collecting", n = seconds).to_string()
                } else if self.has_displayed_rows {
                    t!("screener.empty_filtered").to_string()
                } else {
                    t!("screener.empty_no_data").to_string()
                },
            ),
            p,
            cx,
            MoonDataTable::new("screener-table", row_count, move |ix, _window, _app| {
                screener_row(
                    &prepared.entries[table_rows[ix]],
                    &view,
                    p,
                    &row_cols,
                    &row_style,
                )
            })
            .columns(
                visible
                    .iter()
                    .map(|&&(key, title, width, right)| {
                        let title = match key {
                            super::auto_figures::AUTO_FIGURES_KEY => scan_title.clone(),
                            _ => column_label(key, title),
                        };
                        let col = MoonDataTableColumn::new(key, title, width).sortable(true);
                        let col = if right { col.right() } else { col };
                        if key == "market" {
                            col.fixed_left()
                        } else {
                            col
                        }
                    })
                    .collect::<Vec<_>>(),
            )
            .state(&self.table_state)
            .style(design::table_style(p))
            .on_sort(move |key, ascending, _window, app| {
                let key = key.to_string();
                sort_view.update(app, |t, cx| t.set_sort(&key, !ascending, cx));
            })
            .on_double_click_row(move |ix, _window, app| {
                dbl_view.update(app, |t, cx| t.open_chart(ix, cx));
            })
            .on_select_row(move |_ix, _window, app| {
                // Row selection is unused, as in Orders, so clear every selection field immediately.
                state_reset.update(app, |s, c| {
                    s.selected_row = None;
                    s.selected_column = None;
                    s.selected_cell = None;
                    c.notify();
                });
            }),
        )
    }

    /// Build the core multi-selector over the cores the display preset shows.
    ///
    /// The shared picker Orders, Assets and Report host: an All row, a checkbox per core under its
    /// exchange, exchange rows that toggle their whole section, and the saved core groups. The
    /// Screener used to carry a single-choice list of its own, which could show every core or one
    /// and nothing in between.
    ///
    /// Args:
    ///     cx: Screener context used to read sessions, exchanges, and selection state.
    ///
    /// Returns:
    ///     The configured multi-select dropdown.
    fn source_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let (cores, venues) = {
            let b = self.backend.read(cx);
            let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton);
            let venues = b.session.core_venues();
            (
                // Only the cores the Type list keeps in the table: one it has taken out must
                // not stay tickable here.
                moon_core::session::core_order::CoreOrder::new(&b.config)
                    .from_sessions(b.session.sessions(), |s| {
                        b.core_displayed(preset, s.id) && self.kind_lists(s.id, venues)
                    }),
                venues,
            )
        };
        let view = cx.entity();
        let exchange_view = view.clone();
        let extras = crate::controls::core_combo_extras(true, &view, &self.backend, cx);
        crate::controls::core_combo(
            "screener-source",
            &cores,
            venues,
            &self.sel_cores,
            crate::controls::CoreAllRowMode::ImplicitOrComplete,
            t!("screener.all_cores").to_string(),
            |n| t!("screener.cores_n", n = n).to_string(),
            160.0,
            extras,
            cx,
            move |id, app| {
                view.update(app, |this, cx| this.toggle_core(id, cx));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |this, cx| {
                    this.toggle_exchange_cores(exchange_cores, cx);
                });
            },
        )
    }

    /// Build the Type list: a tick per market kind, narrowing the table to the ticked ones.
    ///
    /// The same shape as the core selector beside it — an All row, a tick per choice, the menu
    /// staying open — because it is the same gesture on the same bar. Coin-margined contracts are
    /// a choice of their own, as they are a value of their own in the Type column, and so is an
    /// exchange this build cannot name: with only named kinds on offer, ticking one would drop
    /// those rows with no way to ask for them back short of "all".
    fn type_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let choices = TYPE_CHOICES.map(|kind| {
            let label = match kind {
                Some(kind) => t!(kind.label_key()).to_string(),
                None => UNKNOWN_CHOICE.to_string(),
            };
            (kind, label)
        });
        let view = cx.entity();
        choice_menu(
            "screener-type",
            column_title("type"),
            &choices,
            &self.type_sel,
            cx,
            move |choice, app| view.update(app, |this, cx| this.toggle_type(choice, cx)),
        )
    }

    /// Seconds until the Listed answers can exist, while the table is empty because the Listed
    /// list hides rows that are still waiting for theirs; `None` otherwise.
    ///
    /// A list narrowed to `Futures Only` in the first minute after the cores connect has nothing
    /// to show yet, and that is a wait, not "no such coins". Whether it IS that case is decided
    /// with the rows, in [`Self::filter_cached`]: a table emptied by anything else is just empty.
    fn listed_wait_seconds(&self) -> Option<i64> {
        self.listed_waiting.then(|| {
            collecting_seconds(
                self.prepared.listing_settles_ms,
                moon_core::util::time::now_unix_ms_i64(),
            )
        })
    }

    /// Build the Listed list: a tick per answer of the Listed column.
    ///
    /// "Only the one-sided coins" is two ticks here, `Spot Only` and `Futures Only`; unknown is a
    /// choice like the others, so the rows nothing could answer for can be looked at on their own.
    /// A row whose answer is still on its way has no choice of its own and goes with unknown.
    fn listed_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let choices = LISTED_CHOICES.map(|listing| {
            let label = match listing {
                Listing::Both => t!("screener.listed_both").to_string(),
                Listing::SpotOnly => t!("screener.listed_spot_only").to_string(),
                Listing::FuturesOnly => t!("screener.listed_futures_only").to_string(),
                Listing::Unknown | Listing::Pending => UNKNOWN_CHOICE.to_string(),
            };
            (listing, label)
        });
        let view = cx.entity();
        choice_menu(
            "screener-listed",
            column_title("listed"),
            &choices,
            &self.listed_sel,
            cx,
            move |choice, app| view.update(app, |this, cx| this.toggle_listed(choice, cx)),
        )
    }

    /// Build the persistent visible-column menu with checkbox-style toggles.
    ///
    /// The menu remains open after clicks, and its last visible column is disabled so the table
    /// cannot be emptied.
    fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let visible_count = self.visible_cols.len();
        let mut menu = MoonDropdown::new("screener-columns")
            // The shared column-selector icon button instead of a text-field trigger; the asset
            // and the childless trigger are `design::COLUMN_SELECTOR_ICON`'s contract.
            .trigger_icon(design::COLUMN_SELECTOR_ICON)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_width(design::glyph_btn_w(cx))
            .fit_menu_width(170.0, 560.0)
            .close_on_select(false);
        // The All item enables every column or, when already checked, leaves only the first.
        let all_on = COLS.iter().all(|c| self.visible_cols.contains(c.0));
        let all_view = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| {
                    all_view.update(app, |t, cx| t.toggle_all_cols(cx));
                }),
        );
        for &(key, title, ..) in COLS {
            let shown = self.visible_cols.contains(key);
            let last_visible = shown && visible_count == 1;
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("col-{key}"), column_label(key, title))
                    .checked(shown)
                    .disabled(last_visible)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, cx| t.toggle_col(key, cx));
                    }),
            );
        }
        div()
            .id("screener-cols-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("screener.columns").to_string()))
                    .into()
            })
            .child(menu)
    }

    /// Build the bottom bar with Moonbot-style source, Market, and Vol. controls on the left and the
    /// filtered/total count plus column menu on the right. Exact ticker mode shows `=`.
    fn bottom_bar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let label = |text: &'static str| {
            div()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text_soft))
                .child(text)
        };
        // Frozen render idiom (`workspace::scope_marker`): head is the existing formatted count
        // and never clips; the marker's facts trail it, clipping at their right edge when the
        // window narrows, exactly as a dock footer's fixed height requires.
        let count_head = format!("{} / {} {}", self.shown, self.total, t!("screener.coins"));
        let footer = scope_marker::scope_footer(count_head, self.scope_marker.as_ref());
        let footer_tip = scope_marker::scope_footer_tooltip(&footer, self.scope_marker.as_ref());
        let has_tail = !footer.tail.is_empty();
        // The shared helper draws nothing when nothing is hidden, which is what keeps this row's
        // child count, gaps and shrink behaviour identical to its pre-feature form.
        let footer_tail = scope_marker::scope_footer_tail(
            "screener-footer-tail",
            footer.tail,
            footer_tip,
            design::t_body(cx),
            p.text_muted,
        );
        // Facet skin: the bar paints no fill, so the window backdrop runs under it, and its top
        // edge is a rule that fades toward the window edges.
        let facet = design::is_facet(cx);
        h_flex()
            .flex_none()
            .w_full()
            .h(design::fit_h_px(cx, 30.0, 14.0, 9.0))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .when(!facet, |this| this.bg(moon(p.shell_high)))
            .border_t(px(1.0))
            .border_color(moon_alpha(p.border, 1.0))
            .when(facet, |this| {
                design::facet_top_rule(this, design::facet_ink(p, design::FACET_CHROME_RULE))
            })
            .child(self.source_combo(cx))
            .child(self.type_menu(cx))
            .child(self.listed_menu(cx))
            .child(label(column_title("market")))
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("scr-coin")
                        .state(&self.coin_input)
                        .size(design::INPUT_SIZE)
                        .cleanable(true),
                ),
            )
            .when(
                moon_core::db::report_coin_is_exact(&self.coin_input.read(cx).value()),
                |row| {
                    row.child(
                        div()
                            .flex_none()
                            .font_family(design::mono())
                            .text_color(rgb(p.accent))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("="),
                    )
                },
            )
            .child(label(column_title("vol24")))
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("scr-dvol")
                        .state(&self.dvol_input)
                        .size(design::INPUT_SIZE)
                        .cleanable(true),
                ),
            )
            .child(self.scan_controls(p, cx))
            .child(div().flex_1())
            // Head and tail are DIRECT children of the row, never nested in one shared wrapper —
            // nested, the wrapper would take a zero flex basis while the `flex_none` head keeps
            // painting at full width, straight over the tail.
            .child(
                div()
                    // `flex_none` only while a tail exists to yield in its place. Pinning the count
                    // unconditionally would take space from this bar's inputs at a narrow width on
                    // behalf of a marker that is not on screen.
                    .when(has_tail, |el| el.flex_none())
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(footer.head),
            )
            .children(footer_tail)
            .child(self.columns_menu(cx))
    }
}

impl EventEmitter<()> for ScreenerView {}
impl Focusable for ScreenerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ScreenerView {
    /// Render this window with native minimize, maximize, and close controls.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = crate::window::windowing::responsive_width(window);
        v_flex()
            .size_full()
            .relative()
            // Under the Facet skin `MoonRoot` paints the window backdrop behind this view, and an
            // opaque fill on the window root would cover it.
            .when(!design::is_facet(cx), |this| this.bg(moon(p.shell)))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .child(screener_header(p, cx))
            .child(self.table(cx))
            .child(self.bottom_bar(p, cx))
            .child(
                MoonWindowFrame::tool("screener-window-frame-hit", chrome_width)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(SCREENER_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

/// Whether the Type list lets a market kind through: every kind while nothing is ticked.
fn type_admits(selected: &HashSet<Option<MarketKind>>, kind: Option<MarketKind>) -> bool {
    selected.is_empty() || selected.contains(&kind)
}

/// Tick or untick one choice of a list whose empty selection means "all".
///
/// Ticking the last unticked choice collapses back to the empty selection: every choice ticked
/// IS "all", and keeping it as an explicit full set would leave the All row unticked over a table
/// that shows everything.
///
/// Args:
///     selected: The ticked choices; empty for all.
///     choice: The choice clicked, or `None` for the All row, which clears the selection.
///     universe: How many choices the list offers.
pub(super) fn toggle_choice<T: Copy + Eq + std::hash::Hash>(
    selected: &mut HashSet<T>,
    choice: Option<T>,
    universe: usize,
) {
    let Some(choice) = choice else {
        selected.clear();
        return;
    };
    if !selected.remove(&choice) {
        selected.insert(choice);
    }
    if selected.len() >= universe {
        selected.clear();
    }
}

/// What a list's button reads: its column title, then "all", the one ticked choice, or how many.
///
/// Args:
///     title: The column the list filters, as its header spells it.
///     all: The word for an empty selection.
///     choices: Every choice with its caption, in menu order.
///     selected: The ticked choices.
pub(super) fn choice_summary<T: Copy + Eq + std::hash::Hash>(
    title: &str,
    all: &str,
    choices: &[(T, String)],
    selected: &HashSet<T>,
) -> String {
    let mut ticked = choices
        .iter()
        .filter(|(choice, _)| selected.contains(choice));
    match (ticked.next(), selected.len()) {
        (None, _) => choice_caption(title, all),
        (Some((_, label)), 1) => choice_caption(title, label),
        (Some(_), count) => choice_caption(title, &count.to_string()),
    }
}

/// One caption of a list's button: the column title, then what the list is narrowed to.
///
/// The title stays on the button because two of these lists stand side by side: a bare "All"
/// on each would not say which column it speaks for.
fn choice_caption(title: &str, value: &str) -> String {
    format!("{title}: {value}")
}

/// Every caption a list's button can show, so the button is sized once for the longest of them.
///
/// The counts run from two to one short of every choice: one tick prints its caption, and every
/// choice ticked collapses to "all" (see [`toggle_choice`]).
///
/// Args:
///     title: The column the list filters, as its header spells it.
///     all: The word for an empty selection.
///     choices: Every choice with its caption, in menu order.
pub(super) fn choice_captions<T>(title: &str, all: &str, choices: &[(T, String)]) -> Vec<String> {
    let counts = (2..choices.len()).map(|count| count.to_string());
    std::iter::once(all.to_string())
        .chain(choices.iter().map(|(_, label)| label.clone()))
        .chain(counts)
        .map(|value| choice_caption(title, &value))
        .collect()
}

/// Design-unit width a list's button never goes below, however short its captions are.
const CHOICE_TRIGGER_MIN_W: f32 = 64.0;
/// Design-unit bounds of a list's open menu: room for the longest answer, never a banner.
const CHOICE_MENU_W: (f32, f32) = (130.0, 320.0);

/// One multi-select list of the bottom bar: an All row and a tick per choice, staying open.
///
/// Built on the same dropdown and the same row idiom as the core selector and the column menu,
/// so the three read as one family. Like the core selector's, the button keeps ONE width whatever
/// is ticked — here the width of its longest caption: the menu stays open across clicks, and a
/// button that resized with each tick would move the open menu and every control after it.
///
/// Args:
///     id: Element id of the dropdown; rows are keyed under it.
///     title: The column the list filters, shown on its button.
///     choices: Every choice with its caption, in menu order.
///     selected: The ticked choices; empty for all.
///     cx: Application context, for the button's density.
///     on_toggle: Called with the clicked choice, or `None` for the All row.
fn choice_menu<T, F>(
    id: &'static str,
    title: &str,
    choices: &[(T, String)],
    selected: &HashSet<T>,
    cx: &App,
    on_toggle: F,
) -> MoonDropdown
where
    T: Copy + Eq + std::hash::Hash + 'static,
    F: Fn(Option<T>, &mut App) + Clone + 'static,
{
    let all_label = t!("report.filter.all").to_string();
    let all_on = selected.is_empty();
    let toggle_all = on_toggle.clone();
    let size = MoonButtonSize::density(cx);
    let captions = choice_captions(title, &all_label, choices);
    // Both bounds are the one width, measured by the component's own fitting function.
    let trigger_w = crate::controls::trigger_fit::widest_design(
        cx,
        captions.iter().map(String::as_str),
        size,
        CHOICE_TRIGGER_MIN_W,
    );
    let mut menu = MoonDropdown::new(id)
        .label(choice_summary(title, &all_label, choices, selected))
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(size)
        .fit_trigger_width(trigger_w, trigger_w)
        .fit_menu_width(CHOICE_MENU_W.0, CHOICE_MENU_W.1)
        .close_on_select(false)
        .item(
            MoonMenuItem::with_key(format!("{id}-all"), all_label)
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| toggle_all(None, app)),
        );
    for (index, (choice, label)) in choices.iter().enumerate() {
        let on = selected.contains(choice);
        let toggle = on_toggle.clone();
        let choice = *choice;
        menu = menu.item(
            MoonMenuItem::with_key(format!("{id}-{index}"), label.clone())
                .checked(on)
                .selected(on)
                .on_click(move |_, _, app| toggle(Some(choice), app)),
        );
    }
    menu
}

/// The bit one Listed answer occupies in [`CoinQuery`]'s mask.
///
/// An answer still on its way shares the bit of unknown: neither is an answer, the list has one
/// tick for "no answer", and a waiting row must not vanish from under it for a minute.
fn listing_bit(listing: Listing) -> u8 {
    match listing {
        Listing::Both => 1,
        Listing::SpotOnly => 2,
        Listing::FuturesOnly => 4,
        Listing::Unknown | Listing::Pending => 8,
    }
}

/// Whole seconds left until the waiting Listed answers settle, never under one.
///
/// The line this feeds is drawn only while some row is still waiting, and the book that settles
/// them lands a few seconds after the moment it names: "0 s left" over a table still waiting
/// would be wrong about exactly those seconds.
///
/// Args:
///     settles_ms: When the last answer is expected, in Unix milliseconds.
///     now_ms: Current Unix time in milliseconds, on the same clock.
pub(super) fn collecting_seconds(settles_ms: Option<i64>, now_ms: i64) -> i64 {
    settles_ms.map_or(1, |settles| {
        (settles.saturating_sub(now_ms).saturating_add(999) / 1000).max(1)
    })
}

/// The market kind of the venue a core is connected to, or `None` for one this build cannot name.
fn kind_of(venue: Option<&CoreVenue>) -> Option<MarketKind> {
    venue
        .and_then(CoreVenue::resolved)
        .map(|resolved| resolved.kind)
}

/// What the table shows about one core's venue: the caption every core list prints, its brand's
/// logo, and its market kind.
fn entry_venue(venue: &CoreVenue) -> EntryVenue {
    EntryVenue {
        label: crate::controls::venue_label(venue).into(),
        brand: venue.brand(),
        kind: kind_of(Some(venue)),
    }
}

/// Normalize a provider's markets once while preserving member order and account identity.
fn entries_for(
    rows: Vec<moon_core::market::SharedScreenerRow>,
    names: &HashMap<CoreId, SharedString>,
) -> Vec<Entry> {
    use std::sync::Arc;
    let mut search_text: HashMap<usize, (Arc<str>, Arc<str>)> = HashMap::new();
    rows.into_iter()
        .map(|row| {
            let (market_uc, coin_uc) = search_text
                .entry(Arc::as_ptr(&row.market_data) as usize)
                .or_insert_with(|| {
                    (
                        row.market.to_uppercase().into(),
                        row.coin.to_uppercase().into(),
                    )
                })
                .clone();
            let core_name = names
                .get(&row.core)
                .cloned()
                .unwrap_or_else(|| SharedString::from(format!("#{}", row.core)));
            Entry {
                row,
                core_name,
                market_uc,
                coin_uc,
                // Attached by the caller, which holds the session's venue map; a row built here
                // alone belongs to a core nobody has identified.
                venue: EntryVenue::default(),
            }
        })
        .collect()
}

/// The Market box text in both spellings it can legitimately mean, uppercased, with the
/// exact-mode trailing space kept.
///
/// Why two: a Latin ticker typed on the Russian layout (`ыиук`) means `SBER`, while a coin whose
/// real name is Cyrillic (`ОФЗ 26254` on a T-Invest core) is only reachable by the text as typed.
/// Folding alone made every Cyrillic name unfindable (#1303).
///
/// It also carries the other per-ROW tests of the bottom bar, the Listed list and the
/// auto-figures filter, so the selection pass takes the row-level controls as one value. The Type
/// list is not here: it is decided per core, with the source selector.
pub(super) struct CoinQuery {
    /// The query as typed, uppercased.
    typed: String,
    /// The Russian-layout keys mapped to Latin, uppercased; `None` when that changes nothing.
    folded: Option<String>,
    /// The Listed answers a row may carry, one bit each; zero for every answer.
    listed: u8,
    /// The auto-figures filter with the answers it reads, and what the column sorts by; `None`
    /// passes every row and leaves the column unsorted.
    scan: Option<ScanQuery>,
}

impl CoinQuery {
    /// Builds both spellings from the raw box text, with every Listed answer admitted.
    pub(super) fn new(raw: &str) -> Self {
        let typed = raw.to_uppercase();
        let folded = crate::controls::coin_search::normalize_layout(raw).to_uppercase();
        let folded = (folded != typed).then_some(folded);
        Self {
            typed,
            folded,
            listed: 0,
            scan: None,
        }
    }

    /// Narrows the rows by the auto-figures filter, which also supplies the column's sort.
    pub(super) fn with_scan(mut self, scan: ScanQuery) -> Self {
        self.scan = Some(scan);
        self
    }

    /// Whether a row passes the auto-figures filter (`ScanQuery::admits`); every row without one.
    pub(super) fn admits_scan(&self, entry: &Entry) -> bool {
        self.scan.as_ref().is_none_or(|scan| scan.admits(entry))
    }

    /// Sort `order` by the auto-figures column; left as it is without a filter to read from.
    pub(super) fn sort_by_scan(&self, entries: &[Entry], order: &mut [usize], desc: bool) {
        if let Some(scan) = &self.scan {
            scan.sort(entries, order, desc);
        }
    }

    /// Narrows the rows to the ticked Listed answers; an empty selection is every answer.
    pub(super) fn listed_in(mut self, selected: &HashSet<Listing>) -> Self {
        self.listed = selected
            .iter()
            .fold(0, |mask, listing| mask | listing_bit(*listing));
        self
    }

    /// Whether a row passes the Listed list: every row while nothing is ticked, and otherwise
    /// only a row carrying one of the ticked answers.
    pub(super) fn admits_listing(&self, listing: Listing) -> bool {
        self.listed == 0 || self.listed & listing_bit(listing) != 0
    }

    /// True when the box is empty, so no row is filtered out BY ITS TEXT; the Listed list is
    /// asked separately, through [`Self::admits_listing`].
    pub(super) fn is_empty(&self) -> bool {
        self.typed.is_empty()
    }
}

/// Match either spelling of the query, admitting only the ticker and its `_` tails in exact mode.
pub(super) fn matches_coin_filter(entry: &Entry, query: &CoinQuery) -> bool {
    matches_one_spelling(entry, &query.typed)
        || query
            .folded
            .as_deref()
            .is_some_and(|folded| matches_one_spelling(entry, folded))
}

/// Match one uppercased spelling against the market, the coin and the coin's identity.
///
/// The identity is what makes the exact form useful across exchanges: `BONK ` finds Bybit's
/// `1kBONK` and Binance's `1000BONK`, which neither the market nor the token spells exactly.
fn matches_one_spelling(entry: &Entry, filter_uc: &str) -> bool {
    let exact = moon_core::db::report_coin_is_exact(filter_uc);
    let query = filter_uc.trim();
    let matches = |text: &str| {
        if exact {
            text == query
                || text
                    .strip_prefix(query)
                    .is_some_and(|tail| tail.starts_with('_'))
        } else {
            text.contains(query)
        }
    };
    matches(&entry.market_uc) || matches(&entry.coin_uc) || matches(&entry.row.identity)
}

#[cfg(test)]
pub(super) mod tests;

/// Build the window header with native minimize, maximize, and close controls.
///
/// Under the Facet skin the header paints no fill of its own, so the window backdrop's top light
/// shows through it, and its bottom border becomes a rule that fades toward the window edges.
fn screener_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    let facet = design::is_facet(cx);
    h_flex()
        .id("screener-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, SCREENER_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .when(!facet, |this| this.bg(moon(p.shell_high)))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .when(facet, |this| {
            design::facet_bottom_rule(this, design::facet_ink(p, design::FACET_CHROME_RULE))
        })
        .child(
            MoonWindowFrame::tool("screener-titlebar-title", 0.0)
                .title_cluster(t!("screener.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("screener-window-frame-visual", 0.0)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(SCREENER_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Open the singleton Screener tool window, focusing the existing backend-tracked handle when valid.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    // Focus an existing live window instead of opening a duplicate.
    if let Some(handle) = backend.read(cx).screener_window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let saved = backend.read(cx).layout.screener_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(100.0), px(80.0)),
            size: size(px(1500.0), px(760.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Choose a display from saved geometry when supported, otherwise fall back to the owner display.
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.and_then(|g| g.display_uuid),
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let title = |_: &App| t!("screener.window_title").to_string();
    let mut opts = crate::window::windowing::tool_window_options(
        title(cx),
        crate::window::windowing::restored_window_bounds(
            saved,
            crate::window::windowing::reachable_window_bounds(bounds, display_id, None, cx),
        ),
        Some(size(px(900.0), px(480.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    let open_span = crate::diag::stall::span(crate::diag::stall::StallKind::Open, "screener");
    let opened = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| ScreenerView::new(b, window, cx));
        cx.new(|cx| {
            crate::window::windowing::frame(
                Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque),
            )
        })
    });
    drop(open_span);
    if let Ok(handle) = opened {
        backend.update(cx, |bk, _| bk.screener_window = Some(handle));
        crate::window::titles::track(handle.into(), owner.is_some(), title, cx);
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}
