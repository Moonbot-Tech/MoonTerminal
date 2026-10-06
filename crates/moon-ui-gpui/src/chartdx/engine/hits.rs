//! Caption hit targets, market buttons and caption configuration.

use super::*;

impl ChartEngine {
    /// The arbitrage venue whose NAME is under a point, in the pane's own logical pixels.
    ///
    /// Read off what the last frame DREW — the caption pass records each name's rectangle as it
    /// places it — because the column moves with the pane and with the roster, and a rectangle
    /// recomputed at click time would have to repeat the whole placement to be right.
    ///
    /// Args:
    ///     pane: Pane index the point belongs to.
    ///     x, y: Point in the pane's own logical pixels.
    ///
    /// Returns:
    ///     The venue's platform code and its DEX name (empty for an ordinary exchange).
    pub fn arb_venue_at(&self, pane: usize, x: f32, y: f32) -> Option<(u8, String)> {
        let data = self.data.borrow();
        let render = data.render.borrow();
        let hit = render
            .panes
            .get(pane)?
            .arb_hits
            .iter()
            .find(|hit| hit.contains(x, y))?;
        Some((hit.code, hit.dex.clone()))
    }

    /// Which VOLUME module a point lands on, if any.
    ///
    /// The same measurement the caption pass drew from, in the pane's own logical pixels: a
    /// right-click has to hit what the last frame actually put on screen.
    ///
    /// Args:
    ///     pane: Pane index the point was resolved to.
    ///     x: Point in the pane's logical pixels.
    ///     y: The same, vertically.
    ///
    /// Returns:
    ///     Index of the caption module, for the menu that edits its period.
    pub fn volume_module_at(&self, pane: usize, x: f32, y: f32) -> Option<usize> {
        let data = self.data.borrow();
        let render = data.render.borrow();
        let hit = render
            .panes
            .get(pane)?
            .volume_hits
            .iter()
            .find(|hit| hit.contains(x, y))?;
        Some(hit.row)
    }

    /// WHICH buttons this chart's captions place, and so which facts the panel has to resolve.
    ///
    /// The gate for the whole push below, per action rather than as one flag: every fact behind a
    /// button costs a lookup per pane per render — the workspace rail, a walk of the core's open
    /// orders, a walk of its blacklist — and a chart carrying only `Cancel Buy` must pay for none
    /// of the other two.
    pub fn wanted_market_actions(&self) -> WantedActions {
        let data = self.data.borrow();
        if !data.draws_live_market() {
            return WantedActions::default();
        }
        use moon_core::config::ChartAction as A;
        let mut wanted = WantedActions::default();
        // ONE walk for the three answers: three `any_drawn` calls would each cross sixteen rows of
        // captions, on a path whose whole purpose is to keep work off the render.
        for part in data.render.borrow().chart_labels.drawn_parts() {
            match part.field.action() {
                Some(A::CancelBuy) => wanted.cancel_buy = true,
                Some(A::PanicSell) => wanted.panic_sell = true,
                Some(A::TempBan) => wanted.temp_ban = true,
                Some(A::Favorite) => wanted.favorite = true,
                // The ban READOUT is not a button, and it needs the same fact the lock does: a
                // chart printing only the remaining time must still be told what is left.
                None => {
                    wanted.temp_ban |=
                        part.field == moon_core::config::ChartLabelField::TempBanLeft;
                }
            }
        }
        wanted
    }

