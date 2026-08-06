//! Alerts panel: the whole drawing layer of a group's cores in one sortable table.
//!
//! It lists EVERY figure, not only the armed ones — a line drawn here, a Fibonacci drawn in
//! Moonbot, a rectangle no core will ever know about — because the panel is where a figure is
//! turned INTO an alert. The Alert column is a checkbox, so arming happens beside the figure being
//! armed rather than only through a hotkey over the chart, and the Kind column says at a glance
//! which figures Moonbot has a type for — those are the ones an alert can be put on at all.
//!
//! The layout follows the other tables of this product on purpose: the multi-select core filter,
//! the scope field and the coin search sit in a toolbar at the TOP, `MoonDataTable` sorts and
//! resizes the columns underneath, and the bottom bar keeps the alert-playback controls that belong
//! to the panel as a whole rather than to any row.
//!
//! Responsibilities split across [`model`] for the row, columns, filters and sort — plain data,
//! testable without a window — [`controls`] for the toolbar and bottom bar, and [`table`] for the
//! table and its cells.
//!
//! Detect playback uses the strategy sound when present; `detect_sound` uses the selected default
//! for an alert firing without a strategy sound.

mod controls;
mod model;
mod table;

use model::{AlCol, AlertsViewState, ArmFilter, FigRow, KindFilter, sort_rows};

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonDataTableState, MoonInputEvent, MoonInputState, MoonPalette, Panel, PanelEvent,
    PanelInfo, PanelState, v_flex,
};

use moon_core::session::CoreId;

use crate::core_order::{CoreOrder, OrderedCores};
use crate::design::moon;
use crate::panels::RenderGate;
use crate::{Backend, design};

/// MoonProto ordinal for the `Alerts` strategy kind, the only kind assignable here.
///
/// This corresponds to `StrategyKindId::ALERTS = 22`.
const ALERTS_KIND: u8 = 22;

/// Base identity for the per-context column widths and visible-column set.
const TABLE_ID: &str = "alerts-table";

/// What the Strategy column offers for one core: `(strategy id, element key, label)`, with the
/// unassigned placeholder at index 0.
pub(super) type StrategyOptions = Rc<Vec<(u64, SharedString, SharedString)>>;

/// Label of the unassigned strategy, and of one whose core no longer lists it.
const NO_STRATEGY: &str = "—";

/// Panel listing every figure drawn on this group's cores.
pub struct AlertsPanel {
    backend: Entity<Backend>,
    group: String,
    /// Filters, sort and visible columns — everything persisted about the view.
    view: AlertsViewState,
    /// Multi-select core filter; an empty set means every core in the group.
    sel_cores: HashSet<CoreId>,
    /// Built rows, already filtered and sorted. `Rc` because the table's row callback outlives the
    /// borrow that built them.
    rows: Rc<Vec<FigRow>>,
    /// Signature the current rows were built from. A CHANGE to it rebuilds at once, without asking
    /// the gate below: that gate's 250 ms floor is there to thin idle repaints, not to delay a
    /// figure that actually moved.
    rows_sig: Option<u64>,
    /// Periodic self-heal for everything the signature cannot see. See [`Self::data_sig`].
    gate: RenderGate,
    coin_input: Entity<MoonInputState>,
    /// Mirror of the coin field, so a rebuild needs no context to read the input.
    query: String,
    /// Each core's assignable `Alerts` strategies, already in the shape a dropdown wants: the
    /// unassigned placeholder first, then `(id, element key, label)` per strategy.
    ///
    /// Built once per rebuild rather than per row: the strategy cell runs for every visible row on
    /// every repaint, and formatting a key and cloning a name per strategy there is the panel's
    /// single largest per-frame cost. Shared by refcount all the way down, so a row clones handles.
    alert_strategies: Rc<HashMap<CoreId, StrategyOptions>>,
    /// The figure whose settings popover is open, if any. Owned here rather than left to each
    /// popover's own state so only ONE can be open, and so the table can mark that row.
    pub(super) settings_for: Option<crate::figstyle::FigStyleTarget>,
    duration_s: u32,
    repeat: u32,
    /// Retained table state holding column order and widths.
    table_state: Entity<MoonDataTableState>,
    /// Last persisted column order, so a resize or selection notification does not dump the dock
    /// unless the order actually changed.
    col_order_cache: Vec<SharedString>,
    /// Context-qualified column-storage ID: `alerts-table:dock` or `alerts-table:win`.
    widths_id: String,
    /// Dock area used by the detach button, assigned in `on_added_to`.
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl AlertsPanel {
    /// Builds a panel with a fresh, default view.
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(backend, group, None, window, cx)
    }

