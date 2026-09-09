//! The crowd's statistics on an empty Main: three tables and nothing else.
//!
//! What the public statistics service publishes about everybody who agreed to be counted — every
//! closed trade as it happens, plus its own coin and trader boards for the rolling day. The
//! figures and the wire live in [`moon_core::crowd`]; this module only draws them.
//!
//! It is cheap by construction, not by tuning, and the three claims below are each readable in
//! `logs/render_diag.log` (`crowd_tick`, `crowd_render`, `crowd_render_us`):
//!
//! * **one tick a second.** A minute-wide window is not read any better at thirty samples a
//!   second, and a second is the shortest interval at which a figure in it can visibly change.
//! * **a repaint only when the tables change.** The tick drains the feed and ages the window
//!   either way, but it notifies only if the rows it would draw differ from the rows on screen. A
//!   quiet market therefore costs one comparison a second and no frames at all.
//! * **frames only while something is actually moving.** The boards are alive — rows slide to
//!   their new places, rows that drop off sink and fade, rows light up when trades land on them —
//!   and all of that is computed from elapsed time, so the view asks for frames at its OWN rate
//!   while movement lasts and stops asking the moment it ends. Handed to the framework's
//!   animations instead, the same movement pinned a window at sixty frames a second under a busy
//!   market: an animation requests a frame every frame for as long as it runs.
//!
//! Nothing here is on the chart's path and nothing here has a canvas: it is text in absolutely
//! placed rows, so there is no atlas, no sprite and nothing to rasterise.
//!
//! Clicking a coin opens its chart on Main, exactly as clicking one in the news feed does: one
//! core trading it opens outright, several offer a picker naming the core and its exchange.
//!
//! The view exists while the empty screen is showing at least one of its tables — including while
//! a chart is drawn OVER that screen, so closing the chart comes back to a minute that has been
//! counted all along. What a covering chart stops is the drawing, not the reading.
//!
//! Which tables are on is changed IN PLACE, and it is
//! built FOR that set: the minute opens the trade socket, the two day boards share one REST poller,
//! and a table nobody shows opens nothing at all. Dropping the view stops whatever it started, so a
//! terminal with a chart open holds no connection to the statistics service.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, IntoElement, ParentElement, Pixels, Point, Render, Styled, Window, div,
};
use moon_core::crowd::board::{CoinDay, DaySummary, Trader};
use moon_core::crowd::{EventSource, Feed, FeedConfig, Minute, Standing, Wants, Wire};

use moon_ui::{MoonContextMenuWindowExt as _, MoonWindowExt as _};

use crate::Backend;
use crate::controls::coin_search;
use crate::design;
use crate::diag;
use table::Look;
use table::motion::Motion;

mod table;

/// The fastest the boards are redrawn while something on them is moving.
///
/// The view's OWN clock, not the monitor's, and it is the whole answer to "what does this cost
/// when the market goes mad": the rate has nothing to do with how many trades arrived.
///
/// Twelve a second — enough that a four-hundred-millisecond slide is five steps rather than a
/// jump, and one tenth of what a screen full of tables would ask for at display rate.
const FRAME: Duration = Duration::from_millis(80);
/// How often the window is aged and the feed drained.
const TICK: Duration = Duration::from_secs(1);
/// A gap between ticks longer than this means the view was frozen — dragged, blocked or asleep —
/// and whatever the wire buffered meanwhile is no longer a minute of anything.
const STALE_AFTER: f32 = 10.0;

/// Which of the three tables this view is showing.
///
/// It decides both halves of the cost: which tables are built, and which connections are opened.
/// The minute is the live trade socket; the two day boards share one REST poller, which fetches
/// only the boards that are shown. A change is passed to the view rather than rebuilding it —
/// what is already open stays open, and only the halves that changed are started or stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CrowdParts {
    /// The rolling minute of the crowd's trades, top right.
    pub(crate) minute: bool,
    /// The service's trader board for the day, down the left.
    pub(crate) traders: bool,
    /// The service's coin board for the day, bottom right.
    pub(crate) coins: bool,
}

impl CrowdParts {
    /// Whether any table is on. A view with none has nothing to draw and nothing to read.
    pub(crate) fn any(self) -> bool {
        self.minute || self.traders || self.coins
    }