    /// Hand one pane the state its market buttons print, and report whether anything moved.
    ///
    /// PUSHED rather than read here because none of it is the engine's to answer: whether panic is
    /// armed mixes the core's snapshot with the terminal's own optimistic override, whether a
    /// command is allowed is a property of the window's workspace rail, and the temporary ban lives
    /// in the session store the engine does not hold.
    ///
    /// Whether the chart is LIVE at all is deliberately not part of what the panel states: the
    /// engine answers that itself, so a trade-detail window — which draws a trade that already
    /// closed — cannot be handed a pressable button by a caller that forgot to ask.
    ///
    /// Args:
    ///     pane: Pane index, as `pane_target` reports them.
    ///     state: What the panel resolved for that pane's market, or `None` for a pane that draws
    ///         no buttons at all.
    ///
    /// Returns:
    ///     Whether the pane's captions were re-formatted — which is also when the present is
    ///     raised, so a caller that only pushes state has nothing to do with the answer.
    pub(crate) fn set_pane_actions(
        &mut self,
        pane: usize,
        state: Option<MarketActionState>,
    ) -> bool {
        let data = self.data.borrow();
        let live = data.draws_live_market();
        let mut st = data.render.borrow_mut();
        let Some(pr) = st.panes.get_mut(pane) else {
            return false;
        };
        let actions = match state {
            // `None` is not "a button that does nothing": it is a pane with no buttons on it at
            // all, and every action caption on it prints NOTHING. That is what a book-only pane and
            // a chart whose last button was deleted both need — a faded control left on screen
            // would still take the press meant for what is under it.
            None => crate::chartdx::text::ActionInputs::default(),
            Some(state) => crate::chartdx::text::ActionInputs {
                live,
                allowed: state.allowed,
                panic_armed: state.panic_armed,
                ban_until_ms: state.ban_until_ms,
                favorite: state.favorite,
            },
        };
        if pr.label_actions == actions {
            return false;
        }
        pr.label_actions = actions;
        let changed = st.refresh_pane_labels(pane);
        if changed {
            st.needs_present = true;
        }
        changed
    }

    /// Scroll one label column of a pane by `steps` lines; positive shows the next rows.
    ///
    /// The offset is clamped to the column's current length, so a notch past either end changes
    /// nothing. Returns whether the drawn captions changed: `false` asks for no repaint.
    pub(crate) fn scroll_label_column(&mut self, pane: usize, row: usize, steps: i32) -> bool {
        let data = self.data.borrow();
        let mut st = data.render.borrow_mut();
        let Some(pr) = st.panes.get_mut(pane) else {
            return false;
        };
        let Some(total) = pr.labels.column_len(row) else {
            return false;
        };
        let cur = crate::chartdx::text::first_of(&pr.label_scroll, row);
        let cur_c = crate::chartdx::text::clamp_first(i64::from(cur), total);
        let first = crate::chartdx::text::clamp_first(i64::from(cur_c) + i64::from(steps), total);
        if first == cur_c && cur_c == cur {
            return false;
        }
        pr.label_scroll.retain(|(r, _)| *r != row);
        if first > 0 {
            pr.label_scroll.push((row, first));
        }
        let changed = st.refresh_pane_labels(pane);
        if changed {
            st.needs_present = true;
        }
        changed
    }

    /// The buttons one pane laid out: where each goes, what it does, and the label it was
    /// measured at.
    ///
    /// Read by the PANEL on its render, which places the application's own control at each
    /// rectangle. The label comes from the caption pass rather than being rebuilt here, so the
    /// string the layout reserved room for is the string the button shows.
    ///
    /// The market rides along because the rectangles are a frame old: a pane retargeted since would
    /// otherwise send the command to a coin nobody aimed at.
    ///
    /// Args:
    ///     pane: Pane index, as `pane_target` reports them.
    ///
    /// Returns:
    ///     One entry per button, in configured order; empty for a pane drawing none.
    pub(crate) fn action_buttons(&self, pane: usize) -> Vec<ChartActionButton> {
        let data = self.data.borrow();
        let render = data.render.borrow();
        let Some(pr) = render.panes.get(pane).filter(|pane| pane.active) else {
            return Vec::new();
        };
        let Some(core) = pr.core else {
            return Vec::new();
        };
        let state = pr.label_actions;
        pr.action_rects
            .iter()
            .filter_map(|placement| {
                // By identity, not by position: the resolved list is compacted on every revision.
                let label = pr
                    .labels
                    .texts
                    .iter()
                    .find(|text| text.row == placement.row && text.part == placement.part)?;
                // What this control SAYS and whether it may be pressed, decided together: the rail
                // decides the second for every button, and the star additionally needs the core to
                // have said what it holds, because its press rewrites that answer.
                let (on, ready) = match placement.mark.action {
                    moon_core::config::ChartAction::CancelBuy => (false, true),
                    moon_core::config::ChartAction::PanicSell => (state.panic_armed, true),
                    moon_core::config::ChartAction::TempBan => (state.ban_until_ms.is_some(), true),
                    moon_core::config::ChartAction::Favorite => {
                        (state.favorite.unwrap_or(false), state.favorite.is_some())
                    }
                };
                Some(ChartActionButton {
                    x: placement.x,
                    y: placement.y,
                    w: placement.w,
                    h: placement.h,
                    action: placement.mark.action,
                    // Read HERE, from the values the label beside it was formatted from, rather
                    // than copied into the rectangle a frame ago: a control that took its state
                    // from one generation and its words from another would draw `Stop Panic`
                    // unpressed.
                    // Both answers from ONE match on the action; see `on`/`ready` above.
                    active: on,
                    enabled: state.allowed && ready,
                    label: label.text.clone(),
                    size: placement.size,
                    row: placement.row,
                    part: placement.part,
                    core,
                    market: pr.market.clone(),
                    coin: pr.coin.clone(),
                })
            })
            .collect()
    }

