//! The crowd's trades and public boards, off the statistics service.
//!
//! Two background threads and one channel. Nothing here runs on the frame path — a reader drains
//! whatever arrived since its last tick, exactly as it drains the synthetic stand-in, so a table
//! cannot tell the difference and no window can be blocked by a socket.
//!
//! The figures themselves live beside this module and know nothing about a wire. This one is the
//! wire and only the wire: it opens the stream, polls the two boards, and hands over
//! [`super::Trade`] values and whole board snapshots.
//!
//! Nothing identifying goes out and nothing identifying comes back. These are the public pages of
//! `stat.moonbot.pro`, read exactly as a browser reads them, and the terminal sends no key, no
//! account and no core name with either request.

mod rest;
mod sockjs;

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use super::board::{CoinDay, DaySummary, Trader};
use super::{EventSource, SyntheticFeed, Trade};

/// Where a reader's trades come from.
///
/// Every reader needs one and none of them cares which: a table drains trades and never asks
/// whether a socket or a seed produced them. One enum rather than one per reader, so "what does a
/// feed do when it is down" is answered once.
pub enum Feed {
    /// The real crowd.
    Live(LiveFeed),
    /// Deterministic stand-in: no network, replayable from a seed.
    Synthetic(SyntheticFeed),
}

impl Feed {
    /// Open the live feed, or a seeded stand-in when there is no config to open it with.
    ///
    /// Args:
    ///     config: The live feed's, or `None` for the stand-in.
    ///     seed: What the stand-in replays from.
    ///     wants: Which halves of the service to read. A half nobody is showing is a half nobody
    ///         reads: the thread for it is never started, so switching a table off closes its
    ///         socket rather than merely hiding what it delivers.
    pub fn open(config: Option<FeedConfig>, seed: u64, wants: Wants) -> Self {
        match config {
            Some(config) => Feed::Live(LiveFeed::start(config, wants)),
            None => Feed::Synthetic(SyntheticFeed::new(seed)),
        }
    }

    /// What can honestly be said about this feed right now.
    ///
    /// A stand-in is never "connected": there is no wire to be connected to, and saying otherwise
    /// would put a green word on a screen showing invented numbers.
    pub fn wire(&self) -> Wire {
        match self {
            Feed::Live(feed) => feed.wire(),
            Feed::Synthetic(_) => Wire::Synthetic,
        }
    }

    /// The service's coin board for the day. Empty on the stand-in, which has no service behind
    /// it — and an empty board says so on screen rather than inventing rows.
    ///
    /// A snapshot rather than a stream: what arrives replaces what was there.
    pub fn coins(&self) -> &[CoinDay] {
        match self {
            Feed::Live(feed) => feed.coins(),
            Feed::Synthetic(_) => &[],
        }
    }

    /// Its trader board, likewise.
    pub fn traders(&self) -> &[Trader] {
        match self {
            Feed::Live(feed) => feed.traders(),
            Feed::Synthetic(_) => &[],
        }
    }

    /// The day's own totals, or `None` on a stand-in, which has no service to total anything.
    pub fn summary(&self) -> Option<DaySummary> {
        match self {
            Feed::Live(feed) => feed.summary(),
            Feed::Synthetic(_) => None,
        }
    }

    /// Read exactly these halves from now on.
    ///
    /// A stand-in has nothing to open or close: it invents whatever it is asked for.
    ///
    /// Args:
    ///     wants: Which halves to read.
    pub fn want(&mut self, wants: Wants) {
        if let Feed::Live(feed) = self {
            feed.want(wants);
        }
    }

    /// How many times each board has been replaced since this feed was opened.
    ///
    /// What a reader does with it is tell a NEW board from another look at the same one: these
    /// are re-read about once a minute and looked at once a second, so "has it changed" cannot be
    /// answered by comparing rows — two different boards can hold the same twenty rows.
    pub fn boards_seen(&self) -> (u64, u64) {
        match self {
            Feed::Live(feed) => feed.boards_seen(),
            Feed::Synthetic(_) => (0, 0),
        }
    }
}

/// Which halves of the service a reader wants.
///
/// The two are read by different threads over different protocols on different clocks — a socket
/// held open for the trades, a pair of GETs on a minute's timer for the boards — so wanting one
/// and not the other is not a filter, it is one fewer thread and one fewer connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wants {
    /// The live trade stream, which the rolling minute is built from.
    pub trades: bool,
    /// The service's coin board for the day.
    pub coins: bool,
    /// Its trader board, and the day total that is printed beside it.
    pub traders: bool,
}

