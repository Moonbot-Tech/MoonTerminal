//! The crowd's own detections: cards with no core behind them.
//!
//! Everything a detection that no core reported needs, and nothing the rest of the feed needs. It
//! is here rather than in the middle of the panel because the two sources answer different
//! questions: a core's detection arrives with a strategy, a market and a server colour, and this
//! one arrives with a coin, a minute and two figures.
//!
//! Such a card also STANDS DOWN when a core has already reported the same coin, on any exchange:
//! the two would be one event reported twice, and the core's card is the one with a strategy and a
//! market behind it. That is decided at presentation (`rules::crowd_card_yields`) rather than at
//! ingest, so the crowd's card is there, ready, for the moment the core's own card expires.
//!
//! The chart is the one thing such a card cannot supply itself, so it is BORROWED — from the first
//! core in this group that trades the coin, resolved exactly as clicking the ticker resolves it.
//! What is borrowed with it stops at the picture: the venue, the exchange kind and the price moves
//! of that core's market are facts about an instrument the crowd never named, and the card does not
//! print them (see `cards::field_applies`).

use gpui::{Context, Pixels, Point, Window};
use moon_core::config::DetectField;
use moon_core::crowd::CrowdDetect;
use moon_core::session::CoreId;
use moon_ui::MoonPalette;

use super::{DEFAULT_SERVER_COLOR, DETECT_THUMB_BARS, DetectItem, DetectsPanel, detect_expired};
use crate::design;

/// Where a card came from.
///
/// A detection used to be a core's by construction — its strategy fired, or a chart object it owns
/// did — and every field on the card was that core's. The crowd's rule is the first source with no
/// core at all: it watches a public service, on exchanges this terminal may not even be connected
/// to. So there is no core to name, no strategy to credit and no server colour to wear, and every
/// place that used to read `core` now has to say what it means for a card that has none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DetectOrigin {
    /// One of this group's cores reported it.
    Core(CoreId),
    /// The crowd's rolling minute crossed both lines of the rule: what the coin was worth, and on
    /// how many trades. Frozen here for the same reason every other field is — the window moves on,
    /// and a card states what fired it rather than what is true a minute later.
    Crowd { profit: f64, trades: u32 },
}

/// How many crowd cards the feed will hold at once.
///
/// It BOUNDS the crowd's share rather than removing it: the queue's own cap evicts by age and does
/// not care where a card came from, so a rule set loud enough to fire on everything would push out
/// the detections a core reported — the ones somebody is actually trading on. Holding the crowd to
/// its own seats first keeps that to five of forty-eight instead of all of them.
///
/// The rule's own burst is the same number, from the same constant: announcing more per pass than
/// can ever be on screen buys a market snapshot for a card trimmed in the same breath.
const CROWD_CARDS_MAX: usize = moon_core::crowd::detect::SEATS;