    /// Which halves of the service that set has to read.
    fn wants(self) -> Wants {
        Wants {
            trades: self.minute,
            coins: self.coins,
            traders: self.traders,
        }
    }
}

/// What is currently drawn, so a tick that changes nothing does not repaint.
///
/// This is the whole invalidation model: the slice that can change is these rows plus the one word
/// about the wire; the only thing observing it is this view; and it re-renders exactly when one of
/// them differs.
#[derive(PartialEq)]
struct Shown {
    rows: Vec<Standing>,
    /// What can be said about the trade stream — `None` when the minute is off, because there is
    /// then no stream, nothing draws the word, and a wire that flickered would otherwise repaint a
    /// screen that shows nothing about it.
    wire: Option<Wire>,
    /// The service's day boards, as they last stood. They change about once a minute, which is
    /// exactly why they are compared rather than redrawn: fifty-six of every fifty-seven seconds
    /// have nothing new in them.
    coins: Vec<CoinDay>,
    traders: Vec<Trader>,
    /// The day's own totals, which the trader board carries in its heading.
    summary: Option<DaySummary>,
}

/// The three tables, their feed, and the movement between two boards.
pub(crate) struct CrowdStatsView {
    /// Which tables this view is showing.
    ///
    /// Changed in place by [`Self::show`] rather than by building a new view: everything else here
    /// — the minute counted so far, the open socket, the boards last read — is worth keeping
    /// across a change of mind about which of them to draw.
    parts: CrowdParts,
    feed: Feed,
    minute: Minute,
    /// What moved on each of the three boards since the last tick. Kept per table because the
    /// three move on their own clocks: the minute every second, the day boards once a minute.
    minute_moves: Motion<Standing>,
    coin_moves: Motion<CoinDay>,
    trader_moves: Motion<Trader>,
    /// How many boards the feed had replaced when they were last settled. What it answers is "is
    /// this a NEW board" — which is what the rank marks live and die by: they stand until the next
    /// board comes in, however long that takes, and go if that board moved nobody.
    boards_seen: (u64, u64),
    /// How many places of RANK each trader moved when the last board arrived, by account.
    ///
    /// Counted here rather than in the movement, because it is about the SERVICE's rank and not
    /// about which screen row somebody is drawn on: a row the parser drops moves everybody under
    /// it up the screen without moving anybody's rank at all.
    trader_marks: HashMap<u64, i32>,
    /// The terminal this screen belongs to, for opening a coin's chart.
    backend: Entity<Backend>,
    /// Which group window it belongs to, which is what scopes both the search and the open.
    group: String,
    /// Whether the screen this view draws on is actually being shown.
    ///
    /// A chart opening covers the empty screen without ending it: the tables go on reading, so
    /// that closing the chart comes back to a minute that has been counted all along rather than
    /// to an empty one filling from scratch. What stops while it is covered is DRAWING — no
    /// repaint is asked for and no frame chain is started, because a repaint of something nobody
    /// can see is the whole of what it would cost.
    drawn: bool,
    /// Whether a frame chain is running. One at a time: the chain stops itself when the movement
    /// ends, and a second one would double the frame rate for as long as both lasted.
    drawing: bool,
    /// Start of the clock the window is measured on. Arrivals are stamped against it, so a minute
    /// is a minute of these tables being open.
    started: Instant,
    last_tick: Instant,
    shown: Shown,
}

