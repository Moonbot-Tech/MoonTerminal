//! [`StrategiesView`] construction, change detection, and event/focus integration.

use super::*;

use crate::workspace::scope_marker::ScopeMarker;

/// Resolve the Auto rail's seeded core, the overlay `StrategiesView::rail_expanded_core` holds.
///
/// The window is opened from a rail that already says which server the user is working on, so
/// opening it fully collapsed makes them re-find that server by hand every time. The result
/// REPLACES any previous seed rather than accumulating it: the rail names at most one core, and a
/// stale seed for a core the rail left must not linger as if the user had expanded it by hand.
///
/// The intersection with the visible scope keeps two validity levels distinct: `selected_core` is
/// validated against live cores, while the tree is built from the effective workspace scope. The
/// guard prevents any narrower scope from seeding a node that never renders. A `None` workspace
/// slice (not scope-bound) still seeds.
///
/// A core whose strategies are all filtered out does not enter the tree at all; the seeded
/// expansion is harmless there and takes effect as soon as the filter admits it.
///
/// Args:
///     selected_core: Concrete Auto rail selection, or `None` for Classic and Auto Overview.
///     workspace_cores: Cores the window may show, or `None` when it is not scope-bound.
///
/// Returns:
///     The core to open with, or `None` whenever there is nothing unambiguous to expand.
fn rail_seed_core(
    selected_core: Option<CoreId>,
    workspace_cores: Option<&[CoreId]>,
) -> Option<CoreId> {
    let core = selected_core?;
    if workspace_cores.is_some_and(|cores| !cores.contains(&core)) {
        return None;
    }
    Some(core)
}

/// Whether the retained expansion state considers `core` open: hand-expanded, or the current
/// Auto rail seed.
///
/// Args:
///     expanded: Cores the user expanded by hand.
///     rail: The current rail overlay, if any.
///     core: Core being tested.
///
/// Returns:
///     `true` when either source counts `core` as open.
pub(super) fn core_is_open(expanded: &HashSet<CoreId>, rail: Option<CoreId>, core: CoreId) -> bool {
    expanded.contains(&core) || rail == Some(core)
}

/// Toggle one core's expansion across both the persisted set and the rail overlay.
///
/// Collapsing clears the rail seed too when it names this core: otherwise a click meant to close
/// the row would leave it reopened by the overlay on the very next frame. Expanding writes only
/// the persisted set, matching every other hand-expansion site — the overlay is exclusively an
/// Auto rail concern.
///
/// Deliberately leaves `StrategiesView::rail_seen_core` untouched: that field tracks what the rail
/// last resolved to, not what the user is currently showing, so a later unrelated revision that
/// resolves the same rail selection recognises it as unchanged instead of reopening this row.
///
/// Args:
///     expanded: Cores the user expanded by hand, mutated in place.
///     rail: The current rail overlay, mutated in place.
///     core: Core being toggled.
pub(super) fn toggle_core_expansion(
    expanded: &mut HashSet<CoreId>,
    rail: &mut Option<CoreId>,
    core: CoreId,
) {
    if core_is_open(expanded, *rail, core) {
        expanded.remove(&core);
        if *rail == Some(core) {
            *rail = None;
        }
    } else {
        expanded.insert(core);
    }
}

