//! Chart panel construct operations.

use super::*;

impl ChartPanel {
    pub(super) fn sync_orders_from_backend_notify(&mut self, cx: &mut Context<Self>) -> bool {
        crate::diag::bump(&crate::diag::CHART_ORDER_SYNC);
        {
            let b = self.backend.read(cx);
            self.chart.sync_orders_if_visible(&b.session, false);
        }
        self.sweep_cancel_hold(cx);
        // A figure EDITED anywhere — this window's settings panel, another window's, a hotkey —
        // bumps the store's revision but no order does, and the userdata rebuild below is gated on
        // the order signature. Without this the new colour would wait for an unrelated order tick;
        // on a quiet market that is a long time to look at a stale figure.
        let fig_rev = self.backend.read(cx).figures.borrow().rev();
        let figures_changed = self.last_fig_store_rev != fig_rev;
        // A figure can only have STOPPED existing on a revision change, and the check walks the
        // market's figures — so it rides that gate rather than every notify. Before the publish
        // below, which is what carries `hovered` to the engine.
        if figures_changed {
            self.drop_dead_fig_hover(cx);
        }
        // Tool and selection state changes in the tab strip or through hotkeys reach this panel
        // through the backend observer; propagate them into the figure engine.
        self.sync_fig_visual(cx);
        if figures_changed {
            self.last_fig_store_rev = fig_rev;
            self.fig_resync(cx);
        }
        self.drop_stale_fig_settings(cx);
        // News arrives on the same observer; the signature gate makes the common case a no-op. Its
        // gems are own-pass and present themselves, so a news item never wakes the GPUI scene — an
        // open hover card catches up on the next repaint.
        self.sync_news_marks(cx);
        self.sync_warn_marks(cx);
        // The top-left overlay needs no revision gate of its own: it reads the live order rows when
        // GPUI renders. Structural order changes advance `order_lines_rev` through
        // `OrderLineStore::update`, and `ChartDataState::order_signature` already gates that
        // per-pane work. A mark-price-only update follows the market signature and its throttled
        // repaint instead, so polling `orders_table_rev` here would add an aggregate pass without
        // covering an input the existing signatures do not already schedule.
        self.clear_settled_order_drag_preview(cx) && self.apply_order_visual(cx)
    }

    pub fn new(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_main(backend, None, focus_open, epoch, theme, cx)
    }

    /// Construct a chart that is a HISTORICAL VIEWER: no order book, no trading controls.
    ///
    /// The trade-detail window's constructor. It differs from [`Self::new`] in exactly what the
    /// [`Self::historical`] field documents, and the difference is structural rather than a
    /// setting: nothing this panel is later told can put a live order book or a `Panic Sell` button
    /// back onto a picture of a trade that closed hours ago.
    ///
    /// A separate constructor rather than a parameter on [`Self::new_main`] so the live call sites
    /// keep the signatures they have; the flag is unreachable from any of them.
    ///
    /// Args:
    ///     backend: Shared application state and session command surface.
    ///     focus_open: Optional initial core and market.
    ///     epoch: Chart time origin.
    ///     theme: Runtime chart theme.
    ///     cx: Panel context used for observers and market references.
    ///
    /// Returns:
    ///     A chart panel holding no order-book demand and drawing no market action.
    pub fn new_historical(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        // The captions this window opens with are its OWN kind's, not the main chart's: a frozen
        // market cannot be read with captions that state what is happening right now. Asked for at
        // construction, so the panel is never briefly wearing Main's.
        let mut panel = Self::new_with_kind(
            backend,
            None,
            focus_open,
            epoch,
            theme,
            moon_core::config::ChartTabKind::Trade,
            cx,
        );
        panel.historical = true;
        // The engine must know too, and not only the panel: `Self::render` applies the
        // application-wide Live flag to whatever engine it is drawing, so a viewer that is
        // historical only at the panel level is still dragged back to the live edge one frame
        // after its interval is framed. Telling the engine here makes the immunity structural
        // rather than a rule every future Live call site has to remember.
        panel.chart.set_historical(true);
        // Turned off through the same field the live path uses, so the engine's own layout gives
        // the book's width back to the plot (`pane_layout` gives the book zero width).
        panel.orderbook_enabled = false;
        // `new_main` retained a book reference for the focus market a moment ago; this releases it
        // before any coordination tick can read the demand set, so a historical viewer never
        // subscribes to a live book at all.
        panel.sync_orderbook_refs(cx);
        panel.sync_chart_text(cx);
        // The dim control strip marks where an order-placement click lands. There is no order
        // placement here, so shading a strip for it would be a leftover of the thing just removed.
        panel.show_zone = false;
        panel.view_dirty = true;
        panel
    }