impl CrowdStatsView {
    /// Open the tables that were asked for and start reading for them.
    ///
    /// Args:
    ///     parts: Which tables to show — and therefore which halves of the service to read.
    ///     backend: The terminal, for opening a coin's chart.
    ///     group: The group window this screen belongs to.
    ///     cx: The view's context.
    pub(crate) fn new(
        parts: CrowdParts,
        backend: Entity<Backend>,
        group: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let now = Instant::now();
        // The chain is started here and nowhere else, so there is nothing to guard against a
        // second one.
        crate::pulse::arm_every(TICK, cx, |this: &mut Self, cx| {
            this.tick(cx);
            true
        });
        // A run that is not a person watching the market gets the seeded stand-in: `--fixture` is
        // a bench on recorded data and `--debug-script` is FireTest, and both assert that nothing
        // reaches the network. A socket opened under either would be both a surprise and noise in
        // what they measure.
        let live = (moon_core::fixture::active().is_none() && !crate::firetest::scripted())
            .then(FeedConfig::default);
        let feed = Feed::open(live, seed(), parts.wants());
        // The first frame is drawn before the first tick, so it takes the feed's word for itself
        // now: a run that opened saying "no connection" for a second would be describing a wire it
        // had not tried yet.
        let shown = Shown {
            rows: Vec::new(),
            wire: parts.minute.then(|| feed.wire()),
            coins: Vec::new(),
            traders: Vec::new(),
            summary: None,
        };
        Self {
            parts,
            backend,
            group,
            drawn: true,
            feed,
            minute: Minute::new(),
            minute_moves: Motion::default(),
            coin_moves: Motion::default(),
            trader_moves: Motion::default(),
            boards_seen: (0, 0),
            trader_marks: HashMap::new(),
            drawing: false,
            started: now,
            last_tick: now,
            shown,
        }
    }

    /// Say whether the screen this view draws on is being shown.
    ///
    /// Args:
    ///     drawn: Whether it is on screen.
    ///     cx: The view's context, for the repaint that brings it back.
    pub(crate) fn set_drawn(&mut self, drawn: bool, cx: &mut Context<Self>) {
        if self.drawn == drawn {
            return;
        }
        self.drawn = drawn;
        if drawn {
            // Back on screen with a minute that kept counting: draw it once, now, rather than
            // waiting up to a second for the next tick to notice something changed.
            cx.notify();
        }
    }

    /// Show exactly these tables from now on, opening or closing what they read.
    ///
    /// Args:
    ///     parts: The new set.
    ///     cx: The view's context, for the repaint.
    pub(crate) fn show(&mut self, parts: CrowdParts, cx: &mut Context<Self>) {
        if self.parts == parts {
            return;
        }
        self.parts = parts;
        self.feed.want(parts.wants());
        // A table that has just been switched OFF must not go on moving: settled against an empty
        // board it would mark every row as leaving, and the frame chain would then draw ten frames
        // of a farewell nobody is watching.
        if !parts.minute {
            self.minute_moves = Motion::default();
            self.minute = Minute::new();
        }
        if !parts.coins {
            self.coin_moves = Motion::default();
        }
        if !parts.traders {
            self.trader_moves = Motion::default();
            self.trader_marks.clear();
        }
        // The wire belongs to the minute: asked about a stream that has only just been started,
        // it must say so now rather than showing last minute's answer until the next tick.
        self.shown.wire = parts.minute.then(|| self.feed.wire());
        // A table that has just been switched on has nothing in it until the next tick, and the
        // one that was already there must not blink: only the SET changed, so the rows stand.
        cx.notify();
    }

