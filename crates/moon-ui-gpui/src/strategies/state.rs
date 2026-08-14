//! [`StrategiesView`] construction, change detection, and event/focus integration.

use super::*;

/// Seed the tree's expanded cores from the Auto rail selection.
///
/// The window is opened from a rail that already says which server the user is working on, so
/// opening it fully collapsed makes them re-find that server by hand every time.
///
/// The intersection with the visible scope keeps two validity levels distinct: `selected_core` is
/// validated against live cores, while the tree is built from the effective workspace scope. The
/// guard prevents any narrower scope from seeding a node that never renders.
///
/// A core whose strategies are all filtered out does not enter the tree at all; the seeded
/// expansion is harmless there and takes effect as soon as the filter admits it.
///
/// Args:
///     selected_core: Concrete Auto rail selection, or `None` for Classic and Auto Overview.
///     workspace_cores: Cores the window may show, or `None` when it is not scope-bound.
///
/// Returns:
///     The set of cores to open with; empty whenever there is nothing unambiguous to expand.
fn initial_expanded_cores(
    selected_core: Option<CoreId>,
    workspace_cores: Option<&[CoreId]>,
) -> HashSet<CoreId> {
    let Some(core) = selected_core else {
        return HashSet::new();
    };
    if workspace_cores.is_some_and(|cores| !cores.contains(&core)) {
        return HashSet::new();
    }
    HashSet::from([core])
}

/// Resolve the singleton Auto owner's core roots and concrete rail selection together.
///
/// One resolve for both answers: each `singleton_workspace()` call re-ranks the group's cores and
/// re-checks every core's availability, so asking twice doubles the cost of opening this window
/// and of every rail move it observes.
///
/// Args:
///     b: Backend supplying the singleton workspace and its effective scope.
///
/// Returns:
///     The visible core roots and the selected core, or `None` outside a singleton Auto owner.
fn singleton_strategy_scope(b: &Backend) -> Option<(Vec<CoreId>, Option<CoreId>)> {
    let workspace = b.singleton_workspace()?;
    let cores = b
        .effective_workspace_scope(&workspace.group, crate::workspace::RetainedCoreScope::All)
        .ids()
        .to_vec();
    Some((cores, workspace.selected_core))
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
        })
}

impl StrategiesView {
    /// Create the Strategies view and subscribe it to search, tree, backend, and window events.
    ///
    /// The initial Auto rail selection seeds the expanded core set when it belongs to the visible
    /// workspace scope.
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
        let display_zone =
            crate::chrome::clock::resolved_header_clock_zone(backend.read(cx).header_clock_zone());
        let search = cx
            .new(|cx| MoonInputState::new(window, cx).placeholder(t!("strat.search").to_string()));
        // Update the filter and redraw from search input events; render must not poll the input as
        // an event source.
        cx.subscribe(&search, |this, input, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if this.filter.search != value {
                    this.filter.search = value;
                    cx.notify();
                }
            }
        })
        .detach();

        let scope = singleton_strategy_scope(backend.read(cx));
        let workspace_cores = scope.as_ref().map(|(cores, _)| cores.clone());
        let selected_core = scope.and_then(|(_, selected)| selected);
        let initial_sig = strategies_sig(backend.read(cx), workspace_cores.as_deref());
        let expanded_cores = initial_expanded_cores(selected_core, workspace_cores.as_deref());

        let tree_state = cx.new(|cx| MoonTreeState::new(cx));
        // MoonTree can mutate expansion from keyboard input, but `expanded_cores` and
        // `expanded_folders` remain authoritative. Invalidating the cached shape makes the next
        // frame restore that window-owned expansion without requiring unconditional tree pushes.
        //
        // The adapter cache goes with it. A keyboard expansion changes state inside MoonTree and
        // NOTHING the tree signature hashes, so a surviving cache entry would send the next frame
        // straight past the push that reasserts the window's own expansion — and the tree would
        // stay wherever the keyboard left it.
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
            if strategies_changed || goto {
                if strategies_changed {
                    this.reconcile_ui_folders(b.session.store());
                }
                this.last_sig = sig;
                this.sync_pending_select(cx);
                this.clamp_selected_section(cx);
                cx.notify();
            }
        })
        .detach();

        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            let scope = singleton_strategy_scope(this.backend.read(cx));
            let next = scope.as_ref().map(|(cores, _)| cores.clone());
            if next == this.workspace_cores {
                return;
            }
            this.workspace_cores = next;
            // Re-seed additively on an actual rail move because the singleton window outlives the
            // selection. Existing expansions remain intact; returning to a manually collapsed core
            // re-opens it.
            let selected_core = scope.and_then(|(_, selected)| selected);
            this.expanded_cores.extend(initial_expanded_cores(
                selected_core,
                this.workspace_cores.as_deref(),
            ));
            this.last_sig = strategies_sig(this.backend.read(cx), this.workspace_cores.as_deref());
            this.tree_cache = None;
            this.last_tree_shape = None;
            // In-flight version results compare against `versions.key`; retiring it prevents an
            // old hidden core from publishing after the owner changes.
            this.versions.key = None;
            this.versions.sel = None;
            this.versions.row = None;
            this.versions.changed.clear();
            this.versions.section = None;
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
            let Some((x, y, w, h)) = crate::window::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |b, _| {
                if b.layout.strategies_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                    b.layout.strategies_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
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

        Self {
            backend,
            display_zone,
            search,
            filter: StrategyFilter::default(),
            workspace_cores,
            selected: None,
            sel: HashSet::new(),
            versions: versions::VersionsState {
                collapsed: panels.versions_collapsed,
                ..Default::default()
            },
            panels,
            deleted: HashMap::new(),
            deleted_gen: u64::MAX,
            deleted_rev: 0,
            deleted_inflight: false,
            expanded_deleted: HashSet::new(),
            anchor: None,
            flat_order: Vec::new(),
            tree_state,
            selected_section: 0,
            staged: HashMap::new(),
            field_edits: HashMap::new(),
            field_inputs: HashMap::new(),
            field_memos: HashMap::new(),
            field_colors: HashMap::new(),
            focused_field: None,
            expanded_cores,
            expanded_folders: HashSet::new(),
            rules: Rules::load(),
            clipboard: None,
            pending_names: HashSet::new(),
            selected_folder: None,
            ui_folders: HashSet::new(),
            op: None,
            op_input: None,
            op_input_init: String::new(),
            pending_select: None,
            last_sig: initial_sig,
            last_tree_shape: None,
            tree_cache: None,
            pending_scroll: None,
            focus: cx.focus_handle(),
        }
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