/// Resolve the cores a Classic-focused singleton window may DISPLAY.
///
/// [`singleton_strategy_scope`] answers only for an Auto owner, because
/// `Backend::singleton_workspace` is Auto-only by construction
/// (`workspace::resolve_singleton_workspace`). That is right for Auto's SELECTED-CORE question
/// and wrong for the display one: under Classic the window never asked whether a core is a
/// member of the preset it is being viewed under. This asks.
///
/// The universe is `config.servers` UNIONED with live sessions, not sessions alone: this list
/// also gates retained per-core drafts (`logic::staged_count`, `logic::field_edit_count`), and a
/// sessions-only list would silently drop a DISCONNECTED core's pending-change count — which is
/// not a membership question. `Backend::core_displayed` admits any core absent from
/// `config.servers`, so the union is exactly the complement of the hidden configured cores.
///
/// Args:
///     b: Backend supplying the display preset and the configured/live core universe.
///
/// Returns:
///     The shown cores, or `None` when unscoped (no Classic focus, or nothing is hidden) — the
///     latter keeps every unaffected Classic state today byte-identical.
fn singleton_display_cores(b: &Backend) -> Option<Vec<CoreId>> {
    let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton)?;
    if preset != moon_core::config::WorkspaceMode::Classic {
        return None;
    }
    let preset = Some(preset);
    let mut seen = HashSet::new();
    let mut shown = Vec::new();
    let mut excluded = false;
    for id in b
        .config
        .servers
        .iter()
        .map(|server| server.id)
        .chain(b.session.sessions().iter().map(|session| session.id))
    {
        if !seen.insert(id) {
            continue;
        }
        if b.core_displayed(preset, id) {
            shown.push(id);
        } else {
            excluded = true;
        }
    }
    excluded.then_some(shown)
}

/// Resolve the singleton Auto owner's core roots and concrete rail selection together, or the
/// Classic viewing preset's display membership.
///
/// One resolve for both Auto answers: each `singleton_workspace()` call re-ranks the group's
/// cores and re-checks every core's availability, so asking twice doubles the cost of opening
/// this window and of every rail move it observes. Outside an Auto owner, the viewing preset may
/// still hide cores under Classic; `selected_core` is always `None` there, since
/// Classic has no rail selection.
///
/// Args:
///     b: Backend supplying the singleton workspace, its effective scope, and the Classic display
///         preset.
///
/// Returns:
///     The visible core roots and the selected core, or `None` when nothing is scope-bound.
fn singleton_strategy_scope(b: &Backend) -> Option<(Vec<CoreId>, Option<CoreId>)> {
    if let Some(workspace) = b.singleton_workspace() {
        let cores = b
            .effective_workspace_scope(&workspace.group, crate::workspace::RetainedCoreScope::All)
            .ids()
            .to_vec();
        return Some((cores, workspace.selected_core));
    }
    singleton_display_cores(b).map(|cores| (cores, None))
}

/// Resolve the scope marker for the tree's own empty state, over the SAME universe the tree lists.
///
/// Deliberately a second resolve rather than folded into [`singleton_strategy_scope`]: that
/// function's own doc explains why re-resolving `singleton_workspace()` here is not free, and
/// merging the two into one typed context is deferred on purpose — this branch may add a marker
/// beside the scoping, never edit the scoping itself.
///
/// The Classic universe here is LIVE SESSIONS, deliberately NOT the `config.servers ∪ sessions`
/// union [`singleton_display_cores`] iterates. That function's wider universe exists to gate a
/// DISCONNECTED core's pending-change count, which is not a display question; the tree lists
/// connected cores only, because [`visible_strategy_cores`] iterates `backend.session.sessions()`.
/// Counting the union while classifying against sessions would label a merely disconnected
/// configured core "hidden by the preset" when nothing hid it.
///
/// Args:
///     b: Backend supplying the singleton workspace, its effective scope, and the Classic display
///         preset.
///
/// Returns:
///     A marker over the tree's own universe, or `None` when nothing is scope-bound.
fn singleton_strategy_marker(b: &Backend) -> Option<ScopeMarker> {
    if let Some(workspace) = b.singleton_workspace() {
        let scope =
            b.effective_workspace_scope(&workspace.group, crate::workspace::RetainedCoreScope::All);
        return Some(ScopeMarker::new(
            Some(moon_core::config::WorkspaceMode::AutoTrading),
            scope.membership_shown(),
            scope.membership_total(),
        ));
    }
    let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton);
    ScopeMarker::from_membership(
        preset,
        b.session
            .sessions()
            .iter()
            .map(|s| b.core_displayed(preset, s.id)),
    )
}

