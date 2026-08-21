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
    /// Transparent zones over the venue names, so the pointer changes over a clickable one.
    ///
    /// The only way to ask for a native cursor is during PAINT — `set_cursor_style` asserts it —
    /// so a hover handler cannot do this and a styled element must. The zones carry no handlers:
    /// the click itself is routed by the chart's own input, in the pane's coordinates, where every
    /// other chart gesture is decided.
    pub(super) fn arb_cursor_zones(&self) -> Vec<Div> {
        let mut out = Vec::new();
        for (pane, rect) in self.chart.pane_rects() {
            // The engine reports rectangles in the pane's own logical pixels; the overlay is
            // positioned in the chart's, and `pane_rects` is in DEVICE pixels of the chart.
            let sf = self.last_ppp.max(0.1);
            for (x, y, w, h) in self.chart.arb_hit_rects(pane) {
                if w <= 0.0 || h <= 0.0 {
                    continue;
                }
                out.push(
                    div()
                        .absolute()
                        .left(px(x + rect.x / sf))
                        .top(px(y + rect.y / sf))
                        .w(px(w))
                        .h(px(h))
                        .cursor(CursorStyle::PointingHand),
                );
            }
        }
        out
    }
}

/// Whether a core is connected to the venue an arbitrage line names.
///
/// An arbitrage platform code IS the core's platform ordinal for an ordinary exchange — the
/// protocol builds one from the other by copying the byte — so those compare directly. A
/// Hyperliquid deployer is the exception: every deployer shares the futures ordinal, and only the
/// DEX name tells them apart. Which is also why an ordinary exchange must NOT match a core that has
/// a DEX: `xyz` and plain Hyperliquid futures would otherwise be the same venue.
fn venue_matches(venue: &moon_core::venue::CoreVenue, code: u8, dex: &str) -> bool {
    match dex.is_empty() {
        true => venue.id.code == code && venue.dex.is_empty(),
        false => venue.dex == dex,
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
        let sf = window.scale_factor().max(0.1);
        let Some((code, dex)) = self.chart.arb_venue_at(pane, pos.0 / sf, pos.1 / sf) else {
            return false;
        };
        let Some((core, market)) = self
            .chart
            .with_container(|container| container.target(pane))
        else {
            return true;
        };
        let rows = self.cores_trading_here(core, &market, code, &dex, cx);
        match rows.len() {
            0 => {}
            1 => {
                let (target_core, target_market) = rows.into_iter().next().expect("one row");
                self.open_arb_target(target_core, target_market, mode, cx);
            }
            _ => self.pick_arb_core(rows, screen, mode, window, cx),
        }
        true
    }

    /// Every core on `code`'s exchange that trades this coin, excluding the one already charted.
    ///
    /// The coin is matched through the shared search — the same enumeration the News panel and the
    /// coin picker use — so a Hyperliquid spot index or a contract tail resolves the way it does
    /// everywhere else, rather than by a reading of the market's name here.
    fn cores_trading_here(
        &self,
        current: CoreId,
        market: &str,
        code: u8,
        dex: &str,
        cx: &Context<Self>,
    ) -> Vec<(CoreId, String)> {
        let b = self.backend.read(cx);
        let coin = b
            .session
            .market_source()
            .market_label(current, market)
            .coin;
        if coin.is_empty() {
            return Vec::new();
        }
        let venues = b.session.core_venues();
        let wanted = moon_core::symbol::coin_match_key(&coin);
        let mut out: Vec<(CoreId, String)> = Vec::new();
        for hit in coin_search::search(b, "", None, &coin) {
            if hit.core == current || hit.label.match_key() != wanted {
                continue;
            }
            let Some(venue) = venues.get(&hit.core) else {
                continue;
            };
            if venue_matches(venue, code, dex)
                && !out.iter().any(|(existing, _)| *existing == hit.core)
            {
                out.push((hit.core, hit.market));
            }
        }
        out
    }

    /// Open one target the way the pressed button asked.
    fn open_arb_target(
        &mut self,
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
                ArbOpen::Compare => {
                    b.open_compare_if_authorized(group.as_deref(), (core, market))
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
                MoonMenuItem::with_key(format!("arb-core-{core}"), name).on_click(
                    move |_, window: &mut Window, app: &mut App| {
                        window.close_context_menu(app);
                        let market = market.clone();
                        view.update(app, |this, cx| {
                            this.open_arb_target(core, market, mode, cx);
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

#[cfg(test)]
mod tests;
