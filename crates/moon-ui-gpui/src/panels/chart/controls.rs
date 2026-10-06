//! Chart panel controls operations.

use super::*;

impl ChartPanel {
    /// Step time zoom at each plot center using the Ctrl+Shift+wheel floor and pane layout.
    pub(crate) fn super_zoom(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        if self.orderbook_only {
            return;
        }
        let Some((_, sf, _)) = self.chart.slot_geometry() else {
            return;
        };
        let fb = self.chart.slot_dev_width();
        let input = &self.input;
        let changed = self
            .chart
            .with_container_mut(|container| input.super_zoom(zoom_in, container, fb, sf));
        if changed {
            self.mark_input_changed(cx);
            cx.notify();
        }
    }

    /// Moonbot's Ctrl+Right for every pane of this panel: back to the price and the live edge on
    /// the scale already chosen — see `ChartView::center_on_price` for what is and is not touched.
    ///
    /// Goes through `mark_input_changed` like a gesture would, so the resumed follow reaches the
    /// toolbar's Live flag the same way a pan's pull-back does.
    ///
    /// A comparison-locked FOLLOWER keeps its Y: `render` re-imposes the anchor's window next frame
    /// (`set_locked_y`), which is the lock doing its job — the anchor recentres, and the follower
    /// tracks it. Only the return to the live edge is this pane's own there.
    ///
    /// Args:
    ///     cx: Panel context.
    pub(crate) fn center_on_price(&mut self, cx: &mut Context<Self>) {
        if self.chart.center_on_price(now_unix_ms()) {
            self.mark_input_changed(cx);
            cx.notify();
        }
    }

    /// Sets this panel's price scale, where `None` means Auto. Rendering applies it through the
    /// engine's `set_scale`.
    pub fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        if self.scale != pct {
            self.scale = pct;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// This panel's configured price scale, where `None` means Auto.
    ///
    /// What was PICKED, which is what a scale control must state. It is not a readout of the
    /// window currently drawn: a vertical drag or a right-button zoom moves that window without
    /// changing the choice, and reporting the dragged window instead would make the control's
    /// own label wander while the user is holding the mouse.
    ///
    /// Returns:
    ///     The configured scale, or `None` for Auto.
    pub(crate) fn scale(&self) -> Option<f32> {
        self.scale
    }

    /// Applies this panel's price scale even when the stored choice has not changed.
    ///
    /// [`Self::set_scale`] returns early on an unchanged value, and the engine caches on the same
    /// value again, so re-picking the preset already displayed is normally swallowed. That is the
    /// right economy while nothing else moves the Y window - but a vertical drag or a
    /// right-button zoom sets the view's own manual flag, which parks the pinned percentage, and
    /// then the one gesture a user would reach for to undo it, choosing the scale that is already
    /// shown, does nothing at all. Going through the engine's uncached path makes the visible
    /// choice always re-appliable.
    ///
    /// Args:
    ///     pct: The scale to apply, or `None` for Auto.
    ///     cx: Panel context.
    pub(crate) fn force_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        self.scale = pct;
        self.chart.reapply_scale(pct);
        self.view_dirty = true;
        cx.notify();
    }

