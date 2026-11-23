//! The chart's market BUTTONS: what they know, where they go, and what a press on one does.
//!
//! `Cancel Buy` and `Panic Sell` used to be GPUI elements placed by a per-tab setting that could
//! say `Left`, `Centre`, `Right` or nothing — through two different layout paths, one for a single
//! pane and one for a stack. They are captions now: the chart's own label model already answers
//! "what is printed, in which band, at which alignment, in what order", and a button is a thing
//! placed on a chart like any other.
//!
//! They are still the application's own `MoonButton`, and that is the whole shape of this module.
//! The chart draws in its own pass, which cannot host a GPUI element, so the two halves meet on a
//! RECTANGLE: the caption pass measures the label, reserves the room and publishes where it went;
//! this module resolves the state those captions print — which only the terminal can answer — and
//! places a real control at each published rectangle.
//!
//! Those rectangles come from the last PRESENTED frame while the controls are placed on the panel's
//! render, so during a resize a button can lag the plot by a frame. That is the price of a real
//! control on an own-pass chart.

use gpui::*;
use moon_core::config::ChartAction;
use moon_core::session::CoreId;
use moon_ui::{MoonButton, MoonButtonIconSlot, MoonButtonSegment, MoonButtonVariant, MoonRect};

use super::ChartPanel;
use crate::Backend;

/// Convert the chart's measured caption size and line box to MoonUI tier-segment inputs.
///
/// The caption is drawn by the chart at the monitor's density — its size is in the chart's own
/// logical pixels — while the segment is a GPUI element scaled by the window content zoom. So the
/// segment is handed the size divided by that zoom and lands on the same device pixels. The
/// legacy font delta must never enter the calculation. The line box matches the caption layout's
/// measured size plus four logical pixels.
///
/// Args:
///     cx: Application context providing the active theme's content zoom.
///     size: Caption font size as the chart measured it, in its own logical pixels.
///
/// Returns:
///     Unscaled font size and line height for a tier-sized button segment.
fn action_label_metrics(cx: &App, size: f32) -> (f32, f32) {
    let zoom = moon_ui::MoonTheme::content_zoom(cx).max(0.1);
    (size / zoom, (size + 4.0) / zoom)
}

/// The MoonUI icon the auto-figures eye draws: open while the pane shows auto figures, crossed out
/// while it does not.
///
/// Args:
///     shown: Whether the pane draws auto figures right now.
///
/// Returns:
///     The embedded asset path (`moon_ui::MoonAssets`).
fn eye_icon(shown: bool) -> &'static str {
    match shown {
        true => "icons/eye.svg",
        false => "icons/eye-off.svg",
    }
}

