//! The crowd's statistics on an empty Main: three tables and nothing else.
//!
//! What the public statistics service publishes about everybody who agreed to be counted — every
//! closed trade as it happens, plus its own coin and trader boards for the rolling day. The
//! figures and the wire live in [`moon_core::crowd`]; this module only draws them.
//!
//! It is cheap by construction, not by tuning, and the three claims below are each readable in
//! `logs/render_diag.log` (`crowd_tick`, `crowd_render`, `crowd_render_us`):
//!
//! * **it is woken, never polled.** The counting is the service's, once a second for the whole
//!   terminal (`crowd_tick`), and it wakes this view only when the minute actually changed, a
//!   board was replaced, or the wire found a new word for itself. A quiet market wakes nobody.
//! * **a repaint only when the tables change.** Woken, the view re-derives the rows it would draw
//!   and compares them with the rows on screen; it notifies only if they differ. So a market that
//!   moved a coin nothing shows costs one comparison and no frame at all.
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
//! It COUNTS nothing itself. The wire, the rolling minute and the boards belong to
//! [`service::CrowdService`], one per terminal, and this view holds a lease naming the halves its
//! tables need: dropping the view releases the claim, and the service closes whatever nobody is
//! asking for any more. That is what lets two windows show the same minute, and what lets the rule
//! go on counting while every screen is covered by a chart.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, IntoElement, ParentElement, Pixels, Point, Render, Styled, Window, div,
};
use moon_core::crowd::board::{CoinDay, DaySummary, Trader};
use moon_core::crowd::{Standing, Wants, Wire};

use crate::Backend;
use crate::controls::coin_open;
use crate::design;
use crate::diag;
use service::{CrowdLease, CrowdService};
use table::Look;
use table::motion::Motion;

pub(crate) mod service;
mod table;

/// The fastest the boards are redrawn while something on them is moving.
///
/// The view's OWN clock, not the monitor's, and it is the whole answer to "what does this cost
/// when the market goes mad": the rate has nothing to do with how many trades arrived.
///
/// Twelve a second — enough that a four-hundred-millisecond slide is five steps rather than a
/// jump, and one tenth of what a screen full of tables would ask for at display rate.
const FRAME: Duration = Duration::from_millis(80);

/// Which of the three tables this view is showing.
///
/// It decides both halves of the cost: which tables are built, and which halves of the service
/// this screen CLAIMS. The minute is the live trade socket; the two day boards share one REST
/// poller, which fetches only the boards that are shown. A change is passed to the view rather
/// than rebuilding it — the claim is amended in place, and the service closes only what nobody
/// else is asking for.
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
    /// Changed in place by [`Self::show`] rather than by building a new view: the movement between
    /// two boards, and the lease this view holds on the service, are both worth keeping across a
    /// change of mind about which tables to draw.
    parts: CrowdParts,
    /// The terminal's one reader of the statistics service. See [`service`].
    service: Entity<CrowdService>,
    /// This view's claim on it, naming the halves its tables need. Dropping the view drops the
    /// claim, which is the whole of the teardown.
    lease: CrowdLease,
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
    shown: Shown,
}