    /// Reconstructs the panel from `docks.json`, replaying the persisted view state.
    pub fn restored(
        backend: Entity<Backend>,
        group: String,
        info: &PanelInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(backend, group, Some(info), window, cx)
    }

    /// The one constructor both entry points end in.
    ///
    /// Written as one function rather than as `restored` calling `new` and fixing it up, because
    /// that shape rebuilt the whole row set twice on every restored panel — once for the default
    /// view and again for the saved one — and each rebuild takes the market-source lock per core.
    fn build(
        backend: Entity<Backend>,
        group: String,
        info: Option<&PanelInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let coin_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("BTC, ETH"));
        cx.subscribe(
            &coin_input,
            |this: &mut Self, input, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    // Mirror the field so every later rebuild reads a plain string; the rebuild
                    // itself runs from the backend observer, which has no input to read. NOT
                    // uppercased: the matcher folds both sides itself, and folding one side here
                    // would turn a non-ASCII query into bytes the raw market name can never match.
                    this.query = input.read(cx).value().trim().to_string();
                    this.refresh(cx);
                }
            },
        )
        .detach();
        // Rebuild only when the data behind the rows actually changed. The backend notifies on
        // every drain — several times a second — and nothing about a figure list changes that often.
        cx.observe(&backend, |this, backend, cx| {
            // Two independent reasons to rebuild, exactly as Orders has them: the data behind the
            // rows changed, or the gate's periodic tick came round for everything the signature
            // cannot see. The first must NOT go through the gate — its 250 ms floor would swallow a
            // figure that really moved and leave it unshown until some later drain.
            let rebuild = {
                let b = backend.read(cx);
                let sig = this.data_sig(b);
                let changed = this.rows_sig != Some(sig);
                let due = this
                    .gate
                    .should_notify(sig, moon_chart::paint::now_unix_ms());
                changed || due
            };
            if rebuild {
                this.refresh(cx);
            }
        })
        .detach();
        let widths_id = crate::persistence::table_persist::ctx_id(TABLE_ID, false);
        let saved_widths =
            floored_widths(crate::persistence::table_persist::saved(backend.read(cx), &widths_id));
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
            // Compared against the borrow; the clone happens only when it really moved, and this
            // fires on every frame of a width drag.
            if state.read(cx).column_order != this.col_order_cache {
                this.col_order_cache = state.read(cx).column_order.clone();
                let view = cx.entity();
                cx.defer(move |app| Self::persist(&view, app));
            }
        })
        .detach();
        let mut this = Self {
            backend,
            group,
            view: info.map(view_from_info).unwrap_or_default(),
            sel_cores: HashSet::new(),
            rows: Rc::new(Vec::new()),
            rows_sig: None,
            gate: RenderGate::default(),
            coin_input,
            query: String::new(),
            alert_strategies: Rc::new(HashMap::new()),
            settings_for: None,
            duration_s: 20,
            repeat: 2,
            table_state,
            col_order_cache: Vec::new(),
            widths_id,
            dock: None,
            focus: cx.focus_handle(),
        };
        // A per-context visible-column set in shared storage outranks the one in `docks.json`.
        this.apply_ctx_columns(cx);
        let order = info.map(column_order_from_info).unwrap_or_default();
        if !order.is_empty() {
            this.col_order_cache = order.clone();
            this.table_state.update(cx, |s, _| s.column_order = order);
        }
        this.reload(cx);
        this.sync_table_sort(cx);
        this
    }

    /// Everything the rows are derived from, folded into one number.
    ///
    /// The figure store's own revision counter is the authoritative flag for "a figure changed" —
    /// it is bumped by every add, edit, delete and server reconcile — so the panel reads that
    /// rather than diffing figures itself. Beside it: the group's cores (a name change retitles a
    /// row, a disconnect greys its Alert box), and the cores' `Alerts` strategies, which the
    /// Strategy column resolves names from and the row dropdown offers.
    ///
    /// It is a HINT, not the whole truth, and that is why the caller feeds it to the shared
    /// [`RenderGate`] rather than comparing it directly. A row also carries values this fold does
    /// not reach — the market catalogue behind the coin, the locale behind the tool's name — and a
    /// strict signature would freeze every one of them until an unrelated figure was edited. The
    /// gate's one-second bucket is what makes those self-heal, exactly as it does for Orders.
    fn data_sig(&self, b: &Backend) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b.figures.borrow().rev().hash(&mut h);
        for s in b.session.sessions().iter().filter(|s| s.group == self.group) {
            s.id.hash(&mut h);
            s.name.hash(&mut h);
            if let Some(core) = b.session.store().core(s.id) {
                // Connection state decides whether a row's Alert box may be ticked at all, so it
                // belongs to the fold as much as the names do.
                core_is_ready(&core.status).hash(&mut h);
                // The core's OWN counter for its strategy set, rather than a walk that re-derives
                // it: this runs on every backend drain, per open panel, and the list can hold
                // thousands of strategies on a busy core.
                core.strategies_rev.hash(&mut h);
            }
        }
        h.finish()
    }

    /// Rebuilds the row set from the store, applying both filters, the coin query and the sort.
    fn rebuild(&mut self, b: &Backend) {
        // Limit the panel to cores in its own group, then to the core filter.
        let names: HashMap<CoreId, String> = b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .filter(|s| self.sel_cores.is_empty() || self.sel_cores.contains(&s.id))
            .map(|s| (s.id, s.name.clone()))
            .collect();
        // Each group core's `Alerts` strategies, for the per-row assignment dropdown and for the
        // names the Strategy column sorts by — and whether the core can be commanded at all.
        let mut strategies: HashMap<CoreId, StrategyOptions> = HashMap::new();
        let mut online: HashMap<CoreId, bool> = HashMap::new();
        for id in names.keys().copied() {
            // The placeholder leads every list, so a row that offers "no strategy" does not have to
            // prepend it — and so a core with no strategies, or one the store holds no data for at
            // all, still gets a dropdown that can CLEAR an assignment instead of an empty menu.
            let mut list: Vec<(u64, SharedString, SharedString)> = vec![(
                0,
                SharedString::from("strat-none"),
                SharedString::from(NO_STRATEGY),
            )];
            if let Some(core) = b.session.store().core(id) {
                online.insert(id, core_is_ready(&core.status));
                list.extend(
                    core.strategies
                        .iter()
                        .filter(|s| s.kind_ordinal == ALERTS_KIND)
                        .map(|s| {
                            (
                                s.id,
                                SharedString::from(format!("strat-{}", s.id)),
                                SharedString::from(s.name.clone()),
                            )
                        }),
                );
            }
            strategies.insert(id, Rc::new(list));
        }
        self.alert_strategies = Rc::new(strategies);
        // Collect the figures first, releasing the `figures` borrow before touching the market
        // source: one lock at a time, and no borrow held across another subsystem's lock.
        // The two scope filters read fields that are right there on the borrowed figure, so they
        // run BEFORE the market name and the figure are cloned — with "only armed" or "only
        // Moonbot" selected, most of the store is otherwise copied only to be dropped.
        let view = self.view;
        let figures: Vec<(CoreId, String, moon_core::figures::Figure)> = b
            .figures
            .borrow()
            .all()
            .filter(|(core, _, f)| {
                names.contains_key(core)
                    && view.scope_accepts(f.tool().def().alertable, f.alert)
            })
            .map(|(core, market, f)| (core, market.to_string(), f.clone()))
            .collect();
        // Resolve the coins core by core: one market-source lock and one catalogue snapshot each,
        // over that core's DISTINCT markets. Both halves matter — a per-figure resolve would take
        // the lock once per row, and several figures on one market are the normal case, so the
        // distinct set is what is asked about.
        //
        // The answers land in a vector parallel to `figures` rather than in a map keyed by
        // `(core, market)`: the rows are built in the same order, so a position is free where a
        // key would allocate a market name per row to look itself up with.
        let ms = b.session.market_source();
        let mut coin_of: Vec<String> = vec![String::new(); figures.len()];
        let mut by_core: HashMap<CoreId, Vec<usize>> = HashMap::new();
        for (ix, (core, ..)) in figures.iter().enumerate() {
            by_core.entry(*core).or_default().push(ix);
        }
        for (core, positions) in by_core {
            let mut distinct: Vec<&str> = positions
                .iter()
                .map(|ix| figures[*ix].1.as_str())
                .collect();
            distinct.sort_unstable();
            distinct.dedup();
            let coins: HashMap<&str, String> = distinct
                .iter()
                .copied()
                .zip(ms.market_labels(core, &distinct))
                .map(|(market, label)| (market, label.display_coin().to_string()))
                .collect();
            for ix in positions {
                if let Some(coin) = coins.get(figures[ix].1.as_str()) {
                    coin_of[ix] = coin.clone();
                }
            }
        }
        let mut rows: Vec<FigRow> = figures
            .into_iter()
            .zip(coin_of)
            .map(|((core, market, f), coin)| {
                let strategy = self.strategy_name(core, f.strategy_id);
                FigRow::new(
                    core,
                    names.get(&core).cloned().unwrap_or_default(),
                    market,
                    coin,
                    &f,
                    strategy,
                    online.get(&core).copied().unwrap_or(false),
                )
            })
            // Source and arm already applied above, on the borrowed figures.
            .filter(|row| row.matches(&self.query))
            .collect();
        sort_rows(&mut rows, self.view.sort, self.view.sort_asc);
        // A settings panel left open on a figure that is no longer listed has nothing to edit; it
        // would render empty and keep the row highlight on nothing.
        if self
            .settings_for
            .as_ref()
            .is_some_and(|t| !rows.iter().any(|r| r.is(t)))
        {
            self.settings_for = None;
        }
        self.rows = Rc::new(rows);
    }

    /// Rebuilds the rows from the current backend state, without repainting.
    ///
    /// Split from [`Self::refresh`] because a constructor has no repaint to ask for yet.
    fn reload(&mut self, cx: &App) {
        let backend = self.backend.clone();
        let b = backend.read(cx);
        self.rebuild(b);
        // Record what was just built. A click that arms, deletes or reassigns also moves the figure
        // store, so without this the observer would see a changed signature on the next drain and
        // rebuild the very rows this call produced. Written to the field rather than fed to the
        // gate: the gate may reject the sample on its floor and would then keep the OLD signature,
        // which is that same duplicate rebuild wearing a different hat.
        self.rows_sig = Some(self.data_sig(b));
    }

    /// Rebuilds now and repaints, for a change made by the panel's own controls.
    ///
    /// The observer's gate cannot see a filter change — the data behind the rows did not move — so
    /// every control writes through here rather than waiting for the next backend drain.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        cx.notify();
    }

    /// Returns a core's strategy name by ID, or the placeholder when it has none — or when the ID
    /// names a strategy the core no longer lists.
    ///
    /// Read out of the very list the row's dropdown offers, so the cell and the menu cannot disagree
    /// about what a strategy is called.
    fn strategy_name(&self, core: CoreId, strategy_id: u64) -> String {
        if strategy_id == 0 {
            return NO_STRATEGY.to_string();
        }
        self.alert_strategies
            .get(&core)
            .and_then(|v| v.iter().find(|(id, ..)| *id == strategy_id))
            .map(|(_, _, name)| name.to_string())
            .unwrap_or_else(|| NO_STRATEGY.to_string())
    }

    /// Applies a change to the view state, repainting and persisting only when it actually moved.
    ///
    /// Only a change to the FILTERS can change which figures are rows, so only that one rebuilds.
    /// A sort re-orders the rows already in hand, and a column toggle changes nothing about them at
    /// all — either would otherwise walk the whole figure store and take the market-source lock per
    /// core, and the column menu stays open for several clicks in a row.
    fn mutate(view: &Entity<Self>, app: &mut App, f: impl FnOnce(&mut AlertsViewState)) {
        let changed = view.update(app, |this, cx| {
            let mut next = this.view;
            f(&mut next);
            if next == this.view {
                return false;
            }
            let cols_changed = next.columns != this.view.columns;
            let sort_changed = (next.sort, next.sort_asc) != (this.view.sort, this.view.sort_asc);
            let rows_changed = (next.kind, next.arm) != (this.view.kind, this.view.arm);
            this.view = next;
            if cols_changed {
                // Drop any drag-defined order with the change. The table is given only the VISIBLE
                // columns, so a column switched back on is appended at the tail of the saved order
                // — which would park it to the RIGHT of the action buttons. Falling back to the
                // canonical order costs a manual arrangement that the toggle was already redrawing.
                this.col_order_cache.clear();
                this.table_state.update(cx, |s, _| s.column_order.clear());
                this.save_ctx_columns(cx);
            }
            if sort_changed {
                this.sync_table_sort(cx);
            }
            if rows_changed {
                this.refresh(cx);
            } else {
                if sort_changed {
                    let mut rows = this.rows.as_ref().clone();
                    sort_rows(&mut rows, this.view.sort, this.view.sort_asc);
                    this.rows = Rc::new(rows);
                }
                cx.notify();
            }
            true
        });
        if changed {
            Self::persist(view, app);
        }
    }

    /// Mirrors the view's sort into the table state, which draws the header's arrow.
    fn sync_table_sort(&self, cx: &mut Context<Self>) {
        let (key, asc) = (self.view.sort.key().to_string(), self.view.sort_asc);
        self.table_state.update(cx, |s, _| s.set_sort(key, asc));
    }

    /// Applies a saved per-context visible-column set, when it holds recognized keys.
    fn apply_ctx_columns(&mut self, cx: &App) {
        if let Some(keys) =
            crate::persistence::table_persist::visible(self.backend.read(cx), &self.widths_id)
        {
            let mask = keys
                .iter()
                .filter_map(|k| AlCol::from_key(k))
                .fold(0u32, |m, c| m | c.bit());
            // A set of nothing but the action column is as unusable as an empty one — it is a
            // table of two buttons — so both are refused in favour of the current view. The action
            // bit is then added back, because those buttons are the only way to delete a figure or
            // reach its settings.
            if mask & !AlCol::Actions.bit() != 0 {
                self.view.columns = mask | AlCol::Actions.bit();
            }
        }
    }

    /// Saves the current visible-column set in per-context storage.
    fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .view
            .visible_columns()
            .iter()
            .map(|c| c.key().to_string())
            .collect();
        crate::persistence::table_persist::set_visible(&self.backend, &self.widths_id, keys, cx);
    }

    /// Dumps the dock layout so the view state survives a reopen. View changes emit no `DockEvent`,
    /// so the panel persists them itself; the dump reads this panel, so it runs after the borrow.
    fn persist(view: &Entity<Self>, app: &mut App) {
        let (dock, group, backend) = {
            let p = view.read(app);
            (p.dock.clone(), p.group.clone(), p.backend.clone())
        };
        let Some(dock) = dock.and_then(|d| d.upgrade()) else {
            return;
        };
        let state = dock.read(app).dump(app);
        backend.update(app, |b, _| {
            b.dock_states.insert(group, state);
            b.dock_dirty = true;
        });
    }

    /// Returns the group's cores in canonical order for the core selector.
    fn group_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }

    /// Toggles the selected-core filter. `None` is the All row: a complete selection collapses back
    /// to the empty-means-all form, anything else selects every group core.
    fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .backend
            .read(cx)
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| s.id)
            .collect();
        match id {
            None => crate::controls::toggle_all_core_selection(&mut self.sel_cores, all),
            Some(id) => {
                if !self.sel_cores.remove(&id) {
                    self.sel_cores.insert(id);
                }
            }
        }
        self.refresh(cx);
    }

    /// Toggles every still-available core of one clicked exchange section.
    fn toggle_exchange_cores(&mut self, exchange_cores: Vec<CoreId>, cx: &mut Context<Self>) {
        let available = self
            .group_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.refresh(cx);
        }
    }

    /// Returns the retained table state for the detached window's width-reset button.
    pub(crate) fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Switches a newly created detached panel to the `:win` column-storage context, so a docked
    /// tab and a window keep separate widths and column sets.
    pub(crate) fn mark_table_detached(&mut self, cx: &mut Context<Self>) {
        self.widths_id = crate::persistence::table_persist::ctx_id(TABLE_ID, true);
        let saved = floored_widths(crate::persistence::table_persist::saved(
            self.backend.read(cx),
            &self.widths_id,
        ));
        self.table_state.update(cx, |s, c| {
            s.column_widths = saved;
            // The `:win` context brings its own visible set, so a drag order carried over from the
            // docked one would place a re-shown column past the action buttons — the hazard
            // `mutate` clears for. Inert while a detached panel starts with an empty order; free.
            s.column_order.clear();
            c.notify();
        });
        self.col_order_cache.clear();
        self.apply_ctx_columns(cx);
        self.refresh(cx);
    }
}