    /// Drain whatever arrived, age the window, and repaint only if the tables moved.
    fn tick(&mut self, cx: &mut Context<Self>) {
        diag::bump(&diag::CROWD_TICK);
        let now = Instant::now();
        // The real interval, uncapped: a machine that slept for an hour ages an hour out of the
        // window and lets the trade rates fall to nothing.
        let dt = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;
        let now_ms = now.duration_since(self.started).as_millis() as u64;

        // What arrived is stamped with the moment it was DRAINED, so a backlog that piled up while
        // this view was frozen would enter the window as one simultaneous burst — a minute of
        // trades reported as one second of them. After a gap this long the honest table is an
        // empty one: throw the backlog away and let the minute fill again.
        //
        // What ARRIVED this second is counted coin by coin as it goes past, because nothing on a
        // row can answer it: the window's figures move only when its CONTENTS change, so a coin
        // taking a $10 win every second while a $10 win ages out shows the same numbers minute
        // after minute — and that is exactly the coin worth lighting up.
        // Drained even when the minute is off, because a feed opened without the trade thread
        // delivers nothing to drain and one that was opened WITH it must not be left to fill up.
        let trades = self.feed.drain(now_ms);
        diag::bump_by(&diag::CROWD_TRADES, trades.len() as u64);
        // Keyed rather than scanned: a busy second is hundreds of trades over dozens of coins, and
        // a linear search per trade turns that into thousands of comparisons on the UI thread.
        let mut landed: HashMap<String, f64> = HashMap::new();
        if dt < STALE_AFTER {
            for trade in trades {
                *landed.entry(trade.coin.clone()).or_insert(0.0) += trade.profit;
                self.minute.push(trade);
            }
        }
        self.minute.tick(now_ms);

        // Only what is drawn is derived, and only what is drawn is compared: a table that is
        // switched off must not be able to cause a repaint.
        let shown = Shown {
            rows: if self.parts.minute {
                self.minute.standings(table::MINUTE_SEATS)
            } else {
                Vec::new()
            },
            wire: self.parts.minute.then(|| self.feed.wire()),
            // Only the rows that are DRAWN, for the same reason as the traders below.
            coins: if self.parts.coins {
                self.feed
                    .coins()
                    .iter()
                    .take(table::DAY_COIN_SEATS)
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            },
            // Only the rows that are DRAWN. The service sends fifty and twenty are shown, so
            // keeping all of them here meant a change in row forty-two failed the comparison and
            // repainted a screen on which nothing had moved.
            traders: if self.parts.traders {
                self.feed
                    .traders()
                    .iter()
                    .take(table::TRADERS_SHOWN)
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            },
            summary: self.parts.traders.then(|| self.feed.summary()).flatten(),
        };
        // What is new since the last board, before the boards are swapped.
        let marks = self.settle(&shown, &landed, now);
        // The marks are drawn but are not part of the board, so they have to be asked about
        // separately: a new board that moved nobody rubs every mark out while leaving all twenty
        // rows exactly as they were, and without this the screen would go on showing them.
        let moved = shown != self.shown || marks != self.trader_marks;
        self.shown = shown;
        self.trader_marks = marks;
        // Covered by a chart, the rows are still kept up to date — they are simply not drawn.
        if moved && self.drawn {
            cx.notify();
        }
        // And if that left anything moving, the view draws itself at ITS rate until it stops.
        self.draw_while_moving(cx);
    }

    /// Whether any of the three boards has something in flight.
    fn moving(&self, now: Instant) -> bool {
        self.minute_moves.moving(now)
            || self.coin_moves.moving(now)
            || self.trader_moves.moving(now)
    }

    /// Start the frame chain if something is moving and one is not already running.
    ///
    /// This is the only thing here that draws faster than once a second, it runs at a rate of our
    /// own choosing, and it stops itself the moment the last row settles — which is what keeps "a
    /// still board costs nothing" true whatever the market does.
    fn draw_while_moving(&mut self, cx: &mut Context<Self>) {
        if self.drawing || !self.drawn || !self.moving(Instant::now()) {
            return;
        }
        self.drawing = true;
        crate::pulse::arm_every(FRAME, cx, |view: &mut Self, cx| {
            cx.notify();
            // One frame past the end, so the last of a fade is actually drawn and a row that has
            // finished leaving is taken off the screen.
            let carry_on = view.drawn && view.moving(Instant::now());
            view.drawing = carry_on;
            carry_on
        });
    }