impl Wants {
    /// Whether either day board is wanted — which is what decides the polling thread.
    ///
    /// One thread for the two of them, because they are two GETs on one timer; WHICH of them it
    /// fetches is the thread's own business, and it fetches only what is shown.
    pub fn boards(self) -> bool {
        self.coins || self.traders
    }
}

/// What a reader can honestly say about the wire.
///
/// Derived in one place because it was derived in three, from the same two booleans, and one of
/// them was allowed to disagree with the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    /// Invented numbers; there is no wire to be up or down.
    Synthetic,
    /// Opening: the socket has been asked for and has not answered yet.
    ///
    /// Its own state because the handshake is three round trips over a long-polling transport, and
    /// a screen that said "no connection" for the whole of it would be describing a wire nobody
    /// had given a chance to answer.
    Opening,
    /// Connected.
    Live,
    /// Meant to be connected and is not.
    Down,
}

impl EventSource for Feed {
    fn drain(&mut self, now_ms: u64) -> Vec<Trade> {
        match self {
            Feed::Live(feed) => feed.drain(now_ms),
            Feed::Synthetic(feed) => feed.drain(now_ms),
        }
    }
}

/// How long the trade stream may say nothing at all before it is no longer called live.
///
/// The server holds a quiet poll for about twenty-five seconds and then sends a heartbeat, so
/// three times that is silence with no innocent explanation left.
const BEAT_STALE: Duration = Duration::from_secs(90);

/// How long a reader thread may sleep before it looks again at whether anybody still wants it.
///
/// Both threads wait in steps of this rather than in one long sleep: a screen that is closed while
/// a poller is a minute into its wait would otherwise leave a thread and a socket behind for the
/// rest of that minute, and toggling the screen would stack them.
pub(super) const ALIVE_STEP: Duration = Duration::from_secs(1);

/// Wait, in steps, for as long as the reader is still there.
///
/// Args:
///     alive: Handle to the reader; `None` once it has gone.
///     span: How long to wait in total.
///
/// Returns:
///     Whether the reader is still there, and the caller should carry on.
pub(super) fn waited(alive: &Weak<()>, span: Duration) -> bool {
    let until = Instant::now() + span;
    loop {
        if alive.upgrade().is_none() {
            return false;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(ALIVE_STEP));
    }
}

/// Origin of the statistics service, and the only address this module ever talks to.
///
/// One host, no fallback and no second address anywhere in the tree. A fallback is worth having
/// when the two candidates answer the same question the same way; these would not — a second
/// origin computes its own boards off its own datagram losses, so a reader that quietly changed
/// host would show a different day's figures under the same heading and nothing on screen would
/// say which it was looking at.
pub const STAT_ORIGIN: &str = "https://stat.moonbot.pro";

/// Where to read from.
///
/// No identity of any kind goes with it: what is read here is the CROWD, whose trades this service
/// publishes, and the terminal has nothing to add to that request. A request carrying a key or a
/// core name would be telling the service who is watching, which is not part of reading a public
/// page.
#[derive(Clone, Debug)]
pub struct FeedConfig {
    /// Origin of the statistics service; [`STAT_ORIGIN`] unless a caller says otherwise.
    pub base: String,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            base: STAT_ORIGIN.to_string(),
        }
    }
}

/// What a background thread sends to the reader.
/// Every event carries the GENERATION of the half that produced it.
///
/// A thread is asked to stop by dropping the handle it watches, and it notices between requests —
/// which for the long poll can be most of a minute. In that window a screen can switch the table
/// back on and be given a fresh thread, and then two sockets are delivering the same trades into
/// one rolling minute, which would count every one of them twice. The generation makes that
/// unrepresentable: a reader takes only what its CURRENT thread sends, and whatever the outgoing
/// one still has to say is dropped at the door.
pub enum FeedEvent {
    Trade(u64, Trade),
    /// The service's own coin board for the rolling day, whole. It is a snapshot, not a stream:
    /// what arrives replaces what was there.
    Coins(u64, Vec<CoinDay>),
    /// Its trader board, likewise.
    Traders(u64, Vec<Trader>),
    /// The day in two numbers, everybody included.
    Summary(u64, DaySummary),
    /// Connection state of the trade stream, for the one honest word a screen says about it.
    Connected(u64, bool),
}