impl ChartPanel {
    /// Hand every pane the state its buttons print.
    ///
    /// Called from the panel's render, where the backend is in hand, for the same reason the old
    /// buttons resolved their armed state there: whether panic is armed mixes the core's snapshot
    /// with this terminal's own optimistic override, and whether a command is allowed at all is a
    /// property of THIS window's workspace rail. The engine holds neither.
    pub(super) fn sync_market_actions(&mut self, cx: &mut Context<Self>) {
        // A BOOK-ONLY pane is all order book, with no exceptions: every press in it is the book's,
        // and the buttons the old overlay drew were excluded from it for exactly that reason. The
        // whole roster goes quiet — the captions resolve to nothing, so no rectangle is published
        // and no control is placed.
        let wanted = match self.orderbook_only {
            true => crate::chartdx::WantedActions::default(),
            false => self.chart.wanted_market_actions(),
        };
        if !wanted.any() {
            // Cleared ONCE, on the edge: the state is latched on the pane, so a chart that just
            // lost its last button would go on printing an armed `Stop Panic`, while a chart that
            // never had one must not walk its panes on every render to clear nothing.
            if std::mem::take(&mut self.market_actions_pushed) {
                for pane in 0..self.chart.pane_count() {
                    self.chart.set_pane_actions(pane, None);
                }
            }
            return;
        }
        self.market_actions_pushed = true;
        let backend = self.backend.read(cx);
        for pane in 0..self.chart.pane_count() {
            let Some((core, market)) = self.chart.pane_target(pane) else {
                // A vacated or not-yet-targeted slot: cleared rather than skipped, or it would keep
                // printing the previous coin's state for as long as the slot is held.
                self.chart.set_pane_actions(pane, None);
                continue;
            };
            let state = crate::chartdx::MarketActionState {
                allowed: self.workspace_action_allowed(backend, core),
                // Each fact is read only where a button prints it: a profile without the lock or
                // the star pays nothing for them, and walking a core's lists for a caption that
                // does not exist is work per pane per render.
                panic_armed: wanted.panic_sell && backend.is_panic_armed(core, &market),
                // The DEADLINE, and the CORE's own rather than one derived from the clock — see
                // `CoreData::temp_ban_until_ms`. Derived here it would move on every render once
                // the countdown saturates, re-formatting the caption forever to print the same
                // minute. A row the core still lists is a ban whatever that countdown reached,
                // which is also the state the coin menu offers the lift in.
                ban_until_ms: wanted
                    .temp_ban
                    .then(|| {
                        backend
                            .session
                            .store()
                            .core(core)
                            .and_then(|data| data.temp_ban_until_ms(&market))
                    })
                    .flatten(),
                // Read only where a star prints it, like the two facts above, and by the CORE's
                // own name for the coin rather than by the market: its favourites list is matched
                // against `market_currency`. `None` here is the core's silence — or a catalogue
                // that has not named the market yet — not an unmarked coin; see
                // `Backend::fav_market`.
                favorite: wanted
                    .favorite
                    .then(|| {
                        let coin = self.chart.pane_coin(pane)?;
                        backend.fav_market(core, &coin)
                    })
                    .flatten(),
                // The panel's OWN answer — the tab's switch, or the eye's override for this coin —
                // read against the configuration the backend holds now, so the eye is right on the
                // first render after a press or a market change, before any sync ran.
                auto_figures: wanted.auto_figures
                    && self.auto_figures_shown(
                        backend.layout.auto_figures_for(self.default_kind),
                        &(core, market.clone()),
                    ),
            };
            self.chart.set_pane_actions(pane, Some(state));
        }
    }