/// Restores persisted view fields from `PanelInfo`, keeping defaults for absent ones.
///
/// The core selection is deliberately not persisted, matching every other table here: a panel
/// reopens showing all of its group's cores.
fn view_from_info(info: &PanelInfo) -> AlertsViewState {
    let mut v = AlertsViewState::default();
    let PanelInfo::Panel(j) = info else {
        return v;
    };
    if let Some(k) = j.get("kind").and_then(|x| x.as_u64()) {
        v.kind = KindFilter::from_u8(k as u8);
    }
    if let Some(a) = j.get("arm").and_then(|x| x.as_u64()) {
        v.arm = ArmFilter::from_u8(a as u8);
    }
    if let Some(k) = j.get("sort").and_then(|x| x.as_str()) {
        // An unsortable or unknown key leaves the default rather than pinning the table to a column
        // whose header offers no way back.
        if let Some(col) = AlCol::from_key(k).filter(|c| c.sortable()) {
            v.sort = col;
        }
    }
    if let Some(asc) = j.get("sort_asc").and_then(|x| x.as_bool()) {
        v.sort_asc = asc;
    }
    if let Some(arr) = j.get("columns").and_then(|x| x.as_array()) {
        let mask = arr
            .iter()
            .filter_map(|x| x.as_str())
            .filter_map(AlCol::from_key)
            .fold(0u32, |m, c| m | c.bit());
        // As in `apply_ctx_columns`: an actions-only set is a table of two buttons, so it is
        // refused rather than restored.
        if mask & !AlCol::Actions.bit() != 0 {
            v.columns = mask | AlCol::Actions.bit();
        }
    }
    v
}

