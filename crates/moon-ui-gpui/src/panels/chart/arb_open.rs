//! Clicking an arbitrage venue's name: open THIS coin on THAT exchange.
//!
//! The column already answers "what does this coin cost elsewhere"; the obvious next question is
//! "show me". A left click opens the coin on the other exchange, a right click opens it in a
//! comparison tab, and when several cores are connected to that exchange the choice is a picker —
//! the same shape the News panel uses for the same problem.
//!
//! How a venue is matched to a core: an arbitrage platform code IS the core's platform ordinal for
//! an ordinary exchange (`ArbPlatformCode::from_exchange` is a byte copy), so the two compare
//! directly. A Hyperliquid deployer is the exception — every deployer shares the futures ordinal —
//! and is matched by its DEX name instead, which is the same name `known_dexes` gave the column.

use gpui::*;
use moon_core::session::CoreId;
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _};

use super::ChartPanel;
use crate::controls::coin_search;

impl ChartPanel {
    /// Where the chart slot sits in the window, in LOGICAL pixels, and the scale it draws at.
    ///
    /// THE one place the two coordinate systems meet: the caption pass works in window logical
    /// pixels, the panel's own input and layout in slot ones. Both the press and the cursor zones
    /// go through this, so they cannot drift apart again.
    fn chart_origin_logical(&self) -> Option<((f32, f32), f32)> {
        let (bounds, sf, _) = self.chart.slot_geometry()?;
        let sf = sf.max(0.1);
        Some(((f32::from(bounds.origin.x), f32::from(bounds.origin.y)), sf))
    }

    /// Transparent zones over the venue names, so the pointer changes over a clickable one.
    ///
    /// The only way to ask for a native cursor is during PAINT — `set_cursor_style` asserts it —
    /// so a hover handler cannot do this and a styled element must. The zones carry no handlers:
    /// the click itself is routed by the chart's own input, in the pane's coordinates, where every
    /// other chart gesture is decided.
    pub(super) fn arb_cursor_zones(&self) -> Vec<Div> {
        let mut out = Vec::new();
        let Some((origin, _)) = self.chart_origin_logical() else {
            return out;
        };
        for (pane, _) in self.chart.pane_rects() {
            for (x, y, w, h) in self.chart.arb_hit_rects(pane) {
                if w <= 0.0 || h <= 0.0 {
                    continue;
                }
                // The rectangles are in the WINDOW's logical pixels and this overlay is laid out
                // inside the chart slot, so the slot's own position comes off — the exact inverse
                // of what the press does above, and the reason both are computed from one helper.
                out.push(
                    div()
                        .absolute()
                        .left(px(x - origin.0))
                        .top(px(y - origin.1))
                        .w(px(w))
                        .h(px(h))
                        .cursor(CursorStyle::PointingHand),
                );
            }
        }
        out
    }
}

/// What a click on a venue name does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ArbOpen {
    /// Open the coin on that exchange, as a coin click anywhere else does.
    Chart,
    /// Open it beside the current one, in a comparison tab.
    Compare,
}

impl ChartPanel {
    /// Handle a click on an arbitrage venue name, if the point is on one.
    ///
    /// Args:
    ///     pos: Point in the chart's own logical pixels.
    ///     screen: Same point in window coordinates, to anchor a picker on.
    ///     mode: What the pressed button asked for.
    ///
    /// Returns:
    ///     Whether the click was consumed. A name with no core behind it consumes the click too:
    ///     the user hit the label, and falling through to place an order there would be a surprise.
    pub(super) fn try_open_arb_venue(
        &mut self,
        pos: (f32, f32),
        screen: Point<Pixels>,
        mode: ArbOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.input.pane_at(pos.0, pos.1) else {
            return false;
        };
        // The point arrives in DEVICE pixels of the chart — `chart_local_from_window_pos` scales it
        // — while the caption pass places its rectangles in LOGICAL ones, and both are measured
        // from the same origin: the chart's, not the pane's. So the only conversion is the scale
        // factor. Getting this wrong opened whichever venue happened to sit under the mis-scaled
        // point, which on a 1.5x display is several rows down the column.
        // The caption pass places its names in the WINDOW's logical pixels — `pane_bounds` is built
        // as `slot_origin + rect`, and the cursor readout beside it adds the same origin — while
        // this point is in the SLOT's device pixels. So the conversion is both: scale down, then
        // add where the slot sits in the window. Missing the origin put every hit off by exactly
        // the panel's position, which on a chart under a header and a toolbar is a long way down.
        let Some((origin, sf)) = self.chart_origin_logical() else {
            return false;
        };
        let (lx, ly) = (pos.0 / sf + origin.0, pos.1 / sf + origin.1);
        // Measured rather than assumed, behind `log.chart_input`: the chart draws in its own pass,
        // and a hit test that disagrees with the drawing cannot be seen by reading either.
        log::debug!(
            "arb hit: press ({:.1},{:.1}) -> logical ({lx:.1},{ly:.1}) ppp {sf:.2} window sf {:.2} · rects {:?}",
            pos.0,
            pos.1,
            window.scale_factor(),
            self.chart.arb_hit_rects(pane),
        );
        let Some((code, dex)) = self.chart.arb_venue_at(pane, lx, ly) else {
            return false;
        };
        let Some((core, market)) = self
            .chart
            .with_container(|container| container.target(pane))
        else {
            return true;
        };
        let rows = self.cores_trading_here(core, &market, code, &dex, cx);
        // The chart the click came FROM, so a comparison can hold both sides of it.
        let anchor = (core, market);
        match rows.len() {
            0 => {}
            1 => {
                let (target_core, target_market) = rows.into_iter().next().expect("one row");
                self.open_arb_target(anchor, target_core, target_market, mode, cx);
            }
            _ => self.pick_arb_core(anchor, rows, screen, mode, window, cx),
        }
        true
    }

