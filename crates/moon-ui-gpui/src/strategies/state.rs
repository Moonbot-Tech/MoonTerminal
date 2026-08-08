//! [`StrategiesView`] construction, change detection, and event/focus integration.

use super::*;

fn strategies_sig(b: &Backend) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| {
            a.wrapping_mul(31)
                .wrapping_add(c.strategies_rev)
                .wrapping_mul(31)
                .wrapping_add(c.schema_rev)
        })
}

impl StrategiesView {
    /// Creates the Strategies view and subscribes it to search, tree, backend, and window events.
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

        let initial_sig = strategies_sig(backend.read(cx));

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
            let sig = strategies_sig(b);
            if sig != this.last_sig || goto {
                this.last_sig = sig;
                this.sync_pending_select(cx);
                this.clamp_selected_section(cx);
                cx.notify();
            }
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
            expanded_cores: HashSet::new(),
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
            // Hide dependency-inactive parameters by default.
            only_active_params: true,
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