/// Raises any restored column width that sits below its column's floor.
///
/// A table writes its RESOLVED widths back into the state it was given, and the panel persists that
/// state — so the very first automatic layout becomes the saved one, and a column auto-fitted narrow
/// stays narrow on every launch afterwards, whatever the code declares. Harmless for text, which
/// merely truncates. Not harmless for the action column, whose two glyph buttons sit in a cell
/// clipped to exactly the column's width: the shipped layout froze it at 51 px and the delete
/// button was invisible on every launch until this ran.
///
/// Only the floored columns are touched; a width the user chose for anything else is theirs.
fn floored_widths(mut widths: HashMap<String, f32>) -> HashMap<String, f32> {
    for col in AlCol::ALL.into_iter().filter(|c| c.width_is_a_floor()) {
        if let Some(w) = widths.get_mut(col.key()) {
            *w = w.max(col.width());
        }
    }
    widths
}

/// Whether a core is in a state that can actually carry a command.
///
/// `Ready` is the same test the Analytics purge and the tuner's core menu apply before offering an
/// action. It matters here because a chart-alert command is enqueued into the core's channel and
/// then attempted ONCE by the worker: on a core that is merely reconnecting the attempt fails, is
/// logged, and is never retried — so a box ticked now would leave the figure marked armed here and
/// unknown to Moonbot forever.
fn core_is_ready(status: &moon_core::feed::ConnStatus) -> bool {
    *status == moon_core::feed::ConnStatus::Ready
}

