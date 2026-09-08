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
use moon_core::config::{ChartAction, TempBanSpan};
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant,
    MoonContextMenuWindowExt as _, MoonMenuItem, MoonRect, MoonWindowExt as _,
};
use rust_i18n::t;

use super::ChartPanel;
use crate::Backend;

/// Line box a button's label draws in, as a share of its own size.
///
/// The caption pass reserves `line_h = size + 4` around the text — see `Item::line_h` — and this is
/// that rule stated as a ratio, because the component takes a multiple rather than a sum. Close
/// enough at every size the caption clamp allows, and it is the label's box inside a rectangle that
/// was already sized for it.
const BUTTON_LINE_RATIO: f32 = 1.3;

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
                // Each fact is read only where a button prints it. The shipped pair carries no ban
                // button, and walking a core's blacklist for one nobody placed is work per pane per
                // render for a caption that does not exist.
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
    pub(super) fn action_buttons(&self, cx: &App) -> Vec<AnyElement> {
        if !self.market_actions_pushed {
            return Vec::new();
        }
        // The published rectangles are in the WINDOW's logical pixels and this overlay is laid out
        // inside the chart SLOT, so the slot's own position comes off — the same conversion the
        // arbitrage cursor zones make, through the same helper.
        let Some((origin, _)) = self.chart_origin_logical() else {
            return Vec::new();
        };
        let mut out: Vec<AnyElement> = Vec::new();
        for pane in 0..self.chart.pane_count() {
            for button in self.chart.action_buttons(pane) {
                if button.w <= 0.0 || button.h <= 0.0 {
                    continue;
                }
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
                };
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
                out.push(
                    div()
                        .absolute()
                        .left(px(button.x - origin.0))
                        .top(px(button.y - origin.1))
                        .w(px(button.w))
                        .h(px(button.h))
                        // The chart's own input lies UNDER this overlay and reads a left press as a
                        // trading gesture. `MoonButton` stops propagation only while it is disabled
                        // — so an enabled one would place an order under the control that was
                        // pressed. The wrapper swallows the press whatever state the button is in.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            MoonButton::new(id)
                                // The label as a SEGMENT rather than a plain one, for its size: the
                                // rectangle was measured at the caption's own font, so the words in
                                // it draw at that same number and the reader's size step moves both
                                // together. Base value, because the component scales what it is
                                // given — see `design::font_base_for`. Mono for the same reason: it
                                // is the face the width was measured in.
                                .segment(
                                    MoonButtonSegment::new(button.label)
                                        .font_size(crate::design::font_base_for(cx, button.size))
                                        .line_height(crate::design::font_base_for(
                                            cx,
                                            button.size * BUTTON_LINE_RATIO,
                                        ))
                                        .mono(true),
                                )
                                .size(MoonButtonSize::Micro)
                                .variant(variant)
                                .selected(selected)
                                .disabled(!button.enabled)
                                .bounds(MoonRect::new(0.0, 0.0, button.w, button.h))
                                .on_click(move |event, window, app| {
                                    let (market, coin) = (market.clone(), coin.clone());
                                    // The lock is the one control that ASKS. Closing it is a choice
                                    // — an hour, a day, three days — and a button cannot carry four
                                    // answers; opening it is not, so a running ban lifts on the
                                    // press with nothing in between.
                                    if action == ChartAction::TempBan && !active {
                                        open_ban_menu(
                                            &backend,
                                            group.clone(),
                                            core,
                                            market,
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
                                        None,
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
///     span: How long to ban for, or `None` to lift a ban that is running; the other two ignore it.
///     core: Core the button was drawn for.
///     market: Market it was drawn for.
///     coin: That market's `market_currency`, which is what the core's favourites list is matched
///         against; empty while the catalogue has not named the market.
///     app: Application context for the update.
#[allow(clippy::too_many_arguments)]
fn dispatch_market_action(
    backend: &Entity<Backend>,
    group: Option<&str>,
    action: ChartAction,
    span: Option<TempBanSpan>,
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
            // One control, both directions: a span sets the ban, `None` lifts the one that runs.
            // The press decided which — see the button's own handler — and it decided against the
            // same state the lock was DRAWN in, so the picture and the command agree.
            ChartAction::TempBan => {
                let ban = span.map(TempBanSpan::duration);
                if let Err(error) = b.session.set_temp_ban(core, market.clone(), ban) {
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

/// Ask how long to ban this coin for, at the point the lock was pressed.
///
/// A menu rather than four buttons on the chart: the spans are MoonBot's own four, they are chosen
/// once and then forgotten, and a control per span would take the plot's whole bottom edge to say
/// what one lock already says.
///
/// The workspace rail is checked when a row is CHOSEN, not here: the menu can stand open while an
/// Auto workspace moves the core out of this group's scope, and the row that was picked is the
/// command — see [`dispatch_market_action`].
///
/// Args:
///     backend: Live command and workspace authority.
///     group: Owning chart group, or `None` for an unscoped diagnostic chart.
///     core: Core the lock was drawn for.
///     market: Market it was drawn for.
///     at: Where to open, in window coordinates — the press itself.
///     window: Window that owns the menu.
///     app: Application context.
fn open_ban_menu(
    backend: &Entity<Backend>,
    group: Option<String>,
    core: CoreId,
    market: String,
    at: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    let items: Vec<MoonMenuItem> = TempBanSpan::ALL
        .iter()
        .map(|&span| {
            let (backend, group, market) = (backend.clone(), group.clone(), market.clone());
            let hours = span.hours();
            MoonMenuItem::with_key(
                SharedString::from(format!("chart-ban-{hours}h")),
                // Spelled in DAYS only past a day, exactly as the coin menu spells the same four.
                match hours > 24 {
                    true => t!("coin_menu.tbl_days", n = hours / 24).to_string(),
                    false => t!("coin_menu.tbl_hours", n = hours).to_string(),
                },
            )
            .on_click(move |_, window, app| {
                window.close_context_menu(app);
                dispatch_market_action(
                    &backend,
                    group.as_deref(),
                    ChartAction::TempBan,
                    Some(span),
                    core,
                    market.clone(),
                    // A ban is keyed by the MARKET, so the coin this dispatch also carries is not
                    // read on this path; see `coin_menu::temp_ban_symbol`.
                    String::new(),
                    app,
                );
            })
        })
        .collect();
    window.open_fitted_moon_context_menu(
        app,
        "chart-ban-menu",
        at,
        items,
        BAN_MENU_MIN_W,
        BAN_MENU_MAX_W,
    );
}

/// Width bounds for that menu, in design pixels: enough for the longest localized span.
const BAN_MENU_MIN_W: f32 = 120.0;
const BAN_MENU_MAX_W: f32 = 240.0;
