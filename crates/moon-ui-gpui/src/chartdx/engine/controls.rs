//! Chart visibility controls, historical framing and Live/Pause transitions.

use super::*;

impl ChartEngine {
    /// The graphics settings the currently uploaded geometry was built with.
    ///
    /// The hit test reads THIS rather than the backend config: the arrows on screen were baked
    /// with this value, and testing against a newer one would answer for arrows not yet drawn.
    pub(crate) fn chart_graphics(&self) -> moon_core::config::ChartGraphicsCfg {
        self.data.borrow().chart_graphics
    }

    /// Sets comparison book-only mode, hiding the plot and price axis while expanding the order book
    /// to the full width. Returns true on change.
    pub fn set_orderbook_only(&mut self, only: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.orderbook_only == only {
            return false;
        }
        data.orderbook_only = only;
        data.mark_view_dirty();
        true
    }

    /// Allows or forbids the horizontal-volume zone on every engine pane, whatever the tab
    /// configured; a follower of an active comparison lock forbids it. Returns true on change.
    pub fn set_hvol_allowed(&mut self, allowed: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.hvol_allowed == allowed {
            return false;
        }
        data.hvol_allowed = allowed;
        data.mark_view_dirty();
        true
    }

    /// Sets whether this panel draws the comparison lock on every engine pane, for the caption
    /// pass's corner-strip reservation. Returns true on change.
    pub fn set_compare_lock_shown(&mut self, shown: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.compare_lock_shown == shown {
            return false;
        }
        data.compare_lock_shown = shown;
        data.mark_view_dirty();
        true
    }

    /// Sets the per-window price-axis position for every engine pane. Returns true on change.
    pub fn set_price_axis_pos(
        &mut self,
        pos: crate::persistence::chart_persist::PriceAxisPos,
    ) -> bool {
        let mut data = self.data.borrow_mut();
        if data.price_axis_pos == pos {
            return false;
        }
        data.price_axis_pos = pos;
        data.mark_view_dirty();
        true
    }

    /// Sets per-window time-axis visibility for every engine pane. Returns true on change.
    pub fn set_time_axis_visible(&mut self, visible: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.time_axis_visible == visible {
            return false;
        }
        data.time_axis_visible = visible;
        data.mark_view_dirty();
        true
    }

    /// Toggles per-tab order-line labels. Returns true on change.
    pub fn set_line_labels(&mut self, show: bool) -> bool {
        let mut st = self.state.borrow_mut();
        if st.line_labels == show {
            return false;
        }
        st.line_labels = show;
        st.needs_present = true;
        true
    }

    /// Arms or clears the shot's exchange caption. Returns true on change.
    ///
    /// Args:
    ///     until: Deadline past which the caption returns to the core name by itself, or `None` to
    ///         restore it now.
    ///
    /// Returns:
    ///     Whether anything changed.
    pub(crate) fn arm_shot_caption(&mut self, until: Option<Instant>) -> bool {
        self.state.borrow_mut().arm_shot_caption(until)
    }

    /// Whether the armed exchange caption has satisfied the renderer-side pre-capture proof.
    ///
    /// Returns:
    ///     `true` once enough completed text passes have drawn the substituted caption since arming.
    pub(crate) fn shot_caption_drawn(&self) -> bool {
        self.state.borrow().shot_caption_drawn()
    }

    /// Which arming of the shot caption is in force.
    ///
    /// Returns:
    ///     The current arming generation.
    pub(crate) fn shot_caption_gen(&self) -> u64 {
        self.state.borrow().shot_caption_gen()
    }

    /// Toggles crosshair cursor readout labels. Returns true on change.
    pub fn set_cursor_labels(&mut self, show: bool) -> bool {
        let mut st = self.state.borrow_mut();
        if st.cursor_labels == show {
            return false;
        }
        st.cursor_labels = show;
        st.needs_present = true;
        true
    }

    /// Sets the mode marker glyph drawn beside the crosshair; `None` clears it.
    /// Returns true on change.
    pub fn set_cursor_badge(&mut self, badge: Option<&'static str>) -> bool {
        let mut st = self.state.borrow_mut();
        if st.cursor_badge == badge {
            return false;
        }
        st.cursor_badge = badge;
        st.needs_present = true;
        true
    }

    /// Sets the projected manual-order size from s1-s6 in USD for the cursor crosshair label.
    /// Returns true when the change exceeds the anti-jitter threshold. None means no size or rate.
    pub fn set_prospective_usd(&mut self, usd: Option<f64>) -> bool {
        let mut data = self.data.borrow_mut();
        let changed = match (data.prospective_usd, usd) {
            (Some(a), Some(b)) => (a - b).abs() > a.abs().max(1.0) * 1e-3,
            (None, None) => false,
            _ => true,
        };
        if changed {
            data.prospective_usd = usd;
            data.mark_view_dirty();
        }
        changed
    }

