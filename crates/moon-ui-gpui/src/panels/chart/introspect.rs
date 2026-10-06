//! Chart panel introspect operations.

use super::*;

impl ChartPanel {
    /// Returns the number of open chart panes for the tab's count badge.
    pub fn pane_count(&self) -> usize {
        self.chart.pane_count()
    }

    /// When this panel's stalest auto-added pane last had a detect, or `None` when no pane of it
    /// may be evicted; see `Container::stalest_detect_ms` for why the cap asks this and not a TTL
    /// deadline.
    pub fn stalest_detect_ms(&self) -> Option<f64> {
        self.chart.stalest_detect_ms()
    }

    /// Returns whether any pane is pinned. The stack sorts pinned charts first, and
    /// `prune_ttl` skips them.
    pub fn is_pinned(&self) -> bool {
        (0..self.chart.pane_count()).any(|i| self.chart.pane_pinned(i))
    }

    /// Idempotently pins every pane, as required for charts restored into a custom tab. Pinning
    /// cancels TTL auto-close, so the deadline timer is re-armed for any remaining unpinned pane.
    pub fn ensure_pinned(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for i in 0..self.chart.pane_count() {
            if !self.chart.pane_pinned(i) && self.chart.toggle_pane_pin(i) {
                changed = true;
            }
        }
        if changed {
            self.view_dirty = true;
            self.arm_ttl_timer(cx);
            cx.notify();
        }
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn debug_data_handle(&self) -> crate::chartdx::ChartDataHandle {
        self.chart.data_handle()
    }

    /// This panel's chart-slot rectangle, in window-relative LOGICAL pixels, plus the device
    /// pixels per logical pixel and the slot's size already in DEVICE pixels.
    ///
    /// The slot is the `gpu_canvas` element: plot, price axis, time axis, order book and the corner
    /// caption, and none of the panel's GPUI chrome around them. That is exactly the region the
    /// chart shot copies, which is why this accessor exists at all - `self.chart` is private to
    /// this module tree and `shot` is the one caller from outside it.
    ///
    /// `None` before the first paint: the geometry is published by the GPU canvas per frame
    /// (`chartdx::data_state::state::apply_slot_geometry`), so a chart in a tab that has never been
    /// shown has none yet.
    ///
    /// Returns:
    ///     Logical bounds, the window scale, and the engine's physical slot size after a paint.
    pub(crate) fn shot_geometry(&self) -> Option<(Bounds<Pixels>, f32, (u32, u32))> {
        self.chart.slot_geometry()
    }

    /// Everything the shot's burnt-in header states, snapshotted in ONE read.
    ///
    /// Here for the same reason [`Self::shot_geometry`] is, and for one more. `self.chart` and the
    /// panel's own overrides are private to this module tree, so the shot could not reach any of
    /// this — but more importantly, this metadata BELONGS to the chart: the ticker is the
    /// renderer's cached caption value, the venue is the exact string the privacy substitution
    /// puts on the picture, the movement windows are addressed by a catalogue order only this
    /// layer knows, and the timeframe is an override whose `None` means "follow the global
    /// default". Reassembling all of that at the capture site would put the same knowledge in two
    /// places and let the burnt-in line disagree with the chart under it.
    ///
    /// Read at CAPTURE time rather than when the hotkey was pressed: about a dozen frame callbacks
    /// separate the two while the substituted caption reaches the screen, and the header must
    /// describe the frame that was actually photographed.
    ///
    /// Args:
    ///     backend: Live backend, read in the same callback as the capture.
    ///
    /// Returns:
    ///     The header's inputs. Absent figures stay absent — a market whose history has not
    ///     answered is reported as unknown, never as zero.
    pub(crate) fn shot_inputs(&self, backend: &Backend) -> shot::ShotInputs {
        let theme = backend
            .preview
            .as_ref()
            .unwrap_or(&backend.config)
            .chart_theme();
        let target = self.active_target();
        let venue = target
            .as_ref()
            .map(|(core, _)| backend.session.core_venues().get(core))
            .map(crate::controls::venue_section_label)
            .unwrap_or_default();
        // Addressed through `LabelWindow::index()` rather than by literal 2/4/5. The readout is
        // indexed by `LabelWindow::ALL`'s order and the catalogue says so itself; a second copy of
        // that mapping here would be free to drift, and the symptom would be a header quietly
        // stating the 30-minute move under a "1h" label.
        let windows = target.as_ref().and_then(|(core, market)| {
            backend
                .session
                .market_source()
                .market_windows(*core, market)
        });
        let delta = |window: moon_core::config::LabelWindow| -> Option<f64> {
            windows.as_ref()?.windows[window.index()].delta_pct
        };
        shot::ShotInputs {
            coin: self
                .pane_ticker()
                .or_else(|| target.map(|(_, market)| market)),
            venue,
            // `None` follows the global default, which is the same resolution this panel already
            // performs when it is constructed.
            tf_min: self
                .candle_view
                .unwrap_or_else(|| backend.layout.candle_view_for(self.default_kind))
                .tf_min,
            bg: theme.bg,
            // The chart's own supporting-text colour, NOT a hard-coded dark. `bg` is
            // user-configurable and dark by default, so fixed dark text would be invisible for
            // most users; this is the one colour guaranteed to read against `bg` in every theme.
            text: theme.axis_label,
            delta_3h: delta(moon_core::config::LabelWindow::H3),
            delta_1h: delta(moon_core::config::LabelWindow::H1),
            delta_15m: delta(moon_core::config::LabelWindow::M15),
            // The chart's own badge, read from the renderer rather than derived here: the figure
            // and whether it is shown are decided in `chartdx`, and a second copy of that decision
            // would be free to disagree with the badge inside the picture.
            scale_pct: self.chart.scale_badge(),
            time_scale_s: self.chart.time_scale_secs(),
        }
    }

    /// Arm or clear the corner caption's shot substitution: the EXCHANGE in place of the core name.
    ///
    /// Here for the same reason [`Self::shot_geometry`] is — `self.chart` is private to this module
    /// tree and `shot` is the one caller from outside it. The screen keeps the core name at every
    /// other moment: it is what lets the user tell his own charts apart, and it is also his own
    /// account label, which is why the picture he shares must not carry it.
    ///
    /// `until` is a deadline rather than a duration because the renderer expires it from wall clock
    /// with no timer of its own. The shot clears it explicitly on every path; the deadline is the
    /// watchdog behind that, for a chain that never completes.
    ///
    /// Must not be called from inside a `ChartPanel` update — it needs its own `update` on the
    /// panel entity, and gpui refuses the re-entrant borrow. Both current `ChartShot` call sites
    /// dispatch from other entities, which is what makes this safe today.
    ///
    /// Arming FORCES an order sync first, and that is load-bearing rather than tidy. The caption's
    /// strings are rebuilt on the SYNC paths, never on the frame path, so without a sync the
    /// substitution would not reach the labels at all. `sync_orders_if_visible` normally returns
    /// early unless `order_signature` moved, and that signature is built only from the target
    /// core's `order_lines_rev`; a venue arrives on `FeedMsg::Identity`, which never touches that
    /// revision, so without the force a core identified after the last order change would be
    /// captured as the shared "not identified" wording instead of its exchange.
    ///
    /// Args:
    ///     until: Deadline past which the caption restores itself, or `None` to restore it now.
    ///     cx: Panel context, used to re-read the session when arming.
    ///
    /// Returns:
    ///     Whether anything changed, meaning a repaint was requested.
    pub(crate) fn arm_shot_caption(
        &mut self,
        until: Option<Instant>,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.chart.arm_shot_caption(until);
        if changed {
            self.sync_orders_if_visible(cx, true);
        }
        changed
    }

    /// Whether the substituted caption has satisfied the renderer-side pre-capture proof.
    ///
    /// The shot's capture gate. `false` means the renderer-side proof is incomplete, so the caller
    /// must wait or give up rather than read the desktop.
    ///
    /// Returns:
    ///     `true` once enough completed text passes have drawn the exchange caption.
    pub(crate) fn shot_caption_drawn(&self) -> bool {
        self.chart.shot_caption_drawn()
    }

    /// Which arming of the shot caption is currently in force.
    ///
    /// A shot compares this against the value it saw when it armed. A newer one means a second
    /// press replaced this shot, and the older chain should stand down quietly rather than report
    /// a failure for a picture somebody else is now taking.
    ///
    /// Returns:
    ///     The current arming generation.
    pub(crate) fn shot_caption_gen(&self) -> u64 {
        self.chart.shot_caption_gen()
    }

    /// Mark whether this panel is part of the currently rendered GPUI scene. This only gates
    /// CPU-side data prepare; the `gpu_canvas` element lifetime is still owned by GPUI scene replay.
    pub fn set_scene_visible(&mut self, visible: bool) {
        self.scene_visible = visible;
        self.chart.set_scene_visible(visible);
    }
}