    /// One real button per rectangle the caption pass published.
    ///
    /// Placed by BOUNDS rather than by flow, because the flow was already decided — by the caption
    /// layout, in the band and at the alignment the reader chose. The control keeps everything a
    /// button has and a drawn rectangle cannot: the application's own type, its hover and press
    /// states, its disabled look, and the pointer over it.
    pub(super) fn action_buttons(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        if !self.market_actions_pushed {
            return Vec::new();
        }
        // The eye's press lands on THIS panel, which owns the per-pane override; every other
        // button goes to the backend.
        let panel = cx.entity().downgrade();
        // The published rectangles are in the CHART's logical pixels and this overlay is laid out
        // inside the chart SLOT in content pixels, so the slot's own position comes off and the
        // zoom divides — the same conversion the arbitrage cursor zones make, through the same
        // helper.
        let Some((origin, _, zoom)) = self.chart_origin_logical() else {
            return Vec::new();
        };
        let mut out: Vec<AnyElement> = Vec::new();
        for pane in 0..self.chart.pane_count() {
            for button in self.chart.action_buttons(pane) {
                if button.w <= 0.0 || button.h <= 0.0 {
                    continue;
                }
                let (font_size, line_height) = action_label_metrics(cx, button.size);
                let (variant, selected) = match (button.action, button.active) {
                    // An armed panic and a running ban are the same statement — this control is ON
                    // — and `selected` is how every other button in the application says it.
                    (ChartAction::PanicSell, active) => (MoonButtonVariant::Danger, active),
                    // A running ban and a marked coin are the same statement — this control is ON
                    // — and they wear it alike: the accent only while it is.
                    (ChartAction::TempBan | ChartAction::Favorite, true) => {
                        (MoonButtonVariant::Amber, true)
                    }
                    (ChartAction::TempBan | ChartAction::Favorite, false) => {
                        (MoonButtonVariant::Soft, false)
                    }
                    (ChartAction::CancelBuy, _) => (MoonButtonVariant::Soft, false),
                    // A display toggle wears no accent: on Main it is ON by default, and an amber
                    // square on every chart would shout a state nobody chose. The icon — an open or
                    // a crossed-out eye — is what it says.
                    (ChartAction::AutoFigures, _) => (MoonButtonVariant::Soft, false),
                };
                let panel = panel.clone();
                let backend = self.backend.clone();
                let group = self.workspace_group.clone();
                let (action, active, core, market, coin) = (
                    button.action,
                    button.active,
                    button.core,
                    button.market,
                    button.coin,
                );
                // Keyed by the CAPTION it came from, not by its place in the list: a button dropped
                // for lack of room would otherwise hand its hover and press state to the next one.
                let id =
                    SharedString::from(format!("chart-act-{pane}-{}-{}", button.row, button.part));
                // The chart owns the rectangle, including its inset. Tier padding would consume
                // that room again and clip the measured caption.
                let control = MoonButton::new(id).padding_x(0.0);
                let control = match action {
                    // An icon rather than the caption's text: a crossed-out eye exists in MoonUI's
                    // icon set and in no font. Sized like the caption, so the caption-size step
                    // moves it with the square it sits in.
                    ChartAction::AutoFigures => control
                        .leading_icon(MoonButtonIconSlot::new(eye_icon(active)).size(font_size)),
                    _ => control.segment(
                        MoonButtonSegment::new(button.label)
                            .font_size(font_size)
                            .line_height(line_height)
                            .mono(true),
                    ),
                };
                out.push(
                    div()
                        .absolute()
                        .left(px((button.x - origin.0) / zoom))
                        .top(px((button.y - origin.1) / zoom))
                        .w(px(button.w / zoom))
                        .h(px(button.h / zoom))
                        // The chart's own input lies UNDER this overlay and reads a left press as a
                        // trading gesture. `MoonButton` stops propagation only while it is disabled
                        // — so an enabled one would place an order under the control that was
                        // pressed. The wrapper swallows the press whatever state the button is in.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            control
                                .variant(variant)
                                .selected(selected)
                                .disabled(!button.enabled)
                                .bounds(MoonRect::new(0.0, 0.0, button.w / zoom, button.h / zoom))
                                .on_click(move |event, window, app| {
                                    let (market, coin) = (market.clone(), coin.clone());
                                    // The eye is the panel's, not the backend's: nothing to send,
                                    // nothing to save — see `toggle_auto_figures_eye`. A panel
                                    // already released has no eye left to flip, hence `let _`.
                                    if action == ChartAction::AutoFigures {
                                        let _ = panel.update(app, |this, cx| {
                                            this.toggle_auto_figures_eye(core, market, cx)
                                        });
                                        return;
                                    }
                                    // The lock is the one control that ASKS. Closing it is a choice
                                    // — which list, which cores, how long — so it opens the coin
                                    // menu; opening it is not, so a running ban lifts on the press
                                    // with nothing in between.
                                    if action == ChartAction::TempBan && !active {
                                        open_lock_coin_menu(
                                            &backend,
                                            group.clone(),
                                            core,
                                            market,
                                            coin,
                                            event.position(),
                                            window,
                                            app,
                                        );
                                        return;
                                    }
                                    dispatch_market_action(
                                        &backend,
                                        group.as_deref(),
                                        action,
                                        core,
                                        market,
                                        coin,
                                        app,
                                    );
                                })
                                .render(),
                        )
                        .into_any_element(),
                );
            }
        }
        out
    }
}

