//! Main chart stack tabs operations.

use super::*;

impl MainChartStack {
    /// Renders one Main-chart tile in either stacked or fullscreen presentation.
    ///
    /// Fullscreen deliberately keeps the card header so the active market remains identifiable;
    /// only the inter-tile gutter is removed, and a position note appears when siblings exist.
    ///
    /// Args:
    ///     ix: Index of the chart entry being rendered.
    ///     panel: Chart panel placed inside the card.
    ///     size: Optional extent along the stack axis.
    ///     flex: Whether the tile may flex in Compress or Fit layout.
    ///     min_w: Minimum extent along the stack axis while flexing.
    ///     horizontal: Whether the stack axis is horizontal.
    ///     border: Border color for the card.
    ///     entity: Stack entity used by the fullscreen toggle callback.
    ///     palette: Active MoonUI palette.
    ///     fullscreen: Whether this is the single full-bleed active chart.
    ///     title_size: Resolved title font size.
    ///     tokens: Active theme tokens for chart-card geometry.
    ///
    /// Returns:
    ///     The sized chart card with its fullscreen toggle handler attached.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_tile(
        &self,
        ix: usize,
        panel: Entity<ChartPanel>,
        size: Option<f32>,
        flex: bool,
        min_w: Option<f32>,
        horizontal: bool,
        border: Rgba,
        entity: Entity<Self>,
        palette: MoonPalette,
        fullscreen: bool,
        title_size: Pixels,
        tokens: &moon_ui::MoonThemeTokens,
    ) -> Stateful<Div> {
        let panel_for_event = panel.clone();
        let label = self
            .charts
            .get(ix)
            .map(|e| e.market.clone())
            .unwrap_or_else(|| "Chart".to_string());
        // The card — and therefore the market name — is drawn in fullscreen too. Without it the
        // fullscreen view is the stacked view MINUS its only label, so with a single chart the
        // gesture reads as "the coin name disappeared" rather than as a mode change; with several,
        // nothing on screen says which one is showing. Fullscreen drops only the inter-tile gutter,
        // which has nothing left to separate, and gains a position note when there are siblings.
        let trailing = (fullscreen && self.charts.len() > 1)
            .then(|| SharedString::from(format!("{}/{}", ix + 1, self.charts.len())));
        // Tiled, the tab row above says which chart is selected; the card repeats it with an accent
        // frame so the answer is also where the user's eyes already are. Fullscreen shows only the
        // selected chart, so there is nothing to distinguish it from.
        let border = if !fullscreen && Some(ix) == self.active && self.charts.len() > 1 {
            rgb(palette.accent)
        } else {
            border
        };
        let mut tile = chart_stack_card(
            SharedString::from(format!("main-chart-stack-tile-{ix}")),
            label,
            panel,
            palette,
            border,
            title_size,
            tokens,
            tile_gutter(fullscreen, self.charts.len()),
            trailing,
        )
        .on_mouse_up(
            MouseButton::Right,
            move |event: &MouseUpEvent, _window, app| {
                // A short right-click in the panel area exits fullscreen. In the control zone
                // (order book or reserved strip), right-click remains trading-only, while a
                // right-button price drag remains zoom and does not toggle the stack.
                //
                // Only a CLEAN right-click: with a modifier held the press belongs to a trading
                // gesture, and Moonbot's own fullscreen toggle is the unmodified click too. A
                // gesture the chart recognised already suppresses this release, but an unbound
                // Ctrl+right-click reached here and collapsed the stack under the user's hand.
                let panel = panel_for_event.read(app);
                if !event.modifiers.modified()
                    && panel.window_pos_allows_main_stack_toggle(event.position)
                    && !panel.window_pos_in_control_zone(event.position)
                    && !panel.rmb_was_moved()
                {
                    entity.update(app, |this, cx| this.toggle_from_chart(ix, cx));
                    app.stop_propagation();
                }
            },
        );
        // Fill the cross axis. Along the stack axis use flex with a cap for COMPRESS, a fixed size,
        // or stretching for FIT. Horizontal stacks use X as their axis; vertical stacks use Y.
        tile = if horizontal {
            tile.h_full()
        } else {
            tile.w_full()
        };
        if flex {
            tile = tile.flex_1();
            let m = min_w.unwrap_or(0.0);
            tile = if horizontal {
                tile.min_w(px(m))
            } else {
                tile.min_h(px(m))
            };
            if let Some(v) = size {
                tile = if horizontal {
                    tile.max_w(px(v))
                } else {
                    tile.max_h(px(v))
                };
            }
        } else if let Some(v) = size {
            // Fixed without shrinking (`min == max == v`), so SCROLL tiles overflow and can scroll.
            tile = if horizontal {
                tile.w(px(v)).min_w(px(v))
            } else {
                tile.h(px(v)).min_h(px(v))
            };
        }
        tile
    }
}