/// The live crowd feed as an [`EventSource`].
pub struct LiveFeed {
    rx: Receiver<FeedEvent>,
    /// Where the threads read from, kept so a half can be started later.
    config: Arc<FeedConfig>,
    /// The sender each new thread is given a clone of.
    tx: Sender<FeedEvent>,
    /// One liveness token per half, held while that half is wanted.
    ///
    /// Its thread holds a [`Weak`] of it and stops within [`ALIVE_STEP`] of it being dropped — or
    /// after whatever request is in flight. Two tokens rather than one because the halves are
    /// switched on and off independently: dropping the boards' token must not touch the socket.
    ///
    /// Never read: EXISTING is the whole of what a token does, and it does it in `Drop`. Without
    /// them the only way a thread learns it has been abandoned is a failed SEND — and a thread
    /// polling a service that is down has nothing to send, so it would poll for the life of the
    /// process.
    trades_alive: Option<Arc<()>>,
    boards_alive: Option<Arc<()>>,
    /// Which generation of each half is the current one. See [`FeedEvent`].
    trades_epoch: u64,
    boards_epoch: u64,
    /// What the polling thread was started FOR, so a change of which boards are shown restarts it.
    boards_want: Wants,
    connected: bool,
    /// Whether the trade thread has ever said anything, so a wire that has not answered YET can be
    /// told from one that has answered and failed.
    spoke: bool,
    /// When the trade thread last said anything at all — a trade, or a heartbeat.
    last_beat: Instant,
    /// The last coin board the service sent, and the last trader board.
    ///
    /// Held rather than passed through because they are SNAPSHOTS on a minute's cadence, not a
    /// stream: a reader that took only what arrived in the last tick would be empty for the
    /// fifty-six seconds between them.
    coins: Vec<CoinDay>,
    traders: Vec<Trader>,
    /// The day's own totals, as the service last reported them.
    summary: Option<DaySummary>,
    /// How many times each board has been REPLACED.
    ///
    /// Not how many rows it has and not when it arrived: what a reader needs from these is "is
    /// this the same board I drew last time", and the boards are polled on their own clock — a
    /// minute apart — while the reader looks at them every second.
    coins_seen: u64,
    traders_seen: u64,
}

impl LiveFeed {
    /// Start the reader threads that were asked for, and return the handle a screen drains.
    ///
    /// Each thread owns its own retry policy and never panics outwards: a feed that is down is an
    /// empty table, which every reader here already treats as a legitimate state. Both end on
    /// their own once this handle is dropped — within [`ALIVE_STEP`] if they are waiting, or after
    /// whatever request is in flight, which for the long poll can be most of a minute.
    ///
    /// Args:
    ///     config: Where to read from.
    ///     wants: Which halves to read; a half nobody wants gets no thread at all.
    pub fn start(config: FeedConfig, wants: Wants) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut feed = Self {
            rx,
            config: Arc::new(config),
            tx,
            trades_alive: None,
            boards_alive: None,
            trades_epoch: 0,
            boards_epoch: 0,
            boards_want: Wants {
                trades: false,
                coins: false,
                traders: false,
            },
            connected: false,
            spoke: false,
            last_beat: Instant::now(),
            coins: Vec::new(),
            traders: Vec::new(),
            summary: None,
            coins_seen: 0,
            traders_seen: 0,
        };
        feed.want(wants);
        feed
    }

    /// Read exactly these halves from now on, starting and stopping threads to match.
    ///
    /// The point of it being a change rather than a new feed: a screen that switches a table on
    /// keeps the socket it already had, the boards it already holds and the minute it has already
    /// counted. Rebuilding instead emptied all three and made the whole screen start over, which
    /// is what a person sees as "everything redrew".
    ///
    /// A half that is already running is left alone. A half that is switched off drops its token,
    /// and its thread ends on its own; the data it delivered is KEPT, because switching a table
    /// back on should show what was last true rather than a blank waiting for the next poll.
    ///
    /// Args:
    ///     wants: Which halves to read.
    pub fn want(&mut self, wants: Wants) {
        if wants.trades && self.trades_alive.is_none() {
            let alive = Arc::new(());
            let watch = Arc::downgrade(&alive);
            let config = Arc::clone(&self.config);
            let tx = self.tx.clone();
            // A new generation, so whatever the last thread still says is no longer ours.
            self.trades_epoch += 1;
            let epoch = self.trades_epoch;
            // A thread that cannot be started is a feed that is down, which every screen here
            // already knows how to say. It is NOT worth a panic: this runs on the frame path, and
            // taking the whole terminal down over a decorative table would be the worse failure.
            let started = std::thread::Builder::new()
                .name("crowd-trades".into())
                .spawn(move || sockjs::run(&config, &tx, &watch, epoch));
            self.trades_alive = started.ok().map(|_| alive);
            // A thread that could not be started has already failed; one that has just started has
            // not said anything YET, and those are different words on the screen.
            self.spoke = self.trades_alive.is_none();
            self.connected = false;
        } else if !wants.trades && self.trades_alive.is_some() {
            self.trades_alive = None;
            // A new generation on the way OUT as well as on the way in. The thread notices it has
            // been abandoned between requests, which for the long poll is most of a minute, and
            // everything it says in the meantime belongs to a reader that has moved on — a reader
            // that, now the feed outlives any one screen, is still there to be told.
            self.trades_epoch += 1;
            self.connected = false;
            self.spoke = false;
        }
        // A second thread, because the two are different animals: the trades are a socket held
        // open for as long as the reader wants it, and the boards are a GET on a minute's timer.
        // One thread doing both would have to stop reading the socket to poll.
        // The polling thread is rebuilt when WHICH BOARDS are shown changes, not only when the
        // pair goes on or off: it fetches exactly what is drawn, and that is decided when it
        // starts. Only the two board flags are compared — the trade stream is switched on and off
        // independently of them, and comparing the whole request tore down a healthy poller every
        // time it was.
        let boards_changed =
            self.boards_want.coins != wants.coins || self.boards_want.traders != wants.traders;
        if wants.boards() && (self.boards_alive.is_none() || boards_changed) {
            let alive = Arc::new(());
            let watch = Arc::downgrade(&alive);
            let config = Arc::clone(&self.config);
            let tx = self.tx.clone();
            self.boards_epoch += 1;
            let epoch = self.boards_epoch;
            let started = std::thread::Builder::new()
                .name("crowd-boards".into())
                .spawn(move || rest::run(&config, &tx, &watch, wants, epoch));
            self.boards_alive = started.ok().map(|_| alive);
        } else if !wants.boards() && self.boards_alive.is_some() {
            self.boards_alive = None;
            self.boards_epoch += 1;
        }
        self.boards_want = wants;
    }

    /// The coin board as it last stood.
    pub fn coins(&self) -> &[CoinDay] {
        &self.coins
    }

    /// How many times each board has been replaced: the coins, then the traders.
    pub fn boards_seen(&self) -> (u64, u64) {
        (self.coins_seen, self.traders_seen)
    }

    /// The trader board as it last stood.
    pub fn traders(&self) -> &[Trader] {
        &self.traders
    }

    /// The day's own totals, or `None` before the first answer.
    pub fn summary(&self) -> Option<DaySummary> {
        self.summary
    }

    /// What can honestly be said about this feed right now.
    ///
    /// The connected flag alone is not enough, in either direction. A thread reports a break by
    /// sending `false`, so one that stopped without being able to say why — a session the server
    /// keeps open and stops delivering on — would leave the last `true` standing and the screen
    /// would go on calling a dead wire live; silence past [`BEAT_STALE`] is silence from the wire
    /// itself, since the server heart-beats a quiet poll about every twenty-five seconds. And
    /// before the first frame arrives there is nothing to be wrong yet, which is [`Wire::Opening`]
    /// rather than a failure.
    pub fn wire(&self) -> Wire {
        if self.connected {
            if self.last_beat.elapsed() < BEAT_STALE {
                return Wire::Live;
            }
            return Wire::Down;
        }
        if self.spoke {
            Wire::Down
        } else {
            Wire::Opening
        }
    }
}