/// Send what the pressed button asks for.
///
/// The workspace rail is re-validated HERE, inside the same borrow that dispatches, rather than
/// trusted from the state the button was drawn with: that state is a frame old, and the rail can
/// have moved to another core since. A closed rail already disables the control; this is the second
/// gate, and it is the one that counts.
///
/// Args:
///     backend: Live command and workspace authority.
///     group: Owning chart group, or `None` for an unscoped diagnostic chart.
///     action: What the pressed button does.
///     core: Core the button was drawn for.
///     market: Market it was drawn for.
///     coin: That market's `market_currency`, which is what the core's favourites list is matched
///         against; empty while the catalogue has not named the market.
///     app: Application context for the update.
fn dispatch_market_action(
    backend: &Entity<Backend>,
    group: Option<&str>,
    action: ChartAction,
    core: CoreId,
    market: String,
    coin: String,
    app: &mut App,
) {
    backend.update(app, |b, cx| {
        if !b.workspace_action_allows_core(group, core) {
            return;
        }
        match action {
            ChartAction::CancelBuy => {
                if let Err(error) = b.session.cancel_market_buys(core, market) {
                    log::warn!("cancel market buys failed: {error:#}");
                }
            }
            ChartAction::PanicSell => {
                b.toggle_panic_sell(core, market);
            }
            // The COIN, not the market: the core matches its favourites against
            // `market_currency`. See `Backend::toggle_fav_market`.
            ChartAction::Favorite => b.toggle_fav_market(core, &coin),
            // Never reaches here: the button's own handler routes the eye to its panel before the
            // backend is touched, because it has nothing to send.
            ChartAction::AutoFigures => {}
            // Only a CLOSED lock reaches here: it lifts the ban that runs. An open one opens the coin
            // menu instead, whose rows set the ban — see the button's own handler, which decided
            // against the same state the lock was DRAWN in, so the picture and the command agree.
            ChartAction::TempBan => {
                if let Err(error) = b.session.set_temp_ban(core, market.clone(), None) {
                    log::warn!(
                        "chart: temp blacklist {market} on core {} failed: {error:#}",
                        moon_core::feed::core_label(core)
                    );
                }
            }
        }
        cx.notify();
    });
}

/// Open the shared coin menu at the point the open lock was pressed.
///
/// The SAME menu a right click on a coin opens in the Report, Orders and Assets — permanent and
/// temporary blacklists on this core and on the group's cores, «Где в чёрных списках», compare,
/// copy — rather than a menu of the lock's own. A coin the trader is only watching has no other
/// chart route to the permanent list (the coin menu opens on the chart only over an order line),
/// and a second copy of those rows would drift from the first.
///
/// «Selected cores» is the chart group's workspace scope, as the Report's is its own; an unscoped
/// chart offers every core this terminal is configured with (a core whose catalogue does not carry
/// the coin is left out by the menu itself). Each row re-validates the workspace rail when it is CHOSEN —
/// the menu can stand open while an Auto workspace moves a core out of scope.
///
/// Args:
///     backend: Live command and workspace authority.
///     group: Owning chart group, or `None` for an unscoped diagnostic chart.
///     core: Core the lock was drawn for.
///     market: Market it was drawn for.
///     coin: That market's `market_currency`; empty while the pane has not resolved it, and then
///         the catalogue is asked, as the order-line menu does.
///     at: Where to open, in window coordinates — the press itself.
///     window: Window that owns the menu.
///     app: Application context.
#[allow(clippy::too_many_arguments)] // one per fact the menu context needs; no struct owns them all
fn open_lock_coin_menu(
    backend: &Entity<Backend>,
    group: Option<String>,
    core: CoreId,
    market: String,
    coin: String,
    at: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    let ctx = {
        let b = backend.read(app);
        if !b.workspace_action_allows_core(group.as_deref(), core) {
            return;
        }
        let sessions = b.session.sessions();
        let core_name = sessions
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let selected_cores: Vec<CoreId> = match group.as_deref() {
            Some(group) => b
                .effective_workspace_scope(group, crate::workspace::RetainedCoreScope::All)
                .ids()
                .to_vec(),
            None => sessions.iter().map(|s| s.id).collect(),
        };
        let coin = match coin.is_empty() {
            true => b.session.market_source().market_label(core, &market).coin,
            false => coin,
        };
        crate::controls::CoinMenuCtx {
            core,
            core_name,
            market,
            coin,
            selected_cores,
            strat_id: None,
            strat_name: None,
            order_uid: None,
            workspace_group: group,
            side: None,
            short: false,
            // Already on this chart: the menu leaves out «Открыть на графике».
            origin: crate::controls::CoinMenuOrigin::ChartLine,
            history: None,
            trailing: Vec::new(),
            filter_here: None,
        }
    };
    crate::controls::open_coin_menu(ctx, backend.clone(), at, window, app);
}

#[cfg(test)]
mod tests;