    /// Construct a Main chart with an optional group-scoped Auto action authority.
    ///
    /// Args:
    ///     backend: Shared application state and session command surface.
    ///     workspace_group: Owning group for rail-authority checks, or `None` for diagnostics.
    ///     focus_open: Optional initial core and market.
    ///     epoch: Chart time origin.
    ///     theme: Runtime chart theme.
    ///     cx: Panel context used for observers and market references.
    ///
    /// Returns:
    ///     A fully initialized Main chart panel.
    pub fn new_main(
        backend: Entity<Backend>,
        workspace_group: Option<String>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_kind(
            backend,
            workspace_group,
            focus_open,
            epoch,
            theme,
            moon_core::config::ChartTabKind::Main,
            cx,
        )
    }

    /// Build a single-pane panel that follows ONE kind's defaults.
    ///
    /// The kind is a parameter rather than something assigned afterwards: a panel's effective
    /// settings are minted from it during construction, so a panel built as one kind and corrected
    /// to another would build that signature twice and be briefly inconsistent in between.
    ///
    /// Args:
    ///     backend: Shared application state and session command surface.
    ///     workspace_group: Owning group for rail-authority checks, or `None` for diagnostics.
    ///     focus_open: Optional initial core and market.
    ///     epoch: Chart time origin.
    ///     theme: Runtime chart theme.
    ///     kind: Which kind's defaults this panel follows.
    ///     cx: Panel context used for observers and market references.
    ///
    /// Returns:
    ///     A fully initialized panel.
    pub(super) fn new_with_kind(
        backend: Entity<Backend>,
        workspace_group: Option<String>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        kind: moon_core::config::ChartTabKind,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartEngine::new(epoch, theme);
        chart.set_market_source(Some(backend.read(cx).session.market_source()));
        chart.set_figures_store(backend.read(cx).figures.clone());
        let market_ref_epoch = backend.read(cx).chart_market_refs_epoch;
        let mut market = None;
        let mut registered_markets = HashSet::new();
        let mut registered_orderbook = HashSet::new();
        if let Some((core, m)) = focus_open {
            chart.open(core, &m);
            market = Some(m.clone());
            registered_markets.insert((core, m.clone()));
            // The order book starts enabled, so retain its demand reference immediately.
            registered_orderbook.insert((core, m.clone()));
            backend.update(cx, |b, _| {
                b.retain_chart_market(core, &m);
                b.retain_chart_orderbook(core, &m);
            });
        }
        // A fresh panel holds no override yet, so its effective values ARE its kind's defaults.
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(b, None, None, None, kind)
        };
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _revision, cx| {
            this.chart.invalidate_display_time();
            this.view_dirty = true;
            cx.notify();
        })
        .detach();
        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _revision, cx| {
            this.requery_trade_history_on_generation(cx);
        })
        .detach();
        // The trace resolver's wake: one stamp compare per notification, a rebuild of the lines
        // map only when a trade of THIS chart's changed. Cheap in the arrows style — it exits on
        // the style before touching the history.
        let traces_revision = backend.read(cx).traces_revision();
        cx.observe(&traces_revision, |this, _revision, cx| {
            this.sync_trace_lines(false, cx);
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            this.requery_trade_history_on_core_scope(cx);
        })
        .detach();
        let market_data_revision = backend.read(cx).market_data_revision();
        cx.observe(&market_data_revision, |this, _revision, cx| {
            // A sibling's catalog can become resolvable without a Backend notification or a
            // workspace change. In PerCore that catalog is its own, so the admitted aliases have
            // to be collected again or that sibling never enters the read.
            this.requery_trade_history_on_core_scope(cx);
        })
        .detach();
        // Notify for setting changes and infrequent axis text. Frequent market data bypasses GPUI
        // notification because `gpu_canvas.frame()` reads MarketDataSource directly. A local
        // one-shot timer handles time-based pane TTL independently of backend observations.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::CHART_OBS_FIRE);
            let now = Instant::now();
            let (sig, settings_sig, panic_rev, fav_rev) = {
                let b = backend.read(cx);
                let sig = if this.scene_visible {
                    this.chart.notify_signature(&b.session)
                } else {
                    this.data_sig
                };
                (
                    sig,
                    chart_settings_sig(
                        b,
                        this.chart_graphics,
                        this.candle_view,
                        this.chart_labels.clone(),
                        this.default_kind,
                    ),
                    b.panic_rev,
                    b.fav_rev,
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                // A panel with NO override follows `layout.chart_graphics`, which a ⧉ press in
                // another group window rewrites without ever walking this stack. This is the only
                // place such a panel hears about it, so the trade-kind re-query hangs here too; it
                // returns immediately unless that pair actually moved.
                this.requery_trade_history_on_trade_kinds(cx);
                this.requery_trade_history_on_core_scope(cx);
                // ...and the trade style lives beside them: a default flipped to lines elsewhere
                // has to start resolving here too.
                this.request_trace_lines(true, cx);
                this.sync_trace_lines(true, cx);
                this.sync_chart_text(cx);
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            // Both market-button overrides in ONE condition: they settle on the same coordination
            // tick, and two conditions would notify twice and count two repaints in
            // `CHART_OBS_NOTIFY` for the one that actually happens. Both controls are GPUI overlay
            // elements rather than chart-engine geometry, so a plain notify is the whole repaint --
            // deliberately not setting `view_dirty`.
            if this.last_panic_rev != panic_rev || this.last_fav_rev != fav_rev {
                this.last_panic_rev = panic_rev;
                this.last_fav_rev = fav_rev;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            if this.sync_orders_from_backend_notify(cx) {
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            if this.scene_visible {
                this.data_sig = sig;
            }
            // Axis overlay only. The own pass presents market data itself. A hidden chart
            // has no canvas, so a live-data notice must not wake the window. Fast panels
            // floor at 250 ms and slow panels at 1 s. Settings and orders above stay immediate.
            let floor = if this.fast {
                Duration::from_millis(250)
            } else {
                Duration::from_millis(1_000)
            };
            if chart_axis_notify_due(
                this.scene_visible,
                sig != this.last_axis_notify_data_sig,
                this.last_adaptive_notify_at,
                now,
                floor,
            ) {
                this.last_axis_notify_data_sig = sig;
                this.last_adaptive_notify_at = Some(now);
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let chart_handle = chart.data_handle();
        backend.update(cx, |b, _| b.register_chart_consumer(chart_handle));
        cx.on_release(|this, cx| {
            this.release_all_market_refs(cx);
        })
        .detach();
        Self {
            backend,
            default_kind: kind,
            workspace_group,
            chart,
            input: input::ChartInput::default(),
            market,
            scale: None,
            orderbook_enabled: true,
            candle_view: None,
            chart_graphics: None,
            chart_labels: None,
            pending_labels: None,
            show_zone: true,
            auto_pin: false,
            market_actions_pushed: false,
            historical: false,
            last_chart_text_sent: None,
            price_axis_pos: Default::default(),
            time_axis_visible: true,
            line_labels: true,
            cursor_labels: true,
            num: None,
            registered_markets,
            registered_orderbook,
            market_ref_epoch,
            data_sig: 0,
            settings_sig,
            fast: true,
            scene_visible: false,
            main_stack_scroll: false,
            last_axis_notify_data_sig: u64::MAX,
            compare_eligible: false,
            is_compare_anchor: false,
            compare_lock_pending: false,
            locked_y: None,
            orderbook_only: false,
            compare_broom_pending: false,
            compare_broom_on: false,
            ghost_peers: Vec::new(),
            view_dirty: true,
            camera_dirty: false,
            last_adaptive_notify_at: None,
            last_ppp: 1.0,
            label_wheel_accum: 0.0,
            label_wheel_target: None,
            ttl_timer_armed: false,
            order_drag: None,
            pending_order_drag: None,
            book_zone_press: None,
            order_hover: None,
            order_hover_probe: None,
            drag_notify_at: None,
            drag_notify_pending: false,
            fig_hover_probe: None,
            fig_draft_probe: None,
            news: news::NewsState::default(),
            warn: warn::WarnState::default(),
            report_trades: report_trades::ReportTradesState::default(),
            trace_lines: trace_lines::TraceLinesState::default(),
            trade_hover: trade_history_hover::TradeHoverState::default(),
            ruler: None,
            fig_draft: None,
            fig_settings: None,
            last_fig_store_rev: 0,
            last_panic_rev: 0,
            last_fav_rev: 0,
            fig_hover: None,
            fig_drag: None,
            suppress_rmb_up: false,
            click_series: ClickSeries::default(),
            focus: cx.focus_handle(),
        }
    }

    pub fn active_target(&self) -> Option<(CoreId, String)> {
        self.chart.active_target()
    }

    /// Check whether the current workspace still authorizes one chart core.
    ///
    /// Args:
    ///     backend: Live state read in the same callback that will dispatch the side effect.
    ///     core: Chart core targeted by the action.
    ///
    /// Returns:
    ///     `false` only when this panel belongs to an Auto group whose rail selected another core.
    pub(super) fn workspace_action_allowed(&self, backend: &Backend, core: CoreId) -> bool {
        backend.workspace_action_allows_core(self.workspace_group.as_deref(), core)
    }

    /// Builds numbered AddToChart or Custom panel `num` with its owning workspace authority.
    ///
    /// Detections populate AddToChart panels and manual selection or restoration populates Custom
    /// panels through [`Self::add_coin`]. It needs no `Window`, allowing deferred restoration of
    /// detached windows from data alone.
    pub fn new_addto(
        backend: Entity<Backend>,
        workspace_group: String,
        num: u32,
        bucket: ChartBucket,
        epoch: f64,
        theme: ChartTheme,
        kind: moon_core::config::ChartTabKind,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartEngine::new_kind(epoch, theme, ContainerKind::Chart { num, bucket });
        chart.set_market_source(Some(backend.read(cx).session.market_source()));
        chart.set_figures_store(backend.read(cx).figures.clone());
        let market_ref_epoch = backend.read(cx).chart_market_refs_epoch;
        // A fresh panel holds no override yet, so its effective values ARE the global defaults.
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(b, None, None, None, kind)
        };
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _revision, cx| {
            this.chart.invalidate_display_time();
            this.view_dirty = true;
            cx.notify();
        })
        .detach();
        // Stack tiles carry durable trade history too, so they need the same refresh edge Main
        // has: without it a tile's arrows are a snapshot taken when the tile appeared, and a trade
        // closing while it is on screen — on the very market a detect flagged — never draws.
        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _revision, cx| {
            this.requery_trade_history_on_generation(cx);
        })
        .detach();
        // The trace resolver's wake: one stamp compare per notification, a rebuild of the lines
        // map only when a trade of THIS chart's changed. Cheap in the arrows style — it exits on
        // the style before touching the history.
        let traces_revision = backend.read(cx).traces_revision();
        cx.observe(&traces_revision, |this, _revision, cx| {
            this.sync_trace_lines(false, cx);
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            this.requery_trade_history_on_core_scope(cx);
        })
        .detach();
        let market_data_revision = backend.read(cx).market_data_revision();
        cx.observe(&market_data_revision, |this, _revision, cx| {
            // A sibling's catalog can become resolvable without a Backend notification or a
            // workspace change. In PerCore that catalog is its own, so the admitted aliases have
            // to be collected again or that sibling never enters the read.
            this.requery_trade_history_on_core_scope(cx);
        })
        .detach();
        cx.observe(&backend, |this, backend, cx| {
            let now = Instant::now();
            let (sig, settings_sig, panic_rev, fav_rev) = {
                let b = backend.read(cx);
                let sig = if this.scene_visible {
                    this.chart.notify_signature(&b.session)
                } else {
                    this.data_sig
                };
                (
                    sig,
                    chart_settings_sig(
                        b,
                        this.chart_graphics,
                        this.candle_view,
                        this.chart_labels.clone(),
                        this.default_kind,
                    ),
                    b.panic_rev,
                    b.fav_rev,
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                // Same reason as the twin observer in `new_main`: a panel with no override of its
                // own hears a ⧉ press from another group window only here, and the durable history
                // query was narrowed by the previous trade-kind pair.
                this.requery_trade_history_on_trade_kinds(cx);
                this.requery_trade_history_on_core_scope(cx);
                // ...and the trade style lives beside them: a default flipped to lines elsewhere
                // has to start resolving here too.
                this.request_trace_lines(true, cx);
                this.sync_trace_lines(true, cx);
                this.sync_chart_text(cx);
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            // One condition for both overrides; see the twin observer in `new_main`. Numbered
            // AddToChart and Custom panels are today's worst case at 1 Hz -- this is deliberately
            // ahead of that throttle.
            if this.last_panic_rev != panic_rev || this.last_fav_rev != fav_rev {
                this.last_panic_rev = panic_rev;
                this.last_fav_rev = fav_rev;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            if this.sync_orders_from_backend_notify(cx) {
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            if this.scene_visible {
                this.data_sig = sig;
            }
            // Numbered panels cap the axis overlay at 1 Hz. A hidden tile still receives this
            // observation, and the helper refuses the wake. The TTL timer prunes unpinned panes
            // on its own clock. Settings and orders above stay immediate.
            if chart_axis_notify_due(
                this.scene_visible,
                sig != this.last_axis_notify_data_sig,
                this.last_adaptive_notify_at,
                now,
                Duration::from_millis(1_000),
            ) {
                this.last_axis_notify_data_sig = sig;
                this.last_adaptive_notify_at = Some(now);
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let chart_handle = chart.data_handle();
        backend.update(cx, |b, _| b.register_chart_consumer(chart_handle));
        cx.on_release(|this, cx| {
            this.release_all_market_refs(cx);
        })
        .detach();
        Self {
            backend,
            default_kind: kind,
            workspace_group: Some(workspace_group),
            chart,
            input: input::ChartInput::default(),
            market: None,
            scale: None,
            orderbook_enabled: true,
            candle_view: None,
            chart_graphics: None,
            chart_labels: None,
            pending_labels: None,
            show_zone: true,
            auto_pin: false,
            market_actions_pushed: false,
            historical: false,
            last_chart_text_sent: None,
            price_axis_pos: Default::default(),
            time_axis_visible: true,
            line_labels: true,
            cursor_labels: true,
            num: Some(num),
            registered_markets: HashSet::new(),
            registered_orderbook: HashSet::new(),
            market_ref_epoch,
            data_sig: 0,
            settings_sig,
            fast: false,
            scene_visible: false,
            main_stack_scroll: false,
            last_axis_notify_data_sig: u64::MAX,
            compare_eligible: false,
            is_compare_anchor: false,
            compare_lock_pending: false,
            locked_y: None,
            orderbook_only: false,
            compare_broom_pending: false,
            compare_broom_on: false,
            ghost_peers: Vec::new(),
            view_dirty: true,
            camera_dirty: false,
            last_adaptive_notify_at: None,
            last_ppp: 1.0,
            label_wheel_accum: 0.0,
            label_wheel_target: None,
            ttl_timer_armed: false,
            order_drag: None,
            pending_order_drag: None,
            book_zone_press: None,
            order_hover: None,
            order_hover_probe: None,
            drag_notify_at: None,
            drag_notify_pending: false,
            fig_hover_probe: None,
            fig_draft_probe: None,
            news: news::NewsState::default(),
            warn: warn::WarnState::default(),
            report_trades: report_trades::ReportTradesState::default(),
            trace_lines: trace_lines::TraceLinesState::default(),
            trade_hover: trade_history_hover::TradeHoverState::default(),
            ruler: None,
            fig_draft: None,
            fig_settings: None,
            last_fig_store_rev: 0,
            last_panic_rev: 0,
            last_fav_rev: 0,
            fig_hover: None,
            fig_drag: None,
            suppress_rmb_up: false,
            click_series: ClickSeries::default(),
            focus: cx.focus_handle(),
        }
    }
}