impl EventSource for LiveFeed {
    /// Drain the channel.
    ///
    /// Arrivals are stamped with the READER's clock rather than with the exchange's close time:
    /// the rolling minute is a minute of watching, and a trade that closed forty seconds before
    /// the window opened would otherwise age out the moment it landed.
    fn drain(&mut self, now_ms: u64) -> Vec<Trade> {
        let now = Instant::now();
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                // A generation that is not the current one is a thread on its way out; what it
                // still has to say belonged to a screen that has moved on.
                Ok(FeedEvent::Trade(epoch, mut trade)) if epoch == self.trades_epoch => {
                    trade.at_ms = now_ms;
                    self.spoke = true;
                    self.last_beat = now;
                    out.push(trade);
                }
                Ok(FeedEvent::Connected(epoch, state)) if epoch == self.trades_epoch => {
                    self.connected = state;
                    self.spoke = true;
                    self.last_beat = now;
                }
                // Snapshots: what arrived replaces what was there, whole.
                Ok(FeedEvent::Coins(epoch, coins)) if epoch == self.boards_epoch => {
                    self.coins = coins;
                    self.coins_seen += 1;
                }
                Ok(FeedEvent::Traders(epoch, traders)) if epoch == self.boards_epoch => {
                    self.traders = traders;
                    self.traders_seen += 1;
                }
                Ok(FeedEvent::Summary(epoch, summary)) if epoch == self.boards_epoch => {
                    self.summary = Some(summary)
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                // The senders are gone: nothing will arrive again, and the honest thing to show
                // for that is the same "no connection" a broken socket shows.
                Err(TryRecvError::Disconnected) => {
                    self.connected = false;
                    break;
                }
            }
        }
        out
    }
}