    /// Enables or disables this window/tab's order book. Rendering applies the engine flag, and the
    /// backend order-book reference is synchronized for demand-driven subscription.
    pub fn set_orderbook_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        // A historical viewer has no live book to enable. Refused here rather than left to the
        // caller, so the absence is a property of the panel instead of a discipline every future
        // call site has to remember.
        if self.historical {
            return;
        }
        if self.orderbook_enabled != enabled {
            self.orderbook_enabled = enabled;
            self.view_dirty = true;
            self.sync_orderbook_refs(cx);
            self.drop_order_hover_if_disallowed(cx);
            cx.notify();
        }
    }

    /// The candle settings this panel actually draws with right now.
    ///
    /// `candle_view` is an OVERRIDE: `None` means "follow the global default for my kind of tab".
    /// A caller that wants to change ONE field of the effective settings has to start from the
    /// resolved value, or it silently discards every other choice the user made.
    ///
    /// The one other place that performs this resolution — the shot caption's `tf_min` — is left
    /// inline on purpose: it already holds `backend` borrowed for the surrounding literal, so
    /// calling this would re-borrow it through `cx`.
    ///
    /// Args:
    ///     cx: Application context used to read the global defaults.
    ///
    /// Returns:
    ///     The override when this panel has one, otherwise the global default for its kind.
    pub fn effective_candle_view(&self, cx: &App) -> moon_core::market::CandleViewCfg {
        self.candle_view.unwrap_or_else(|| {
            self.backend
                .read(cx)
                .layout
                .candle_view_for(self.default_kind)
        })
    }

    /// Sets this window/tab's candle and trade display settings. `None` uses the global default;
    /// rendering applies the effective value through the engine's `set_candle_view`.
    pub fn set_candle_view(
        &mut self,
        cfg: Option<moon_core::market::CandleViewCfg>,
        cx: &mut Context<Self>,
    ) {
        if self.candle_view != cfg {
            self.candle_view = cfg;
            self.view_dirty = true;
            // Restamp for the same reason `set_chart_graphics` does: the signature is derived from
            // this value, and a stale one turns the next backend notify into a phantom change.
            self.settings_sig = {
                let b = self.backend.read(cx);
                chart_settings_sig(
                    b,
                    self.chart_graphics,
                    cfg,
                    self.chart_labels.clone(),
                    self.default_kind,
                )
            };
            cx.notify();
        }
    }

    /// Sets this window/tab's chart-drawing settings. `None` uses the global default; rendering
    /// applies the effective value through the engine's `set_chart_graphics`.
    ///
    /// A change to the two trade-kind flags also re-runs the durable history query: the SQL that
    /// loads closed trades is narrowed by them, so the set already in memory answers to the PREVIOUS
    /// pair and re-ticking a flag would otherwise show nothing until the next target change.
    pub fn set_chart_graphics(
        &mut self,
        cfg: Option<moon_core::config::ChartGraphicsCfg>,
        cx: &mut Context<Self>,
    ) {
        if self.chart_graphics == cfg {
            return;
        }
        self.chart_graphics = cfg;
        self.view_dirty = true;
        // Restamp the signature: it is derived from this very value, so leaving it stale would make
        // the next backend notification report a settings change that has already been applied.
        self.settings_sig = {
            let b = self.backend.read(cx);
            chart_settings_sig(
                b,
                cfg,
                self.candle_view,
                self.chart_labels.clone(),
                self.default_kind,
            )
        };
        self.requery_trade_history_on_trade_kinds(cx);
        self.requery_trade_history_on_core_scope(cx);
        // The style may have flipped to lines: resolve and hand over what is already resolved.
        // The engine's own graphics update happens on render, before its next order pass.
        self.request_trace_lines(true, cx);
        self.sync_trace_lines(true, cx);
        cx.notify();
    }

    /// Sets this window/tab's chart captions. `None` uses the global default; rendering applies
    /// the effective value through the engine's `set_chart_labels`.
    pub fn set_chart_labels(
        &mut self,
        cfg: Option<moon_core::config::ChartLabelsCfg>,
        cx: &mut Context<Self>,
    ) {
        if self.chart_labels == cfg {
            return;
        }
        self.chart_labels = cfg.clone();
        self.view_dirty = true;
        // Restamp for the same reason `set_chart_graphics` does: the signature carries this value,
        // and a stale one turns the next backend notify into a phantom settings change.
        self.settings_sig = {
            let b = self.backend.read(cx);
            chart_settings_sig(
                b,
                self.chart_graphics,
                self.candle_view,
                cfg,
                self.default_kind,
            )
        };
        cx.notify();
    }

    /// Returns this panel's EFFECTIVE chart-drawing settings: its own override or its KIND's
    /// default, NORMALIZED to what the chart actually draws.
    ///
    /// Normalized because `layout.toml` is hand-editable and every reader of this config has to see
    /// the same clamped value the drawing path uses.
    pub(crate) fn effective_chart_graphics(&self, cx: &App) -> moon_core::config::ChartGraphicsCfg {
        moon_chart::normalize_chart_graphics(self.chart_graphics.unwrap_or_else(|| {
            self.backend
                .read(cx)
                .layout
                .chart_graphics_for(self.default_kind)
        }))
    }

    /// Handles Shift+middle-click by requesting Moonbot-style time-axis synchronization within this
    /// OS window. Backend carries the hovered pane's scale with the window handle; the tab strip or
    /// detached host applies it to that window's stacks and persists it in group layout or the
    /// detached tab specification.
    pub(in crate::panels) fn sync_x_scale_window(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ppm) = self.chart.pane_x_ppm(self.input.hovered_pane) else {
            return false;
        };
        let handle = window.window_handle();
        self.backend.update(cx, |b, bcx| {
            b.chart_x_sync = Some((handle, ppm));
            b.chart_x_sync_rev = b.chart_x_sync_rev.wrapping_add(1);
            bcx.notify();
        });
        true
    }

    /// Sets the default time-axis scale for new panes, supplied by the owning stack. `None` selects
    /// the built-in time-window default.
    pub fn set_default_x_ppm(&mut self, ppm: Option<f32>) {
        self.chart.set_default_x_ppm(ppm);
    }

    /// Applies a synchronized time-axis scale to every open pane in this panel.
    pub fn apply_x_ppm(&mut self, ppm: f32, cx: &mut Context<Self>) {
        if self.chart.set_x_ppm_all(ppm, now_unix_ms()) {
            // Shift+middle-click and apply-all are view-changing zooms: they restart the idle
            // Live timer the same way a wheel zoom does.
            self.mark_input_changed(cx);
            cx.notify();
        }
    }

    /// Controls whether a hidden book leaves the reserved order zone on this window/tab — the strip
    /// itself, not only its marker fill: off, together with a hidden book, the pane trades nothing
    /// (`order_gestures_allowed`). The engine is not involved; the zone is a hit-test concern.
    pub fn set_show_zone(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_zone != show {
            self.show_zone = show;
            self.drop_order_hover_if_disallowed(cx);
            cx.notify();
        }
    }

    /// Forget a line the pointer was resting on once the pane stops taking order gestures.
    ///
    /// `order_hover` is recomputed only when the pointer moves, and the presses read
    /// `hit_order_line`, which already answers nothing here — but the cursor shape and the line
    /// highlight are drawn from the stale field. A click on the layout popup has already taken the
    /// pointer off the canvas, and hover-out clears the field on its own; the flips that reach a
    /// RESTING pointer are the pointer-less ones — ⧉ apply-all, a tab spec restored on ingest, the
    /// book suspended when its window loses focus, the comparison broom lifted. Cleared here so the
    /// frame after any of them stops promising a grab.
    pub(super) fn drop_order_hover_if_disallowed(&mut self, cx: &mut Context<Self>) {
        if !self.order_gestures_allowed(cx) {
            self.set_order_interaction(None, cx);
        }
    }

    /// Sets the per-window/tab auto-pin flag. [`Self::try_place_order_click`] performs the pin after
    /// a successful order.
    pub fn set_auto_pin(&mut self, on: bool, _cx: &mut Context<Self>) {
        self.auto_pin = on;
    }

    /// Sets comparison eligibility for a horizontal tab, controlling lock-button visibility.
    pub fn set_compare_eligible(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.compare_eligible != on {
            self.compare_eligible = on;
            cx.notify();
        }
    }

    /// Marks this chart as the stack-controlled comparison anchor and highlights its lock.
    pub fn set_compare_anchor(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.is_compare_anchor != on {
            self.is_compare_anchor = on;
            cx.notify();
        }
    }

    /// Records a lock-button request and notifies the stack observer that consumes it.
    pub(super) fn request_compare_lock(&mut self, cx: &mut Context<Self>) {
        self.compare_lock_pending = true;
        cx.notify();
    }

    /// Takes and clears the pending lock-button request for the stack observer.
    pub fn take_compare_lock_request(&mut self) -> bool {
        std::mem::take(&mut self.compare_lock_pending)
    }

    /// Records a broom-button request for the stack observer to apply to anchor peers.
    pub(super) fn request_compare_broom(&mut self, cx: &mut Context<Self>) {
        self.compare_broom_pending = true;
        cx.notify();
    }

    /// Takes and clears the pending broom-button request.
    pub fn take_compare_broom_request(&mut self) -> bool {
        std::mem::take(&mut self.compare_broom_pending)
    }
}