impl DetectsPanel {
    /// Take the crowd rule's new crossings and freeze a card for each.
    ///
    /// The chart is the one thing such a card cannot supply itself, so it is borrowed: the first
    /// core in this group that trades the coin, resolved exactly as clicking the ticker resolves
    /// it, and its snapshot frozen here like every other card's. A coin no core trades is a normal
    /// case and not an error — the crowd trades on exchanges this terminal need not be connected to
    /// — and the card then carries its figures with no chart behind them rather than an empty
    /// frame pretending to be one.
    ///
    /// Args:
    ///     now_ms: Wall clock of this pass.
    ///     cx: Panel context, for the service and the market source.
    ///
    /// Returns:
    ///     Whether the visible card collection changed.
    pub(super) fn ingest_crowd(&mut self, now_ms: f64, cx: &mut Context<Self>) -> bool {
        // Read once for the pass: a card freezes what these say when it FIRES, so a setting changed
        // now reaches the next crossing rather than the countdown already on screen.
        let cards = crate::chart_tabs::crowd_cards(&self.backend.read(cx).layout);
        let service = self.backend.read(cx).crowd();
        let head = service.read(cx).detects_head();
        if head == self.crowd_cursor {
            return false;
        }
        let fresh: Vec<CrowdDetect> = service
            .read(cx)
            .detects_since(self.crowd_cursor)
            .cloned()
            .collect();
        // Whatever the ring dropped before this panel read it is gone; catching the cursor up is
        // what keeps a panel that was away for an hour from asking the same question every time.
        self.crowd_cursor = head;
        let mut changed = false;
        for row in fresh {
            // A crossing whose card would be pruned on this very pass buys nothing but a market
            // snapshot. Same rule as the core feed applies to its own replay.
            if detect_expired(now_ms, row.at_ms as f64, cards.keep_ms()) {
                continue;
            }
            // Every seat taken, and the reader asked for what is on screen to be left alone. Judged
            // BEFORE the market snapshot, which is the expensive half of taking a card in. A coin
            // already holding a seat is not a newcomer and refreshes in place as always.
            let held = self.items.iter().filter(|it| it.crowd().is_some()).count();
            let known = self
                .items
                .iter()
                .any(|it| it.crowd().is_some() && it.base == row.coin);
            if !cards.evict && !known && held >= CROWD_CARDS_MAX {
                continue;
            }
            let (market, identity, snap) = {
                let backend = self.backend.read(cx);
                match crate::controls::coin_open::cores_for(backend, &self.group, &row.coin)
                    .into_iter()
                    .next()
                {
                    Some(hit) => {
                        let snap = backend.session.market_source().detect_snapshot(
                            hit.core,
                            &hit.market,
                            DETECT_THUMB_BARS,
                        );
                        // The identity is BORROWED with the chart, and it has to be: the crowd
                        // publishes a ticker and no catalog, so the only thing here that knows
                        // whether its `1kBONK` is Binance's coin is the core whose market this is.
                        let identity = backend
                            .session
                            .market_source()
                            .market_label(hit.core, &hit.market)
                            .identity();
                        (hit.market, identity, Some(snap))
                    }
                    // No core trades it, so there is no card it could be a duplicate of, and the
                    // folded ticker is both the best available answer and one nothing collides with.
                    None => (
                        String::new(),
                        moon_core::symbol::coin_match_key(&row.coin),
                        None,
                    ),
                }
            };
            let origin = DetectOrigin::Crowd {
                profit: row.profit,
                trades: row.trades,
            };
            // Unpacked once: a coin no core trades has no chart and no venue, and every one of
            // these is what that absence looks like on the card.
            let (bars, line, delta_24h, delta_1h, venue, exchange_kind) = match snap {
                Some(snap) => (
                    snap.bars,
                    snap.line,
                    snap.delta_24h,
                    snap.delta_1h,
                    snap.venue,
                    snap.exchange_kind,
                ),
                None => (Vec::new(), Vec::new(), 0.0, 0.0, None, String::new()),
            };
            if let Some(it) = self
                .items
                .iter_mut()
                .find(|it| it.crowd().is_some() && it.base == row.coin)
            {
                // The same coin crossing again refreshes its card in place, exactly as a core
                // detection repeating on the same market does.
                it.origin = origin;
                it.born_ms = row.at_ms as f64;
                it.ttl_ms = cards.keep_ms();
                it.market = market;
                it.identity = identity;
                it.bars = bars;
                it.line = line;
                it.delta_24h = delta_24h;
                it.delta_1h = delta_1h;
                it.venue = venue;
                it.exchange_kind = exchange_kind;
                changed = true;
                continue;
            }
            self.items.push_back(DetectItem {
                origin,
                // No core reported it, and the badge names the source at render instead.
                core_name: String::new(),
                market,
                // The CROWD's spelling of the coin, which is also what a click resolves from: the
                // catalog token of whichever core lent the chart can carry a contract tail the
                // crowd knows nothing about.
                base: row.coin,
                identity,
                // Unused by a crowd card: the rail takes the theme's accent, there is no strategy
                // kind to badge, no direction, no strategy and no chart tab to route to.
                color: DEFAULT_SERVER_COLOR,
                kind: 0,
                is_short: false,
                strat_name: String::new(),
                is_alert: false,
                add_to_chart: 0,
                born_ms: row.at_ms as f64,
                ttl_ms: cards.keep_ms(),
                bars,
                line,
                delta_24h,
                delta_1h,
                venue,
                exchange_kind,
            });
            changed = true;
        }
        // Trimmed among THEMSELVES first. See [`CROWD_CARDS_MAX`]: the queue's own cap below
        // evicts by age, and a rule fired often enough would push out what a core reported. Only
        // when the reader asked for the newest to win — otherwise nothing was taken in above and
        // there is nothing over the cap to trim.
        let crowd_held = self.items.iter().filter(|it| it.crowd().is_some()).count();
        if cards.evict && crowd_held > CROWD_CARDS_MAX {
            let mut over = crowd_held - CROWD_CARDS_MAX;
            // `retain` walks the queue in insertion order, so what goes is the crowd card that
            // has been in it longest. That is not always the oldest CROSSING — a coin that crosses
            // again is refreshed where it sits — but it is the one that has held a seat longest,
            // which is what a seat limit is about.
            self.items.retain(|it| {
                if over > 0 && it.crowd().is_some() {
                    over -= 1;
                    return false;
                }
                true
            });
            changed = true;
        }
        while self.items.len() > super::MAX_DETECT_BTNS {
            self.items.pop_front();
            changed = true;
        }
        changed
    }