    /// Work out what moved between the boards on screen and the ones about to replace them.
    ///
    /// Three boards, three questions, and each one is asked of the FIGURES rather than of the
    /// order: a coin glows when trades landed on it, a coin of the day when its money moved, a
    /// trader when either did. The ORDER is the movement, and that the tables work out for
    /// themselves from the keys.
    ///
    /// Args:
    ///     next: The boards that are about to be drawn.
    ///     landed: What arrived this tick, by coin, with the net money it brought.
    ///     now: The clock the glow and the farewell are measured on.
    ///
    /// Returns:
    ///     The rank marks the trader board should wear — the ones standing, or fresh ones if a new
    ///     board has just arrived.
    fn settle(
        &mut self,
        next: &Shown,
        landed: &HashMap<String, f64>,
        now: Instant,
    ) -> HashMap<u64, i32> {
        // The two day boards are the SERVICE's, polled about once a minute: a new one is a new one,
        // and another look at the same one is not. The minute is OURS — every tick is a new board
        // of it, because we add it up ourselves.
        let seen = self.feed.boards_seen();
        for (coin, net) in landed {
            // Only a coin that is ALREADY on the board: a row arriving is a movement of its own,
            // and lighting it up as well says one thing twice.
            if self.shown.rows.iter().any(|old| old.coin == *coin) {
                self.minute_moves.beat(coin, *net >= 0.0, now);
            }
        }
        if !self.parts.minute {
            return self.trader_marks.clone();
        }
        self.minute_moves.settle(
            &next
                .rows
                .iter()
                .map(|row| (row.coin.clone(), row.clone()))
                .collect::<Vec<_>>(),
            now,
        );

        for row in &next.coins {
            if let Some(was) = self
                .shown
                .coins
                .iter()
                .find(|old| old.coin == row.coin && old.profit != row.profit)
            {
                self.coin_moves
                    .beat(&row.coin, row.profit > was.profit, now);
            }
        }
        self.coin_moves.settle(
            &next
                .coins
                .iter()
                .map(|row| (row.coin.clone(), row.clone()))
                .collect::<Vec<_>>(),
            now,
        );

        for row in &next.traders {
            if let Some(was) = self.shown.traders.iter().find(|old| {
                old.id == row.id && (old.profit != row.profit || old.trades != row.trades)
            }) {
                // Green unless the money actually went DOWN: a row whose trade count moved and
                // whose profit did not has not lost anything, and drawing it in the colour of a
                // loss would say it had.
                self.trader_moves
                    .beat(&row.id.to_string(), row.profit >= was.profit, now);
            }
        }
        self.trader_moves.settle(
            &next
                .traders
                .iter()
                .map(|row| (row.id.to_string(), row.clone()))
                .collect::<Vec<_>>(),
            now,
        );

        // Only a NEW board writes the marks. Another look at the same one leaves them exactly as
        // they stand, however many times we look.
        let marks = if seen.1 == self.boards_seen.1 {
            self.trader_marks.clone()
        } else {
            rank_marks(&self.shown.traders, &next.traders)
        };
        self.boards_seen = seen;
        marks
    }
}

impl CrowdStatsView {
    /// Open this coin's chart on Main, the way every other coin in the terminal opens.
    ///
    /// One market per core in this group that trades EXACTLY this coin: `coin_search` returns
    /// several markets per core (BTCUSDT/BTCUSDC) and contains-matches too, so the hits are
    /// filtered by the core's own label through the shared match key — the catalog token carries a
    /// contract tail (`AAVE_RP`, `SOL_0925`) where the crowd names the bare coin, and on
    /// Hyperliquid spot the market name is an index carrying no coin at all.
    ///
    /// One core opens it outright; several offer a picker naming the core and its exchange; none
    /// does nothing, which is the honest answer for a coin this terminal does not follow.
    ///
    /// Args:
    ///     coin: Ticker clicked.
    ///     pos: Where, so a picker can be anchored to it.
    ///     window: Owning window, used to host that picker.
    ///     cx: View context.
    fn open_coin(
        &mut self,
        coin: &str,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (rows, exchanges) = {
            let backend = self.backend.read(cx);
            let wanted = moon_core::symbol::coin_match_key(coin);
            let mut seen = std::collections::HashSet::new();
            let rows: Vec<(moon_core::session::CoreId, String, String)> =
                coin_search::search(backend, &self.group, None, coin)
                    .into_iter()
                    .filter(|hit| hit.label.match_key() == wanted)
                    .filter(|hit| seen.insert(hit.core))
                    .map(|hit| (hit.core, hit.market, hit.server))
                    .collect();
            // Exchange labels are only needed to tell two cores apart in the picker.
            let exchanges = if rows.len() > 1 {
                backend.session.core_venues().clone()
            } else {
                std::collections::HashMap::new()
            };
            (rows, exchanges)
        };
        match rows.len() {
            0 => {}
            1 => {
                let (core, market, _) = rows.into_iter().next().expect("one row");
                let group = self.group.clone();
                self.backend.update(cx, |backend, backend_cx| {
                    // `false`: open without stealing focus, matching every other coin-nav site.
                    if backend.open_on_main_if_authorized(Some(&group), (core, market), false) {
                        backend_cx.notify();
                    }
                });
            }
            _ => {
                let items: Vec<moon_ui::MoonMenuItem> = rows
                    .into_iter()
                    .map(|(core, market, name)| {
                        // Only a NAMEABLE venue earns a suffix: the row is there to tell two cores
                        // apart, and " · not identified" tells them apart from nothing.
                        let label = match exchanges.get(&core).filter(|venue| venue.is_nameable()) {
                            Some(venue) => {
                                format!("{name} · {}", crate::controls::venue_label(venue))
                            }
                            None => name,
                        };
                        let backend = self.backend.clone();
                        let group = self.group.clone();
                        moon_ui::MoonMenuItem::with_key(format!("crowd-coin-core-{core}"), label)
                            .on_click(move |_, window, app| {
                                window.close_context_menu(app);
                                backend.update(app, |backend, backend_cx| {
                                    if backend.open_on_main_if_authorized(
                                        Some(&group),
                                        (core, market.clone()),
                                        false,
                                    ) {
                                        backend_cx.notify();
                                    }
                                });
                            })
                    })
                    .collect();
                window.open_moon_context_menu(cx, "crowd-coin-cores", pos, items, PICKER_WIDTH);
            }
        }
    }
}