    /// Marks this engine a historical viewer, whose subject is a closed interval and not `now`.
    ///
    /// Args:
    ///     historical: Whether the engine draws a finished interval rather than the live edge.
    pub fn set_historical(&mut self, historical: bool) {
        // Stored in the DATA STATE, where the caption gates read it, rather than beside it here: an
        // engine is `Clone` over these shared handles, so a flag on the engine itself would be
        // copied per clone and could disagree with the state every gate answers from.
        self.data.borrow_mut().set_historical(historical);
        if historical {
            // The pane is constructed live (`ChartView::new` follows `now`). Refusing the global
            // Live flag is not enough: until the trade is framed, `follow_edge` still walks the
            // default-true view to the live edge and the window opens hours away from the trade.
            self.follow = false;
            self.data.borrow_mut().follow = false;
            for pane in self.container.borrow_mut().panes_mut() {
                pane.view.set_manual_persistent();
            }
        }
    }

    /// Applies the toolbar's global Live/Pause follow state to this `ChartEngine`'s single pane.
    /// Only an explicit change to the global flag has an effect. Per-pane pan and rejoin state lives
    /// in `view.follow`; `sync_follow_from_views` supplies the already consolidated value here.
    ///
    /// A HISTORICAL engine refuses the global flag outright. `ChartPanel::render` applies
    /// `backend.follow` to every panel it draws, and that flag defaults to on, so without this
    /// guard a trade window's requested interval would be undone by re-anchoring it to `now`.
    /// Resuming a live engine preserves its chosen time scale: Live changes the anchor, not zoom.
    pub fn set_follow(&mut self, follow: bool, now_ms: f64) -> bool {
        if self.data.borrow().historical || self.follow == follow {
            return false;
        }
        self.follow = follow;
        self.data.borrow_mut().follow = follow;
        for p in self.container.borrow_mut().panes_mut() {
            if follow {
                // Explicit toolbar Live resumes only panes that were not following. Leave already
                // live panes untouched so their window and zoom are not reset.
                if !p.view.follow {
                    p.view.resume_live(now_ms);
                }
            } else {
                // An explicit Live-button disable does not automatically rejoin on a timer.
                p.view.set_manual_persistent();
            }
        }
        self.data.borrow_mut().mark_view_dirty();
        true
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Moonbot's "Center chart" on every pane: the manual Y view is dropped, the last price snaps
    /// to the centre, and the live edge is resumed — out of an explicit Pause as well.
    ///
    /// A HISTORICAL engine refuses it for the same reason it refuses `set_follow`: its subject is
    /// a closed interval, and re-anchoring to `now` would take the trade off screen.
    ///
    /// Args:
    ///     now_ms: Current Unix time in milliseconds.
    ///
    /// Returns:
    ///     Whether the engine acted; `false` for a historical viewer.
    pub fn center_on_price(&mut self, now_ms: f64) -> bool {
        if self.data.borrow().historical {
            return false;
        }
        for p in self.container.borrow_mut().panes_mut() {
            p.view.center_on_price(now_ms);
        }
        self.follow = true;
        let mut data = self.data.borrow_mut();
        data.follow = true;
        data.mark_view_dirty();
        true
    }

    /// Request framing of the first pane on an interval and persist manual X navigation.
    ///
    /// The interval is REQUESTED rather than applied: the plot width is knowable only inside a
    /// prepared frame, and this method is reached from application code that commonly runs before
    /// the first present. It used to measure the width itself, through `pane_rects` and the shared
    /// pane layout, both of which floor an unpresented slot at ONE PIXEL rather than
    /// reporting that they do not know - so the interval was framed for a one-pixel plot and the
    /// visible span at the real width came out wider by the ratio of the two, drawing correct axes
    /// over an empty plot. The view now holds the request until a real width exists, and re-applies
    /// it on resize.
    ///
    /// Args:
    ///     start_ms: Absolute Unix-millisecond entry timestamp.
    ///     end_ms: Absolute Unix-millisecond close timestamp.
    ///     padding_fraction: Fraction of the window reserved on each side. A caller that already
    ///         built its own context into the interval passes zero; one handing over a bare
    ///         interval asks for breathing room here.
    ///
    /// Returns:
    ///     Whether a valid interval was accepted.
    pub(crate) fn show_time_range(
        &mut self,
        start_ms: f64,
        end_ms: f64,
        padding_fraction: f32,
    ) -> bool {
        let changed = self
            .container
            .borrow_mut()
            .view_mut(0)
            .is_some_and(|view| view.request_time_range(start_ms, end_ms, padding_fraction));
        if changed {
            self.follow = false;
            let mut data = self.data.borrow_mut();
            data.follow = false;
            data.mark_view_dirty();
        }
        changed
    }

    pub fn sync_follow_from_views(&mut self) -> bool {
        let container = self.container.borrow();
        let follow = if container.is_empty() {
            self.follow
        } else {
            container.panes().iter().all(|p| p.view.follow)
        };
        drop(container);
        if self.follow == follow {
            false
        } else {
            self.follow = follow;
            self.data.borrow_mut().follow = follow;
            true
        }
    }
}