    /// Rectangles of the arbitrage venue names a pane drew, in the pane's own logical pixels.
    ///
    /// For the cursor overlay: a native cursor can only be set during PAINT, so the panel lays
    /// transparent zones over these and lets the library ask for the pointer. Only reachable venues
    /// are handed out — the others are not targets.
    pub fn arb_hit_rects(&self, pane: usize) -> Vec<(f32, f32, f32, f32)> {
        let data = self.data.borrow();
        let render = data.render.borrow();
        let Some(pane) = render.panes.get(pane) else {
            return Vec::new();
        };
        pane.arb_hits
            .iter()
            .filter(|hit| hit.reachable)
            .map(|hit| (hit.x, hit.y, hit.w, hit.h))
            .collect()
    }

    /// Applies the GLOBAL arbitrage roster — which venues the column lists, in what order, under
    /// what name and colour. Returns true on change.
    ///
    /// Global rather than per tab, so every chart takes the same handle; the panel hands it down on
    /// render exactly like the captions beside it, and the pointer test carries the common case.
    pub fn set_arb_view(&mut self, cfg: std::rc::Rc<moon_core::config::ArbViewCfg>) -> bool {
        let mut data = self.data.borrow_mut();
        if std::rc::Rc::ptr_eq(&data.arb_view, &cfg) {
            return false;
        }
        let unchanged = *data.arb_view == *cfg;
        data.arb_view = cfg.clone();
        data.render.borrow_mut().arb_view = cfg;
        if unchanged {
            return false;
        }
        // The column is arranged where the captions are RESOLVED, on the sync paths, and those
        // short-circuit on an unchanged signature — so a roster edit reaches a chart whose market
        // has not moved only if the signature is invalidated here.
        data.last_order_sig = u64::MAX;
        data.mark_view_dirty();
        true
    }

    /// Applies the chart caption configuration — which figures print beside the plot, where and in
    /// which style — to every engine pane. Returns true on change.
    ///
    /// The value is the owning panel's: its own per-tab override, or the `layout.chart_labels`
    /// default when it has none, already sanitized by the caller.
    ///
    /// Unlike the graphics settings beside it, nothing baked into a GPU layer depends on this: the
    /// captions are drawn by the text pass from resolved strings, so a change only has to make the
    /// panes re-resolve and repaint. `mark_view_dirty` does both.
    pub fn set_chart_labels(
        &mut self,
        cfg: std::rc::Rc<moon_core::config::ChartLabelsCfg>,
    ) -> bool {
        let mut data = self.data.borrow_mut();
        // Pointer first: the panel hands the SAME allocation on every render, so the common case
        // answers without walking sixteen rows of captions.
        if std::rc::Rc::ptr_eq(&data.chart_labels, &cfg) {
            return false;
        }
        // A DIFFERENT allocation holding the same configuration: the panel rebuilds its settings
        // signature on every backend notification. Adopt it anyway — keeping the old handle would
        // fail the pointer test above on every render from here on, and pay the structural compare
        // each time.
        let unchanged = *data.chart_labels == *cfg;
        data.chart_labels = cfg.clone();
        // Mirrored into the text pass, which reads it every frame and must not borrow the data
        // state to do so. The same allocation, not a second copy of it — and done before the early
        // return, because an unchanged configuration still arrives as a NEW allocation on every
        // render and the mirror has to adopt it too.
        data.render.borrow_mut().chart_labels = cfg;
        if unchanged {
            return false;
        }
        // The captions are re-resolved by the SYNC paths, and those short-circuit on an unchanged
        // signature. Invalidating it here is what makes a configuration change reach a chart whose
        // market and orders have not moved — the same reason `set_chart_graphics` resets it.
        data.last_order_sig = u64::MAX;
        data.mark_view_dirty();
        true
    }
}
