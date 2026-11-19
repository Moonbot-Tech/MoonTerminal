//! Clicking an arbitrage venue's name: open THIS coin on THAT exchange.
//!
//! The column already answers "what does this coin cost elsewhere"; the obvious next question is
//! "show me". A left click opens the coin on the other exchange, a right click opens it in a
//! comparison tab, and when several cores are connected to that exchange the choice is a picker —
//! the same shape the News panel uses for the same problem.
//!
//! Which cores count: every configured core on that exchange, whatever window group it sits in —
//! the column dims a venue, and drops its pointing hand, by whether ANY core has that exchange, so
//! a click scoped to one group would leave a hand over names that do nothing. A core in the clicked
//! window's own group wins when there is one, and the coin opens right there; only when the
//! exchange is reachable from another group alone does the click hand the coin to that group's
//! window and bring it forward (opening the window when it is closed). A window holds only its own
//! group's cores, so opening it "here" is not an option for those.
//!
//! The click is still stricter than the dimming in two ways the column cannot see from its sync:
//! the core must list the coin, and its own window must be allowed to open it (not hidden by the
//! workspace, not outside an Auto selection). Such a venue keeps its hand and the click opens
//! nothing.
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
    /// Where the chart slot sits in the window, in the CHART's logical pixels, the scale it draws
    /// at, and the window's content zoom.
    ///
    /// THE one place the coordinate systems meet: the caption pass works in the chart's logical
    /// pixels (the platform's, so the chart keeps device density under UI zoom), the panel's own
    /// input in slot device pixels, and GPUI overlays in the window's content pixels — the chart's
    /// divided by the zoom. Both the press and the cursor zones go through this, so they cannot
    /// drift apart again.
    pub(super) fn chart_origin_logical(&self) -> Option<((f32, f32), f32, f32)> {
        let (bounds, sf, _) = self.chart.slot_geometry()?;
        let sf = sf.max(0.1);
        let zoom = self.chart.slot_content_zoom().max(0.1);
        Some((
            (
                f32::from(bounds.origin.x) * zoom,
                f32::from(bounds.origin.y) * zoom,
            ),
            sf,
            zoom,
        ))
    }

    /// Transparent zones over the venue names, so the pointer changes over a clickable one.
    ///
    /// The only way to ask for a native cursor is during PAINT — `set_cursor_style` asserts it —
    /// so a hover handler cannot do this and a styled element must. The zones carry no handlers:
    /// the click itself is routed by the chart's own input, in the pane's coordinates, where every
    /// other chart gesture is decided.
    pub(super) fn arb_cursor_zones(&self) -> Vec<Div> {
        self.cursor_zones(|pane| self.chart.arb_hit_rects(pane))
    }

    /// Lay one transparent pointing-hand zone over every rectangle a pane published.
    ///
    /// The shared half of the two overlays — the arbitrage names and the market buttons — because
    /// what differs between them is only which rectangles they ask for. Both work in the CHART's
    /// logical pixels while this overlay is laid out inside the chart SLOT in content pixels, so
    /// the slot's own position comes off and the zoom divides here: the exact inverse of what a
    /// press does, from the same helper, which is what keeps the cursor and the click on the same
    /// rectangle.
    ///
    /// Args:
    ///     rects: What one pane offers, in the chart's logical pixels.
    ///
    /// Returns:
    ///     One absolutely-placed zone per rectangle, carrying no handlers.
    pub(super) fn cursor_zones(
        &self,
        rects: impl Fn(usize) -> Vec<(f32, f32, f32, f32)>,
    ) -> Vec<Div> {
        let mut out = Vec::new();
        let Some((origin, _, zoom)) = self.chart_origin_logical() else {
            return out;
        };
        for pane in 0..self.chart.pane_count() {
            for (x, y, w, h) in rects(pane) {
                if w <= 0.0 || h <= 0.0 {
                    continue;
                }
                out.push(
                    div()
                        .absolute()
                        .left(px((x - origin.0) / zoom))
                        .top(px((y - origin.1) / zoom))
                        .w(px(w / zoom))
                        .h(px(h / zoom))
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
        let Some((origin, sf, _)) = self.chart_origin_logical() else {
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
        let rows = prefer_home_group(
            self.cores_trading_here(core, &market, code, &dex, cx),
            self.workspace_group.as_deref(),
        );
        // The chart the click came FROM, so a comparison can hold both sides of it.
        let anchor = (core, market);
        match rows.len() {
            // A venue whose cores do not list this coin. The click stays consumed: the user hit
            // the label, and an order placed under it would be the surprise.
            0 => log::debug!("arb open: no connected core on venue {code}/{dex} lists the coin"),
            1 => {
                let target = rows.into_iter().next().expect("one row");
                self.open_arb_target(anchor, target, mode, cx);
            }
            _ => self.pick_arb_core(anchor, rows, screen, mode, window, cx),
        }
        true
    }

    /// Every connected core on `code`'s exchange that trades this coin, in any window group,
    /// excluding the one already charted.
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
    ) -> Vec<ArbTarget> {
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
        let scope = search_scope(
            b.session
                .sessions()
                .iter()
                .map(|s| (s.id, s.group.as_str())),
            current,
            // The very gate the open runs, asked of the core's OWN group: a core that window would
            // refuse — hidden by its workspace, or outside an Auto selection — is not a target.
            |group, core| b.workspace_action_allows_core(Some(group), core),
        );
        let mut cores: Vec<CoreId> = scope.iter().map(|(core, _)| *core).collect();
        moon_core::session::core_order::CoreOrder::new(&b.config).sort_by(&mut cores, |id| *id);
        // Every hit this core offers for the coin, so the identity rule picks among them rather
        // than taking whichever the search ranked first.
        let mut per_core: Vec<(CoreId, Vec<(String, moon_core::market::MarketLabel)>)> = Vec::new();
        for hit in coin_search::search_in_cores(b, cores, &wanted, coin_search::COIN_MATCH_LIMIT) {
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
                let group = scope.iter().find(|(id, _)| *id == core)?.1.clone();
                Some(ArbTarget {
                    core,
                    market: market.to_string(),
                    group,
                })
            })
            .collect()
    }

    /// Open one target the way the pressed button asked.
    ///
    /// A target in this window's own group opens here, without stealing focus, like every other
    /// coin-navigation site. One in another group can only open in THAT group's window — a window
    /// charts its own group's cores — so the request is authorised by that group and raises its
    /// window: otherwise the chart would change somewhere behind this one and the click would look
    /// dead. A comparison cannot span two groups, so a right click there opens the chart alone.
    /// A group whose window the user closed gets it reopened: the request would otherwise wait,
    /// unseen, for a window nobody asked for.
    fn open_arb_target(
        &mut self,
        anchor: (CoreId, String),
        target: ArbTarget,
        mode: ArbOpen,
        cx: &mut Context<Self>,
    ) {
        let home = self.workspace_group.clone();
        let action = arb_action(mode, home.as_deref(), &target.group);
        let ArbTarget {
            core,
            market,
            group: target_group,
        } = target;
        let backend = self.backend.clone();
        let reopen = backend.update(cx, |b, bcx| {
            let done = match action {
                ArbAction::OpenAway => {
                    b.open_on_main_if_authorized(Some(&target_group), (core, market), true)
                }
                ArbAction::OpenHere => {
                    b.open_on_main_if_authorized(home.as_deref(), (core, market), false)
                }
                // BOTH sides: a comparison of one chart is not a comparison, and the chart the
                // click came from is half the question. Clicking a second venue from inside that
                // tab adds to it rather than opening another — see `open_compare_with`.
                ArbAction::CompareHere => {
                    b.open_compare_pair_if_authorized(home.as_deref(), anchor, (core, market))
                }
            };
            if done {
                bcx.notify();
            }
            let window_missing = !b.group_windows.contains_key(&target_group)
                && !b.opening_group_windows.contains(&target_group);
            (done && action == ArbAction::OpenAway && window_missing)
                .then(|| (b.config.clone(), b.layout.clone(), b.epoch))
        });
        // Outside the update: building a window reads the backend itself. The new window drains
        // the request queued above when it constructs its tabs.
        if let Some((config, layout, epoch)) = reopen {
            crate::window::group_window::spawn_group_window(
                cx,
                &backend,
                &config,
                target_group,
                epoch,
                &layout,
                0.0,
            );
        }
    }

    /// Ask which core, when the exchange has more than one connected.
    ///
    /// A row from another window group carries that group's name: picking it moves the user to
    /// that window, and the menu should say so before the click does.
    fn pick_arb_core(
        &mut self,
        anchor: (CoreId, String),
        rows: Vec<ArbTarget>,
        screen: Point<Pixels>,
        mode: ArbOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = self.workspace_group.as_deref();
        let named: Vec<(ArbTarget, String)> = {
            let b = self.backend.read(cx);
            rows.into_iter()
                .map(|target| {
                    let name = b
                        .session
                        .sessions()
                        .iter()
                        .find(|s| s.id == target.core)
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    let name = match arb_action(mode, home, &target.group) {
                        ArbAction::OpenAway => format!("{name} · {}", target.group),
                        ArbAction::OpenHere | ArbAction::CompareHere => name,
                    };
                    (target, name)
                })
                .collect()
        };
        let view = cx.entity().downgrade();
        let items: Vec<MoonMenuItem> = named
            .into_iter()
            .map(|(target, name)| {
                let view = view.clone();
                let anchor = anchor.clone();
                MoonMenuItem::with_key(format!("arb-core-{}", target.core), name).on_click(
                    move |_, window: &mut Window, app: &mut App| {
                        window.close_context_menu(app);
                        let target = target.clone();
                        let anchor = anchor.clone();
                        view.update(app, |this, cx| {
                            this.open_arb_target(anchor, target, mode, cx);
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

/// One core a venue click can open: the core, the market chosen on it, and the window group that
/// charts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArbTarget {
    /// The core on the clicked exchange.
    pub(super) core: CoreId,
    /// The market the identity rule picked on that core.
    pub(super) market: String,
    /// The group whose window holds the core — the only window that can open it.
    pub(super) group: String,
}

/// Every core the venue search runs over, with its group: each live core its own group's window
/// would open, the charted core excluded.
///
/// Every group, not the clicked window's: the column dims a venue by whether ANY configured core
/// has that exchange, and a click scoped to one group leaves a pointing hand over names that do
/// nothing. Scoping it to one group by name is how this click stayed dead — no core belongs to the
/// group `""` it once asked for.
///
/// Args:
///     sessions: Every live core as `(id, group)`.
///     current: The core the clicked chart shows.
///     opens: Whether `group`'s window would open `core`; the open's own authority gate.
///
/// Returns:
///     The candidate cores in session order, each with its group.
pub(super) fn search_scope<'a>(
    sessions: impl IntoIterator<Item = (CoreId, &'a str)>,
    current: CoreId,
    opens: impl Fn(&str, CoreId) -> bool,
) -> Vec<(CoreId, String)> {
    sessions
        .into_iter()
        .filter(|(core, group)| *core != current && opens(group, *core))
        .map(|(core, group)| (core, group.to_string()))
        .collect()
}

/// Keep only the clicked window's own group's targets whenever it has any.
///
/// The same exchange can be connected in two groups. Opening it where the user already is beats
/// sending them to another window, so another group is offered only when this one has nothing on
/// that exchange.
///
/// Args:
///     targets: Every core that trades the coin on the clicked exchange.
///     home: The clicked window's group, or `None` for a window that belongs to none.
///
/// Returns:
///     The home group's targets when there are any, otherwise every target unchanged.
pub(super) fn prefer_home_group(targets: Vec<ArbTarget>, home: Option<&str>) -> Vec<ArbTarget> {
    let Some(home) = home else {
        return targets;
    };
    if targets.iter().any(|t| t.group == home) {
        targets.into_iter().filter(|t| t.group == home).collect()
    } else {
        targets
    }
}

/// What a click on one target does, once the target is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArbAction {
    /// Open the coin on this window's Main, without taking focus.
    OpenHere,
    /// Open the clicked chart and the target side by side in this window's comparison tab.
    CompareHere,
    /// Open the coin on the Main of the target's own group window, and bring that window forward.
    OpenAway,
}

/// Decide where a target opens.
///
/// A window charts only its own group's cores, so a target in any other group — or any target at
/// all from a window that belongs to no group — opens in its own group's window, and visibly:
/// opened there silently, the chart would change behind this window and the click would look dead.
/// A comparison holds both sides in ONE window, so across groups it degrades to opening the chart.
///
/// Args:
///     mode: What the pressed button asked for.
///     home: The clicked window's group, or `None` for a window that belongs to none.
///     target_group: The group whose window holds the target core.
///
/// Returns:
///     The action to take.
pub(super) fn arb_action(mode: ArbOpen, home: Option<&str>, target_group: &str) -> ArbAction {
    if home != Some(target_group) {
        return ArbAction::OpenAway;
    }
    match mode {
        ArbOpen::Chart => ArbAction::OpenHere,
        ArbOpen::Compare => ArbAction::CompareHere,
    }
}

#[cfg(test)]
mod tests;