/// Fold strategy and schema revisions only for cores visible in the current singleton scope.
fn strategies_sig(b: &Backend, workspace_cores: Option<&[CoreId]>) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter(|session| strategy_core_is_visible(workspace_cores, session.id))
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| {
            a.wrapping_mul(31)
                .wrapping_add(c.strategies_rev)
                .wrapping_mul(31)
                .wrapping_add(c.schema_rev)
                .wrapping_mul(31)
                .wrapping_add(c.strategy_edit_rev)
                .wrapping_mul(31)
                .wrapping_add(c.strategy_edit_note_rev)
        })
}

impl StrategiesView {
    /// Create the Strategies view and subscribe it to search, tree, backend, and window events.
    ///
    /// A process-lifetime snapshot restores browsing state after the window is closed and
    /// reopened. Construction then seeds the Auto rail's selected core into the `rail_expanded_core`
    /// overlay when it belongs to the visible workspace scope, so a collapsed snapshot still opens
    /// that server's list — the overlay, never the restored snapshot itself, which is why a rail
    /// seed never survives into another window or scope.
    ///
    /// Args:
    ///     backend: Shared state supplying strategy data and workspace scope.
    ///     window: Owning window used to construct inputs and observe geometry.
    ///     cx: View context used to create retained entities and subscriptions.
    ///
    /// Returns:
    ///     A fully initialized Strategies view.
    pub(super) fn new(
        backend: Entity<Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let panels = backend.read(cx).layout.strategies_panels;
        let prefs = StrategiesPrefs::restore(&backend.read(cx).layout);
        let display_zone =
            crate::chrome::clock::resolved_header_clock_zone(backend.read(cx).header_clock_zone());
        let session = backend.read(cx).ui_session.strategies.clone();
        let search = cx.new(|cx| {
            let input = MoonInputState::new(window, cx).placeholder(t!("strat.search").to_string());
            match &session {
                Some(s) => input.default_value(s.search.clone()),
                None => input,
            }
        });
        // Update the filter and redraw from search input events; render must not poll the input as
        // an event source.
        cx.subscribe(&search, |this, input, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if this.filter.search != value {
                    this.filter.search = value;
                    this.persist_session(cx);
                    cx.notify();
                }
            }
        })
        .detach();

        let scope = singleton_strategy_scope(backend.read(cx));
        let workspace_cores = scope.as_ref().map(|(cores, _)| cores.clone());
        let scope_marker = singleton_strategy_marker(backend.read(cx));
        // Seeded from the same read the marker was built from, so the backend observer's first
        // tick does not mistake an unchanged roster for a changed one and pay the recount.
        let session_roster_sig = backend
            .read(cx)
            .session
            .sessions()
            .iter()
            .fold(0u64, |acc, s| acc.wrapping_mul(31).wrapping_add(s.id));
        let selected_core = scope.and_then(|(_, selected)| selected);
        let initial_sig = strategies_sig(backend.read(cx), workspace_cores.as_deref());
        let expanded_cores = match &session {
            Some(s) => s.expanded_cores.clone(),
            None => HashSet::new(),
        };
        let rail_seed = rail_seed_core(selected_core, workspace_cores.as_deref());
        let rail_expanded_core = rail_seed;
        let rail_seen_core = rail_seed;

        let tree_state = cx.new(|cx| MoonTreeState::new(cx));
        // MoonTree can mutate expansion from keyboard input, but `expanded_cores` and
        // `expanded_folders` remain authoritative. Invalidating the cached shape makes the next
        // frame restore that window-owned expansion without requiring unconditional tree pushes.
        //
        // The adapter cache goes with it. A keyboard expansion changes state inside MoonTree and
        // NOTHING the tree signature hashes, so a surviving cache entry would send the next frame
        // straight past the push that reasserts the window's own expansion — and the tree would
        // stay wherever the keyboard left it.
        //
        // `pane_cache` deliberately survives this: expansion is not an input to any of its three
        // entries — the kinds list, the Start/Stop plan and the label widths all read data the
        // keyboard did not touch.
        cx.subscribe(&tree_state, |this, _state, _ev: &MoonTreeEvent, cx| {
            this.last_tree_shape = None;
            this.tree_cache = None;
            cx.notify();
        })
        .detach();

        // Redraw for new strategy or schema snapshots. When explicitly enabled by
        // `MOON_STRATEGY_RULES_HOT_RELOAD`, rules reload on the file-mtime timer below; observing
        // backend data must not become a surrogate filesystem polling loop. A `strategies_goto`
        // request also wakes render, where it is drained with Window access.
        cx.observe(&backend, |this, backend, cx| {
            let b = backend.read(cx);
            let goto = b.strategies_goto.is_some();
            let sig = strategies_sig(b, this.workspace_cores.as_deref());
            let strategies_changed = sig != this.last_sig;
            // The marker counts LIVE SESSIONS, so its universe moves when a core connects or
            // disconnects — and neither of those raises a workspace revision, which is where the
            // other observer refreshes it. `strategies_sig` cannot stand in for that signal
            // either: it folds only over cores the current scope already shows, so connecting a
            // core the preset HIDES leaves it untouched, and the pane keeps saying "no cores
            // connected" about a core that just arrived.
            //
            // Only the session ROSTER is folded here, never the marker itself: recomputing the
            // marker walks the group and re-checks availability, and this observer fires on every
            // backend tick. The fold is O(sessions) over ids the caller already holds.
            let roster = b
                .session
                .sessions()
                .iter()
                .fold(0u64, |acc, s| acc.wrapping_mul(31).wrapping_add(s.id));
            //
            // The repaint is DEFERRED to the end of the callback rather than taken here: `b`
            // borrows the backend out of `cx`, and that borrow stays live as long as the arm
            // below still reads it, so a `cx.notify()` at this point cannot borrow `cx` mutably.
            let mut marker_moved = false;
            if roster != this.session_roster_sig {
                this.session_roster_sig = roster;
                let marker = singleton_strategy_marker(b);
                if marker != this.scope_marker {
                    this.scope_marker = marker;
                    marker_moved = true;
                }
            }
            if strategies_changed || goto {
                if strategies_changed {
                    this.reconcile_ui_folders(b.session.store());
                }
                this.last_sig = sig;
                this.sync_pending_select(cx);
                this.clamp_selected_section(cx);
                this.persist_session(cx);
                cx.notify();
            } else if marker_moved {
                // The arm above already repainted. This one covers the case it does not reach:
                // a core the preset HIDES connecting or leaving moves the marker's counts while
                // `strategies_sig` — which folds only over cores the scope already shows — does
                // not budge.
                cx.notify();
            }
        })
        .detach();

        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            // Refreshed before the shape-equality return below: another configured core the
            // current preset also hides leaves the SHOWN list unchanged while the marker's
            // `configured` count moves, and a refresh placed after that return would leave the
            // pane's hidden-by-preset facts stale.
            //
            // Refreshing is not enough on its own — the early return below skips `cx.notify()`,
            // and a pinned `Entity::update` does not notify implicitly, so a marker that moved
            // while the visible vector stayed equal would sit in the field unpainted until some
            // unrelated repaint happened to arrive. Switching Classic to Auto with the same single
            // core selected is exactly that shape.
            let marker = singleton_strategy_marker(this.backend.read(cx));
            let marker_moved = marker != this.scope_marker;
            this.scope_marker = marker;
            let scope = singleton_strategy_scope(this.backend.read(cx));
            let next = scope.as_ref().map(|(cores, _)| cores.clone());
            // Recomputed BEFORE the shape-equality return below: a single-core group's Overview and
            // AutoCore id vectors are equal, so a live rail move between them would otherwise never
            // reach this observer's body at all.
            let selected_core = scope.as_ref().and_then(|(_, selected)| *selected);
            let rail = rail_seed_core(selected_core, next.as_deref());
            // Compared against `rail_seen_core`, never the overlay: the overlay is the user's to
            // clear (`toggle_core_expansion`), and comparing against it would resurrect a
            // hand-collapsed core on the next unrelated revision. `rail_seen_core` tracks what the
            // rail last resolved to regardless of what the user did with the overlay afterwards, so
            // only a rail selection that actually MOVED re-seeds.
            let rail_moved = rail != this.rail_seen_core;
            if rail_moved {
                // REPLACE, never extend: the overlay is what the rail says NOW. `None` under Auto
                // Overview is the live-window twin of "a seed is never carried into another scope".
                this.rail_seen_core = rail;
                this.rail_expanded_core = rail;
                this.tree_cache = None;
                this.last_tree_shape = None;
            }
            if next == this.workspace_cores {
                if marker_moved || rail_moved {
                    cx.notify();
                }
                return;
            }
            this.workspace_cores = next;
            this.last_sig = strategies_sig(this.backend.read(cx), this.workspace_cores.as_deref());
            this.tree_cache = None;
            this.last_tree_shape = None;
            // `pane_cache` needs no explicit reset here either: a scope move changes the visible
            // core list, which the store half of the signature hashes, and the workspace generation
            // it carries has already advanced.
            // In-flight version results compare against `versions.key`; retiring it prevents an
            // old hidden core from publishing after the owner changes.
            this.versions.key = None;
            // Otherwise a confirmation for the strategy this scope just left keeps showing over
            // whatever the new scope selects next (plan amendment A3).
            this.versions.clear_selection();
            this.persist_session(cx);
            cx.notify();
        })
        .detach();

        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _revision, cx| {
            let zone = crate::chrome::clock::resolved_header_clock_zone(
                this.backend.read(cx).header_clock_zone(),
            );
            if zone != this.display_zone {
                this.display_zone = zone;
                cx.notify();
            }
        })
        .detach();

        // Persist Strategies-window geometry in layout. The debounced save loop drains
        // `layout_dirty`, matching group windows.
        cx.observe_window_bounds(window, |this, window, cx| {
            let geom = crate::window::windowing::window_geom_rect(window, cx);
            this.backend.update(cx, |b, _| {
                let geom = geom.keeping_display_of(b.layout.strategies_window);
                if b.layout.strategies_window != Some(geom) {
                    b.layout.strategies_window = Some(geom);
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        if std::env::var_os("MOON_STRATEGY_RULES_HOT_RELOAD").is_some() {
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                loop {
                    executor.timer(Duration::from_secs(1)).await;
                    let alive = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            if this.rules.reload_if_changed() {
                                cx.notify();
                            }
                        })
                        .is_ok()
                    });
                    if !alive {
                        break;
                    }
                }
            })
            .detach();
        }

        cx.on_release(|this, app| {
            this.persist_session(app);
        })
        .detach();

        cx.spawn(async move |view, cx| {
            cx.background_spawn(async { crate::media::exchange_logos::prewarm() })
                .await;
            cx.update(|cx| {
                let _ = view.update(cx, |this, cx| {
                    this.exchange_logos_ready = true;
                    cx.notify();
                });
            });
        })
        .detach();

        let mut this = Self {
            backend,
            display_zone,
            search,
            filter: match &session {
                Some(s) => StrategyFilter {
                    search: s.search.clone(),
                    kind: s.kind,
                    dir: s.dir,
                    exchange: s.exchange,
                    active_only: prefs.active_only,
                },
                None => StrategyFilter {
                    active_only: prefs.active_only,
                    ..StrategyFilter::default()
                },
            },
            prefs,
            exchange_logos_ready: false,
            settings_open: false,
            workspace_cores,
            scope_marker,
            session_roster_sig,
            selected: session.as_ref().and_then(|s| s.selected),
            sel: session.as_ref().map(|s| s.sel.clone()).unwrap_or_default(),
            versions: versions::VersionsState {
                collapsed: panels.versions_collapsed,
                ..Default::default()
            },
            panels,
            deleted: HashMap::new(),
            deleted_gen: u64::MAX,
            deleted_rev: 0,
            deleted_inflight: false,
            expanded_deleted: session
                .as_ref()
                .map(|s| s.expanded_deleted.clone())
                .unwrap_or_default(),
            anchor: session.as_ref().and_then(|s| s.anchor),
            flat_order: Vec::new(),
            tree_state,
            selected_section: session.as_ref().map(|s| s.selected_section).unwrap_or(0),
            staged: HashMap::new(),
            field_edits: HashMap::new(),
            last_edit_note_seq: HashMap::new(),
            field_inputs: HashMap::new(),
            field_memos: HashMap::new(),
            field_colors: HashMap::new(),
            focused_field: None,
            expanded_cores,
            rail_expanded_core,
            rail_seen_core,
            expanded_folders: session
                .as_ref()
                .map(|s| s.expanded_folders.clone())
                .unwrap_or_default(),
            rules: Rules::load(),
            clipboard: None,
            pending_names: HashSet::new(),
            selected_folder: session.as_ref().and_then(|s| s.selected_folder.clone()),
            ui_folders: session
                .as_ref()
                .map(|s| s.ui_folders.clone())
                .unwrap_or_default(),
            op: None,
            op_input: None,
            op_input_init: String::new(),
            pending_select: None,
            last_sig: initial_sig,
            last_tree_shape: None,
            tree_cache: None,
            pane_cache: PaneCache::default(),
            pending_scroll: None,
            tree_field_bounds: std::rc::Rc::new(std::cell::Cell::new(None)),
            params_scroll: MoonVirtualListScrollHandle::new(),
            pending_param_scroll: None,
            focus: cx.focus_handle(),
        };
        // Observe does not run the backend callback at subscribe time, so a snapshot restored
        // after the window was closed must be reconciled against the live store here.
        this.reconcile_ui_folders(this.backend.read(cx).session.store());
        this.clamp_selected_section(cx);
        this
    }

    /// Copy browsing state onto the process-lifetime Backend snapshot.
    ///
    /// Called after user mutations of snapshotted fields, and from `on_release` as a backstop.
    ///
    /// Args:
    ///     cx: App context used to write `Backend.ui_session.strategies`.
    pub(super) fn persist_session(&self, cx: &mut App) {
        let snapshot = StrategiesSessionState::capture(self);
        self.backend.update(cx, |b, _| {
            b.ui_session.strategies = Some(snapshot);
        });
    }

    /// Reseed a live Strategies view from the Auto-selected core while preserving hand expansions.
    ///
    /// Used when focusing an already-open window. Construction seeds the same overlay before the
    /// first paint; this path must also drop the tree cache so the next frame rebuilds the
    /// selected core's subtree. Nothing is persisted here: the rail overlay never reaches
    /// `StrategiesSessionState`, so there is nothing for `persist_session` to save.
    ///
    /// Compared against the OVERLAY, not `rail_seen_core`: focusing an already-open window
    /// deliberately REOPENS the seeded core even if the user had collapsed it by hand, which is
    /// why this path disagrees with the `workspace_revision` observer's comparison.
    ///
    /// Args:
    ///     cx: View context used to read the current scope and notify.
    pub(super) fn ensure_auto_selected_core_expanded(&mut self, cx: &mut Context<Self>) {
        let selected_core =
            singleton_strategy_scope(self.backend.read(cx)).and_then(|(_, selected)| selected);
        let rail = rail_seed_core(selected_core, self.workspace_cores.as_deref());
        if rail == self.rail_expanded_core && rail == self.rail_seen_core {
            return;
        }
        self.rail_seen_core = rail;
        self.rail_expanded_core = rail;
        self.tree_cache = None;
        self.last_tree_shape = None;
        cx.notify();
    }

    /// Toggle one core's expansion from a tree click, over both expansion fields.
    ///
    /// Args:
    ///     core: Core whose row was clicked.
    pub(super) fn toggle_core_expanded(&mut self, core: CoreId) {
        toggle_core_expansion(&mut self.expanded_cores, &mut self.rail_expanded_core, core);
    }

    // ── Selection ───────────────────────────────────────────────────────────
}

impl EventEmitter<()> for StrategiesView {}
impl Focusable for StrategiesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests;