    /// Every core on `code`'s exchange that trades this coin, excluding the one already charted.
    ///
    /// The coin is matched through the shared search — the same enumeration the News panel and the
    /// coin picker use — so a Hyperliquid spot index or a contract tail resolves the way it does
    /// everywhere else, rather than by a reading of the market's name here.
    ///
    /// WHICH of a core's markets is opened is the shared identity rule, not the first hit: a Bybit
    /// core lists BTC under ten expiries beside the perpetual, and a click that opened one of those
    /// would show a chart whose price differs from the row that was clicked. One market per core —
    /// the perpetual, in the quote currency the click came from.
    fn cores_trading_here(
        &self,
        current: CoreId,
        market: &str,
        code: u8,
        dex: &str,
        cx: &Context<Self>,
    ) -> Vec<(CoreId, String)> {
        let b = self.backend.read(cx);
        let label = b.session.market_source().market_label(current, market);
        let venues = b.session.core_venues();
        // Searched by the IDENTITY the core resolved, not by this chart's spelling of the coin: a
        // catalog is matched against the literal query, and `1kBONK` occurs in no field of the
        // same coin's market on Binance. `canonic` is a field every catalog carries and its own
        // search ranks among the first.
        let wanted = label.identity();
        if wanted.is_empty() {
            return Vec::new();
        }
        // Every hit this core offers for the coin, so the identity rule picks among them rather
        // than taking whichever the search ranked first.
        let mut per_core: Vec<(CoreId, Vec<(String, moon_core::market::MarketLabel)>)> = Vec::new();
        for hit in coin_search::search_limited(b, "", None, &wanted, coin_search::COIN_MATCH_LIMIT)
        {
            if hit.core == current {
                continue;
            }
            let Some(venue) = venues.get(&hit.core) else {
                continue;
            };
            if !venue.matches_arb(code, dex) {
                continue;
            }
            match per_core.iter_mut().find(|(core, _)| *core == hit.core) {
                Some((_, hits)) => hits.push((hit.market, hit.label)),
                None => per_core.push((hit.core, vec![(hit.market, hit.label)])),
            }
        }
        per_core
            .into_iter()
            .filter_map(|(core, hits)| {
                let market =
                    moon_core::market::pick_market_for_identity(&hits, &wanted, &label.quote)?;
                Some((core, market.to_string()))
            })
            .collect()
    }

    /// Open one target the way the pressed button asked.
    fn open_arb_target(
        &mut self,
        anchor: (CoreId, String),
        core: CoreId,
        market: String,
        mode: ArbOpen,
        cx: &mut Context<Self>,
    ) {
        let group = self.workspace_group.clone();
        self.backend.update(cx, |b, bcx| {
            let done = match mode {
                ArbOpen::Chart => {
                    // `false`: opened without stealing focus, like every other coin-navigation site.
                    b.open_on_main_if_authorized(group.as_deref(), (core, market), false)
                }
                // BOTH sides: a comparison of one chart is not a comparison, and the chart the
                // click came from is half the question. Clicking a second venue from inside that
                // tab adds to it rather than opening another — see `open_compare_with`.
                ArbOpen::Compare => {
                    b.open_compare_pair_if_authorized(group.as_deref(), anchor, (core, market))
                }
            };
            if done {
                bcx.notify();
            }
        });
    }

    /// Ask which core, when the exchange has more than one connected.
    fn pick_arb_core(
        &mut self,
        anchor: (CoreId, String),
        rows: Vec<(CoreId, String)>,
        screen: Point<Pixels>,
        mode: ArbOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let names: Vec<(CoreId, String, String)> = {
            let b = self.backend.read(cx);
            rows.into_iter()
                .map(|(core, market)| {
                    let name = b
                        .session
                        .sessions()
                        .iter()
                        .find(|s| s.id == core)
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    (core, market, name)
                })
                .collect()
        };
        let view = cx.entity().downgrade();
        let items: Vec<MoonMenuItem> = names
            .into_iter()
            .map(|(core, market, name)| {
                let view = view.clone();
                let anchor = anchor.clone();
                MoonMenuItem::with_key(format!("arb-core-{core}"), name).on_click(
                    move |_, window: &mut Window, app: &mut App| {
                        window.close_context_menu(app);
                        let market = market.clone();
                        let anchor = anchor.clone();
                        view.update(app, |this, cx| {
                            this.open_arb_target(anchor, core, market, mode, cx);
                        })
                        .ok();
                    },
                )
            })
            .collect();
        // The library's own fitted context menu, the same one the coin menu on this chart opens.
        window.open_fitted_moon_context_menu(cx, "arb-core-menu", screen, items, 140.0, 320.0);
    }
}