/// How wide the cores picker is, in pixels — the same width the news feed's picker uses.
const PICKER_WIDTH: f32 = 240.0;

/// What each trader's rank did between two boards, by account.
///
/// A CLIMB is positive: the rank number falls as a row goes up, and a mark that said "-3" for
/// climbing three would be read as a loss by everybody who reads the money beside it.
///
/// Counted off the SERVICE's rank rather than off the row's place on screen. The two are not the
/// same thing: a row the parser drops moves everything under it up the screen without moving
/// anybody's rank, and a mark counted that way would contradict the number printed beside it.
///
/// A row that was not on the board before gets nothing — arriving is its own event, and a mark
/// saying it climbed from somewhere it never was would be an invention.
///
/// Args:
///     was: The board that is on screen.
///     next: The board that has just arrived.
fn rank_marks(was: &[Trader], next: &[Trader]) -> HashMap<u64, i32> {
    next.iter()
        .map(|row| {
            let before = was
                .iter()
                .find(|old| old.id == row.id)
                .map_or(row.place, |old| old.place);
            // Held in i64 through the subtraction and then clamped: both places come off the
            // wire, and a pair of them that wrapped would print a climb where there was a fall.
            let shift = (i64::from(before) - i64::from(row.place))
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX));
            (row.id, shift as i32)
        })
        .collect()
}

/// A seed for the offline stand-in. Nothing depends on it being unpredictable; it exists so two
/// bench runs are not bit-identical in what the tables happen to show.
fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0x5EED)
}

impl Render for CrowdStatsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        diag::bump(&diag::CROWD_RENDER);
        let _render_us = diag::scope(&diag::CROWD_RENDER_US);
        // One moment for the whole frame: three tables read at three instants would be three
        // snapshots of the same movement, which is a picture of nothing.
        let now = Instant::now();
        let look = Look::of(cx);
        let view = cx.entity();
        // One listener per coin, built here because this is the only place that knows how to open
        // a chart; the tables stay free of any context.
        let on_coin = move |coin: &str| -> table::CoinAction {
            let coin = coin.to_string();
            let view = view.clone();
            Box::new(
                move |event: &gpui::MouseDownEvent, window: &mut Window, app| {
                    let at = event.position;
                    view.update(app, |this, cx| this.open_coin(&coin, at, window, cx));
                },
            )
        };
        div()
            .size_full()
            .relative()
            // Figures in columns, compared down the page: the same family every other data surface
            // in the terminal uses, and the reason the money lines up at all.
            .font_family(design::mono())
            .children(self.parts.minute.then(|| {
                table::minute(
                    &self.shown.rows,
                    self.shown.wire.unwrap_or(Wire::Opening),
                    Some((&self.minute_moves, now)),
                    Some(&on_coin),
                    &look,
                )
            }))
            .children(self.parts.coins.then(|| {
                table::day_coins(
                    &self.shown.coins,
                    Some((&self.coin_moves, now)),
                    Some(&on_coin),
                    &look,
                )
            }))
            .children(self.parts.traders.then(|| {
                table::day_traders(
                    &self.shown.traders,
                    Some((&self.trader_moves, now)),
                    &self.trader_marks,
                    self.shown.summary,
                    &look,
                )
            }))
    }
}

#[cfg(test)]
mod tests;