impl MainChartStack {
    /// Render the row of per-chart tabs above the stack, or `None` when it would say nothing.
    ///
    /// It exists because the Main tab is a stack of markets under one label: with several charts
    /// open, nothing on screen said which of them was selected — and the selected one is where
    /// trading hotkeys land. Below two charts the row is suppressed: a single tab names a market
    /// the card header already names, at the cost of a strip of chart height.
    ///
    /// The label is the panel's OWN resolved ticker, so the tab and the chart's corner caption
    /// cannot spell the instrument differently; a chart whose catalog has not answered yet falls
    /// back to the raw market key rather than rendering blank.
    ///
    /// Args:
    ///     window: Used only to render the strip through its own palette and scale tokens; the
    ///         strip is in-flow and still sizes itself.
    ///     cx: Stack context, used to read each panel and to build the click handlers.
    ///
    /// Returns:
    ///     The row, or `None` when fewer than two charts are open.
    pub(super) fn render_tab_row(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Vacated slots are retained placeholders in COMPRESS layout — they hold a position, not a
        // chart, so they get no tab.
        let live: Vec<(CoreId, String, SharedString)> = self
            .charts
            .iter()
            .filter(|entry| !entry.vacated)
            .map(|entry| {
                let label = entry
                    .panel
                    .read(cx)
                    .pane_ticker()
                    .unwrap_or_else(|| entry.market.clone());
                (entry.core, entry.market.clone(), SharedString::from(label))
            })
            .collect();
        if live.len() < 2 {
            return None;
        }
        let active_key = self
            .active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.as_str()));
        let strip_h = crate::chart_tabs::chart_tab_strip_h(cx);
        let gap = 4.0_f32;
        let pad_l = 8.0_f32;
        let items: Vec<MoonTabItem> = live
            .iter()
            .map(|(core, market, label)| {
                MoonTabItem::new(label.clone())
                    .selected(active_key == Some((*core, market.as_str())))
                    .closable(true)
            })
            .collect();
        let keys: Rc<Vec<(CoreId, String)>> = Rc::new(
            live.into_iter()
                .map(|(core, market, _)| (core, market))
                .collect(),
        );
        let view = cx.entity();
        let strip = MoonTabStrip::new("main-chart-tab-row-strip")
            .padding_left(pad_l)
            .gap(gap)
            .overflow_menu(true)
            .items(items)
            .on_click({
                let keys = keys.clone();
                let view = view.clone();
                move |ix, event, _window, app| {
                    let Some(key) = keys.get(ix).cloned() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        if event.click_count() >= 2 {
                            this.fullscreen_market(&key, cx);
                        } else {
                            this.focus_market(&key, cx);
                        }
                    });
                }
            })
            .on_close({
                let keys = keys.clone();
                move |ix, _event, _window, app| {
                    let Some(key) = keys.get(ix).cloned() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        if let Some(at) = this.index_of(&key) {
                            this.close_at(at, cx);
                        }
                    });
                }
            });
        // Same treatment as the Main/Add strip above it: MoonUI keys an inactive tab label off
        // `text_muted` and offers no per-tab colour prop, so the lift arrives as a palette.
        // `render_with_theme`, never `render_with_palette` — the latter substitutes default tokens
        // and would drop the user's font delta, shrinking these labels away from `strip_h`.
        let strip_palette = moon_ui::MoonPalette::active(cx);
        let strip = crate::design::chrome_tab_strip(strip, strip_palette, window, cx);
        Some(
            div()
                .id("main-chart-tab-row")
                .flex_none()
                .w_full()
                .h(px(strip_h))
                .child(strip)
                .into_any_element(),
        )
    }
}