impl CrowdStatsView {
    /// Open the tables that were asked for and claim what they read.
    ///
    /// Args:
    ///     parts: Which tables to show — and therefore which halves of the service to claim.
    ///     backend: The terminal, for the service and for opening a coin's chart.
    ///     group: The group window this screen belongs to.
    ///     cx: The view's context.
    pub(crate) fn new(
        parts: CrowdParts,
        backend: Entity<Backend>,
        group: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let service = backend.read(cx).crowd();
        let lease = service.update(cx, |service, service_cx| {
            service.lease(parts.wants(), service_cx)
        });
        // Woken by the service and only when it has something new to say: a quiet market wakes
        // nobody, and none of this goes through the backend's seventeen-view notification.
        cx.observe(&service, |this, _service, cx| {
            // A covered screen derives nothing: the rows, the sort and the clones exist only to be
            // drawn and compared with what is drawn. The reading goes on regardless — it is the
            // service's — so coming back is one re-derivation, not a minute of refilling.
            if this.drawn && this.refresh(true, cx) {
                cx.notify();
            }
        })
        .detach();
        let mut this = Self {
            parts,
            backend,
            group,
            service,
            lease,
            drawn: true,
            minute_moves: Motion::default(),
            coin_moves: Motion::default(),
            trader_moves: Motion::default(),
            boards_seen: (0, 0),
            trader_marks: HashMap::new(),
            drawing: false,
            shown: Shown {
                rows: Vec::new(),
                wire: None,
                coins: Vec::new(),
                traders: Vec::new(),
                summary: None,
            },
        };
        // The tables open on what has ALREADY been counted: a screen shown over a service that has
        // been reading for the rule starts on a full minute rather than on an empty one filling
        // from scratch.
        this.refresh(false, cx);
        this
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
            // Coming back is not MOVEMENT. Nothing was derived while the screen was covered, so the
            // boards on the other side of that gap are a minute of history — replayed as slides and
            // rank marks they would animate everything that happened while nobody was watching. The
            // screen therefore comes back the way it opens: settled, with what is true now.
            self.shown = Shown {
                rows: Vec::new(),
                wire: None,
                coins: Vec::new(),
                traders: Vec::new(),
                summary: None,
            };
            self.minute_moves = Motion::default();
            self.coin_moves = Motion::default();
            self.trader_moves = Motion::default();
            self.trader_marks.clear();
            self.refresh(false, cx);
            cx.notify();
        }
    }

    /// Show exactly these tables from now on, claiming or releasing what they read.
    ///
    /// Args:
    ///     parts: The new set.
    ///     cx: The view's context, for the repaint.
    pub(crate) fn show(&mut self, parts: CrowdParts, cx: &mut Context<Self>) {
        if self.parts == parts {
            return;
        }
        self.parts = parts;
        let lease = self.lease.clone();
        self.service.update(cx, |service, service_cx| {
            service.relet(&lease, parts.wants(), service_cx)
        });
        // A table that has just been switched OFF must not go on moving: settled against an empty
        // board it would mark every row as leaving, and the frame chain would then draw ten frames
        // of a farewell nobody is watching. The MINUTE itself is not cleared — it is not ours, and
        // the rule may still be counting it.
        if !parts.minute {
            self.minute_moves = Motion::default();
        }
        if !parts.coins {
            self.coin_moves = Motion::default();
        }
        if !parts.traders {
            self.trader_moves = Motion::default();
            self.trader_marks.clear();
        }
        // A table that has just been switched on shows what has already been counted, now, rather
        // than waiting for the next thing to change; the one that was already there must not blink,
        // and does not, because only the SET changed and the rows are read from the same place.
        self.refresh(false, cx);
        cx.notify();
    }

    /// Take what the service has counted, and say whether the tables moved.
    ///
    /// The invalidation model in one function: only what is DRAWN is derived, only what is drawn
    /// is compared, and a table that is switched off can neither move nor cause a repaint.
    ///
    /// Args:
    ///     arrivals: Whether the trades the service drained on its last tick should light up the
    ///         rows they landed on. False for a refresh caused by a change of SET rather than by
    ///         the market — switching a table on is not an arrival.
    ///     cx: The view's context.
    ///
    /// Returns:
    ///     Whether anything drawn differs from what is on screen.
    fn refresh(&mut self, arrivals: bool, cx: &mut Context<Self>) -> bool {
        let now = Instant::now();
        let (next, landed, seen) = {
            let service = self.service.read(cx);
            let next = Shown {
                rows: if self.parts.minute {
                    service.minute().standings(table::MINUTE_SEATS)
                } else {
                    Vec::new()
                },
                wire: self.parts.minute.then(|| service.wire()),
                // Only the rows that are DRAWN, for the same reason as the traders below.
                coins: if self.parts.coins {
                    service
                        .coins()
                        .iter()
                        .take(table::DAY_COIN_SEATS)
                        .cloned()
                        .collect()
                } else {
                    Vec::new()
                },
                // Only the rows that are DRAWN. The service sends fifty and twenty are shown, so
                // keeping all of them here meant a change in row forty-two failed the comparison
                // and repainted a screen on which nothing had moved.
                traders: if self.parts.traders {
                    service
                        .traders()
                        .iter()
                        .take(table::TRADERS_SHOWN)
                        .cloned()
                        .collect()
                } else {
                    Vec::new()
                },
                summary: self.parts.traders.then(|| service.summary()).flatten(),
            };
            let landed = if arrivals {
                service.landed().clone()
            } else {
                HashMap::new()
            };
            (next, landed, service.boards_seen())
        };
        // What is new since the last board, before the boards are swapped.
        let marks = self.settle(&next, &landed, seen, now);
        // The marks are drawn but are not part of the board, so they have to be asked about
        // separately: a new board that moved nobody rubs every mark out while leaving all twenty
        // rows exactly as they were, and without this the screen would go on showing them.
        let moved = next != self.shown || marks != self.trader_marks;
        self.shown = next;
        self.trader_marks = marks;
        // And if that left anything moving, the view draws itself at ITS rate until it stops.
        self.draw_while_moving(cx);
        moved
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
    ///     seen: How many times the service has replaced each of its two boards. Those two are the
    ///         SERVICE's, polled about once a minute: a new one is a new one, and another look at
    ///         the same one is not. The minute is OURS — every pass is a new board of it, because
    ///         we add it up ourselves.
    ///     now: The clock the glow and the farewell are measured on.
    ///
    /// Returns:
    ///     The rank marks the trader board should wear — the ones standing, or fresh ones if a new
    ///     board has just arrived.
    fn settle(
        &mut self,
        next: &Shown,
        landed: &HashMap<String, f64>,
        seen: (u64, u64),
        now: Instant,
    ) -> HashMap<u64, i32> {
        for (coin, net) in landed {
            // Only a coin that is ALREADY on the board: a row arriving is a movement of its own,
            // and lighting it up as well says one thing twice.
            if self.shown.rows.iter().any(|old| old.coin == *coin) {
                self.minute_moves.beat(coin, *net >= 0.0, now);
            }
        }
        // Each board settles on its own. Returning early for a screen without the minute — which
        // is what this used to do — left the two day boards unsettled, so their rows jumped
        // between places instead of sliding and no rank mark was ever computed for them.
        if self.parts.minute {
            self.minute_moves.settle(
                &next
                    .rows
                    .iter()
                    .map(|row| (row.coin.clone(), row.clone()))
                    .collect::<Vec<_>>(),
                now,
            );
        }

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
    /// The resolution and the picker are [`coin_open`]'s, shared with the card the crowd's rule
    /// fires: both surfaces name a coin and no core, and asking that question twice is how the two
    /// would come to disagree about which chart a coin means.
    ///
    /// Args:
    ///     coin: Ticker clicked.
    ///     pos: Where, so a picker can be anchored to it.
    ///     window: Owning window, used to host the picker.
    ///     cx: View context.
    fn open_coin(
        &mut self,
        coin: &str,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Nothing to do afterwards: a table row is not consumed by being clicked.
        coin_open::open(&self.backend, &self.group, coin, pos, window, cx, |_| {});
    }
}

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