/// Reads the drag-defined column order from `PanelInfo` as recognized [`AlCol`] keys. Unknown keys
/// are ignored, which tolerates a layout written by another build.
fn column_order_from_info(info: &PanelInfo) -> Vec<SharedString> {
    let PanelInfo::Panel(j) = info else {
        return Vec::new();
    };
    j.get("column_order")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .filter(|s| AlCol::from_key(s).is_some())
                .map(SharedString::from)
                .collect()
        })
        .unwrap_or_default()
}

impl EventEmitter<PanelEvent> for AlertsPanel {}
impl Focusable for AlertsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for AlertsPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn panel_name(&self) -> &'static str {
        "Alerts"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, cx: &App) -> PanelState {
        PanelState {
            panel_name: "Alerts".to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(serde_json::json!({
                "group": self.group,
                "kind": self.view.kind.to_u8(),
                "arm": self.view.arm.to_u8(),
                // Stable keys rather than an enum index, so the column list may be reordered.
                "sort": self.view.sort.key(),
                "sort_asc": self.view.sort_asc,
                "columns": self
                    .view
                    .visible_columns()
                    .iter()
                    .map(|c| c.key())
                    .collect::<Vec<_>>(),
                "column_order": self
                    .table_state
                    .read(cx)
                    .column_order
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>(),
            })),
        }
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
                "alerts-reset-widths",
                &self.table_state,
            ),
            crate::panels::detach_button(
                "Alerts",
                self.group.clone(),
                self.backend.clone(),
                self.dock.clone(),
            ),
        ])
    }
}

impl Render for AlertsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        // A COLUMN, and nothing else, around the table. Two layout traps live in these four
        // lines and this panel fell into both:
        // - a bare `div()` is `Display::Block` (gpui `Style::default`), where the table host's
        //   `flex_1` means nothing at all;
        // - `h_flex()` is `flex_row().items_center()`, and a centred child is laid out at its
        //   CONTENT height — which for a virtual list is zero.
        // Either one leaves a panel with a filled row count and no table, header included. In a
        // column, `flex_1` IS the height, which is why the settings became a popover anchored to
        // the row's gear rather than anything that shares this box.
        let body = v_flex()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .child(self.table(p, cx));
        v_flex()
            .id("alerts-panel")
            .size_full()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .track_focus(&self.focus)
            .child(self.toolbar(p, cx))
            .child(body)
            .child(self.bottom_bar(p, cx))
    }
}