    /// Open a crowd card's coin, the way every other bare ticker in the terminal opens.
    ///
    /// Both buttons do this, and the card goes when the CHART does — not when the click lands. A
    /// core card is dismissed by opening it because opening it is the whole answer; here the click
    /// may raise a picker instead, and taking the card away under an open menu would remove the
    /// thing the menu is about. So the dismissal waits for a chart to actually open, which also
    /// leaves the card alone for a coin no core trades, where the click does nothing.
    ///
    /// Args:
    ///     coin: The crowd's spelling of the ticker.
    ///     pos: Where the click landed, so a picker can be anchored to it.
    ///     window: Owning window, used to host that picker.
    ///     cx: Panel context.
    pub(super) fn open_crowd(
        &mut self,
        coin: &str,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity();
        let opened = coin.to_string();
        crate::controls::coin_open::open(
            &self.backend,
            &self.group,
            coin,
            pos,
            window,
            cx,
            move |app| {
                view.update(app, |this, cx| this.dismiss_crowd(&opened, cx));
            },
        );
    }

    /// Take a crowd card off the feed, its chart now being open.
    ///
    /// Args:
    ///     coin: The crowd's spelling of the ticker, which is what one card per coin is keyed by.
    ///     cx: Panel context.
    fn dismiss_crowd(&mut self, coin: &str, cx: &mut Context<Self>) {
        let before = self.items.len();
        self.items
            .retain(|it| !(it.crowd().is_some() && it.base == coin));
        if self.items.len() == before {
            return;
        }
        self.arm_prune_timer(cx);
        cx.notify();
    }
}

/// The colour a card is railed and badged in.
///
/// A core card wears its server's colour, frozen with the card so a recoloured server does not
/// repaint a detection that already happened. A crowd card has no server, so it takes the theme's
/// own accent — read at RENDER rather than frozen, because unlike a server colour it follows the
/// palette and a card that kept yesterday's accent through a theme switch would be the one thing on
/// screen still wearing it.
///
/// Args:
///     it: The card.
///     p: Active Moon palette.
pub(super) fn rail_color(it: &DetectItem, p: MoonPalette) -> u32 {
    match it.core() {
        Some(_) => design::rgb_to_u32(it.color),
        None => p.accent,
    }
}

/// What a crowd card says where a core card names its strategy.
///
/// The two figures the rule was actually read against, printed EXACTLY: two decimals and no
/// thousands suffix. The tables compact past a thousand because they are a column to be scanned;
/// this is the evidence for a threshold, and `+1.20K` beside a line drawn at 1200 would print a
/// figure that does not clear the line it crossed.
///
/// The sign is written out even though a crossing is always a gain: the column beside it can hold a
/// loss, and a bare number there would be read as one.
///
/// Args:
///     profit: Net profit of the crowd's minute at the crossing.
///     trades: Trades in that minute.
pub(super) fn crowd_chip_text(profit: f64, trades: u32) -> String {
    format!("{profit:+.2} · {trades}")
}

/// Whether a field has anything to say about THIS card.
///
/// A crowd detection knows a coin, a minute and two figures. It does not know an exchange or a
/// price move: the chart behind it is borrowed from whichever core in this group happens to trade
/// the coin, and printing that core's venue or its 24-hour percentage as the detection's own would
/// state a fact about an instrument the crowd never named. The chart itself is still drawn — a
/// picture of a market is a picture — but a percentage is a claim, and a coin no core trades would
/// carry a fabricated `+0.00%` measured from nothing.
///
/// Args:
///     field: The configured slot.
///     it: The card being drawn.
pub(super) fn field_applies(field: DetectField, it: &DetectItem) -> bool {
    it.crowd().is_none()
        || !matches!(
            field,
            DetectField::Delta24h
                | DetectField::Delta1h
                | DetectField::Exchange
                | DetectField::ExchangeKind
        )
}
