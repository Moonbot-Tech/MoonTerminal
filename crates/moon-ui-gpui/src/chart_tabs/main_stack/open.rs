//! Main chart stack open operations.

use super::*;

impl MainChartStack {
    /// Construct a group's Main chart stack and optionally open its initial target.
    ///
    /// Args:
    ///     backend: Shared application state used by chart panels and market publication.
    ///     group: Main window group that owns this stack.
    ///     focus_open: Optional core and market to focus during construction.
    ///     epoch: Chart time origin passed to child panels.
    ///     theme: Runtime chart rendering theme.
    ///     cx: Stack context used to create and observe panels.
    ///
    /// Returns:
    ///     An initialized Main stack.
    pub(in crate::chart_tabs) fn new(
        backend: Entity<Backend>,
        group: String,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            backend,
            group,
            epoch,
            theme,
            charts: Vec::new(),
            active: None,
            show_stack: false,
            scale: None,
            layout_mode: None,
            layout_height_fit: None,
            layout_height_scroll: None,
            orderbook_enabled: None,
            candle_view: None,
            chart_graphics: None,
            chart_labels: None,
            pending_labels: None,
            x_ppm: None,
            show_zone: None,
            auto_pin: None,
            layout_orientation: None,
            price_axis_pos: None,
            time_axis_visible: None,
            line_labels: None,
            cursor_labels: None,
            pushed_kind: None,
            compare_anchor: None,
            compare_y: None,
            compare_orderbook_only: false,
            crowd: None,
            empty_settings_open: false,
            empty_detect: None,
            empty_places: None,
            idle_timer_armed: false,
            layout_columns: None,
            layout_columns_exact: None,
            layout_min_slot: None,
            measured: Rc::new(Cell::new(Size::default())),
            host_visible: true,
            scroll: MoonVirtualListScrollHandle::new(),
        };
        if let Some((core, market)) = focus_open {
            this.open_or_focus(core, market, crate::backend::ChartHistoryScope::Default, cx);
        }
        this
    }

    /// Create and configure one Main chart panel with the stack's current presentation settings.
    ///
    /// Args:
    ///     core: Live core that supplies the market.
    ///     market: Canonical market name to open.
    ///     cx: Stack context used to create the panel and retain its observer.
    ///
    /// Returns:
    ///     The configured chart-panel entity.
    pub(super) fn create_panel(
        &self,
        core: CoreId,
        market: &str,
        cx: &mut Context<Self>,
    ) -> Entity<ChartPanel> {
        let backend = self.backend.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let workspace_group = self.group.clone();
        let market = market.to_string();
        // The constructor starts every main panel on the Main default, which is wrong while the
        // stack is under the anchor lock — and `sync_default_kind`'s gate would then skip it,
        // because the kind it would push is the one it has already pushed. Told here instead, where
        // the panel is created. The Add stack has no such branch: its constructor takes the kind.
        let kind = self.default_kind();
        let panel = cx.new(|cx| {
            ChartPanel::new_main(
                backend,
                Some(workspace_group),
                Some((core, market)),
                epoch,
                theme,
                cx,
            )
        });
        panel.update(cx, |p, pcx| p.set_default_kind(kind, pcx));
        cx.observe(&panel, |this, panel, cx| {
            // Captions a panel's own menu edited: carried one link further, to the host that owns
            // the tab spec. Taken from the panel that notified, so a stack of eight costs one
            // `Option` read per notification.
            // Kept APART from `dirty`: an edit has to wake the host, but it changes no chart's
            // visibility and opens no market, and folding it into `dirty` would run both of those
            // sweeps for a caption setting.
            let edited = match panel.update(cx, |panel, _| panel.take_pending_labels()) {
                Some(cfg) => {
                    this.pending_labels = Some(cfg);
                    true
                }
                None => false,
            };
            let mut dirty = this.prune_empty(cx);
            if dirty {
                this.sync_visibility(cx);
                this.sync_backend_open_markets(cx);
            }
            dirty |= this.sync_compare(cx);
            if dirty || edited {
                cx.notify();
            }
        })
        .detach();
        if self.scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(self.scale, pcx));
        }
        if let Some(en) = self.orderbook_enabled {
            panel.update(cx, |panel, pcx| panel.set_orderbook_enabled(en, pcx));
        }
        if self.candle_view.is_some() {
            let cv = self.candle_view;
            panel.update(cx, |panel, pcx| panel.set_candle_view(cv, pcx));
        }
        if self.chart_graphics.is_some() {
            let cg = self.chart_graphics;
            panel.update(cx, |panel, pcx| panel.set_chart_graphics(cg, pcx));
        }
        if self.chart_labels.is_some() {
            let cl = self.chart_labels.clone();
            panel.update(cx, |panel, pcx| panel.set_chart_labels(cl, pcx));
        }
        if self.x_ppm.is_some() {
            let ppm = self.x_ppm;
            panel.update(cx, |panel, _| panel.set_default_x_ppm(ppm));
        }
        if let Some(sz) = self.show_zone {
            panel.update(cx, |panel, pcx| panel.set_show_zone(sz, pcx));
        }
        if let Some(ap) = self.auto_pin {
            panel.update(cx, |panel, pcx| panel.set_auto_pin(ap, pcx));
        }
        panel.update(cx, |panel, pcx| {
            panel.set_price_axis_pos(self.price_axis_pos.unwrap_or_default(), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_time_axis_visible(self.time_axis_visible.unwrap_or(true), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_line_labels(self.line_labels.unwrap_or(true), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_cursor_labels(self.cursor_labels.unwrap_or(true), pcx)
        });
        panel
    }

    /// Which kind's defaults the main chart follows: never a window, but it can be locked.
    pub(crate) fn default_kind(&self) -> moon_core::config::ChartTabKind {
        moon_core::config::ChartTabKind::of(false, self.compare_anchor.is_some())
    }

    /// Tell every chart which default it follows now. See `AddChartStack::sync_default_kind`.
    pub(super) fn sync_default_kind(&mut self, cx: &mut Context<Self>) {
        let kind = self.default_kind();
        if self.pushed_kind == Some(kind) {
            return;
        }
        self.pushed_kind = Some(kind);
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, pcx| panel.set_default_kind(kind, pcx));
        }
    }

    /// Synchronize comparison mode while preserving the active market across anchor reordering.
    ///
    /// Args:
    ///     cx: Stack context used to consume comparison requests and update panels.
    ///
    /// Returns:
    ///     Whether the comparison anchor, broom state, or chart order changed.
    pub(super) fn sync_compare(&mut self, cx: &mut Context<Self>) -> bool {
        let previous_active = self.active;
        let active_key = previous_active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.clone()));
        let changed = sync_compare(
            &mut self.charts,
            &mut self.compare_anchor,
            &mut self.compare_y,
            &mut self.compare_orderbook_only,
            self.layout_orientation,
            cx,
        );
        if changed {
            self.active = remap_active_index(
                self.charts.len(),
                active_key.as_ref(),
                previous_active,
                |ix, key| self.charts.get(ix).is_some_and(|entry| entry.is(key)),
            );
        }
        // Unconditional, like the Add stack's: `changed` reports a moved chart LIST, while the lock
        // itself can go on or off without one, and it is the lock that picks the default.
        self.sync_default_kind(cx);
        changed
    }

    /// Move fullscreen focus to the next chart cyclically for the `switch_charts` hotkey.
    /// With fewer than two charts there is nothing to switch. This leaves whole-stack mode, just
    /// like regular chart focus, and synchronizes the group's active trading target.
    ///
    /// Args:
    ///     cx: Stack context used to update visibility, open markets, and observers.
    ///
    /// Returns:
    ///     Nothing; stacks with fewer than two charts are unchanged.
    pub(crate) fn cycle_active(&mut self, cx: &mut Context<Self>) {
        if self.charts.len() < 2 {
            return;
        }
        let cur = self.active.unwrap_or(0);
        let next = (cur + 1) % self.charts.len();
        self.active = Some(next);
        self.show_stack = false;
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        self.arm_idle_timer(cx);
        cx.notify();
    }

    /// Focus an existing Main chart or create it when the core-market pair is not open.
    ///
    /// Args:
    ///     core: Live core that owns the market.
    ///     market: Canonical market name.
    ///     history: Default or published Report durable-history scope for this exact target.
    ///     cx: Stack context used to create panels and publish open markets.
    ///
    /// Returns:
    ///     Nothing; the requested chart becomes the fullscreen active entry.
    pub(in crate::chart_tabs) fn open_or_focus(
        &mut self,
        core: CoreId,
        market: String,
        history: crate::backend::ChartHistoryScope,
        cx: &mut Context<Self>,
    ) {
        let key = (core, market.clone());
        if let Some(index) = self.index_of(&key) {
            self.charts[index].panel.update(cx, |panel, panel_cx| {
                panel.apply_history_scope(core, market.clone(), history.clone(), panel_cx);
            });
            // Already open: this is the same selection the tab row performs, plus the idle timer
            // an explicit open restarts.
            self.select_market(&key, true, cx);
            self.arm_idle_timer(cx);
            return;
        }

        let panel = self.create_panel(core, &market, cx);
        panel.update(cx, |panel, panel_cx| {
            panel.apply_history_scope(core, market.clone(), history, panel_cx);
        });
        self.charts
            .push(ChartStackEntry::new(core, market, panel, SlotOwner::Reader));
        self.active = Some(self.charts.len() - 1);
        self.show_stack = false;
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        // In comparison mode, a new chart immediately receives eligibility and the shared Y range.
        self.sync_compare(cx);
        self.arm_idle_timer(cx);
        cx.notify();
    }

    /// Focus an existing Main chart or replace the active slot for an Auto rail selection.
    ///
    /// Unlike [`Self::open_or_focus`], a missing target never appends beside the current chart.
    /// Replacing the active entry keeps the stack count and position stable while the shared
    /// teardown releases the old chart's panes, market interest, and comparison lock.
    ///
    /// Args:
    ///     core: Selected Auto workspace core.
    ///     market: Canonical target-core market chosen by the parent controller.
    ///     cx: Stack context used to focus or replace the active chart and publish state.
    ///
    /// Returns:
    ///     Nothing; the chosen target becomes the fullscreen active Main entry.
    pub(in crate::chart_tabs) fn replace_or_focus(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        let key = (core, market.clone());
        if self.index_of(&key).is_some() {
            self.select_market(&key, true, cx);
            self.arm_idle_timer(cx);
            return;
        }
        let Some(active) = self.active.filter(|index| *index < self.charts.len()) else {
            self.open_or_focus(core, market, crate::backend::ChartHistoryScope::Default, cx);
            return;
        };
        self.remove_chart_at(active, cx);
        let panel = self.create_panel(core, &market, cx);
        panel.update(cx, |panel, panel_cx| {
            panel.apply_history_scope(
                core,
                market.clone(),
                crate::backend::ChartHistoryScope::Default,
                panel_cx,
            );
        });
        self.charts.insert(
            active,
            ChartStackEntry::new(core, market, panel, SlotOwner::Reader),
        );
        self.active = Some(active);
        self.show_stack = false;
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        self.sync_compare(cx);
        self.arm_idle_timer(cx);
        cx.notify();
    }

    /// Remove panels whose own panes are empty while preserving the active market identity.
    ///
    /// Args:
    ///     cx: Application context used to inspect child-panel state.
    ///
    /// Returns:
    ///     Whether at least one empty panel was removed.
    pub(super) fn prune_empty(&mut self, cx: &App) -> bool {
        let previous_active = self.active;
        let active_key = previous_active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.clone()));
        let changed = retain_nonempty_panels(&mut self.charts, cx);
        self.active = remap_active_index(
            self.charts.len(),
            active_key.as_ref(),
            previous_active,
            |ix, key| self.charts.get(ix).is_some_and(|entry| entry.is(key)),
        );
        if self.active.is_none() {
            self.show_stack = false;
        }
        changed
    }
}
