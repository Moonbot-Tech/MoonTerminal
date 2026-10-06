//! The background threads that fetch trade replays, and the request protocol reaching them.
//!
//! # One thread per HOST and INTENT, and why not one per request
//!
//! Not for throughput — for RESTRAINT. A user walking down a report opens rows faster than any of
//! these endpoints wants to be asked, and a thread per request would turn that into a burst that
//! spends the user's IP budget. Every call to one host goes through that host's LANE threads,
//! and the floor between two calls to a host ([`super::gate::ReplayGate::pace`]) is kept per
//! host across them, so the budget stays one budget however many lanes share it. The hosts are
//! independent budgets — Binance, Gate, OKX and BitGet each meter their own address — so their
//! lanes walk at the same time: a batch of the tuner's rows spread over four venues finishes in
//! the time of its slowest venue, not of their sum. A host has one lane per
//! [`ReplayIntent`]: a chart window ([`ReplayIntent::Chart`]) never queues behind the tuner's
//! batch ([`ReplayIntent::Model`]) — a walk of the batch takes a minute or three, and a window
//! opened meanwhile would show "loading" for exactly that long; the two lanes pace each other
//! through the gate instead, and the batch slows down while the window loads. One COORDINATOR
//! thread owns the queue and everything that is not a venue call — the held-tape reads, the
//! core captures, the native follow-ups — so those answer while every lane is busy.
//! `moon-core` has no async runtime, so these are plain blocking threads and `mpsc` pairs,
//! exactly like the kline cache and the report valuation worker beside them.
//!
//! # Four caches, and each is load-bearing
//!
//! Bars go into the SHARED `klines.sqlite` under the real exchange key, because a one-minute bar
//! fetched here is indistinguishable from one the recorder wrote and the rest of the application
//! benefits from it. Whole OUTCOMES additionally go into a small in-memory ring owned by this
//! worker, keyed by the exact question asked. That second cache is what satisfies "the second
//! open of the SAME trade costs nothing": the kline cache cannot hold ticks at all, and nothing
//! else in the process remembers that a given window was already answered. The third is the tick
//! TILE store ([`super::tick_tiles`]), keyed by exchange and market and answering by COVERAGE
//! the way the bars are: it is what makes a NEIGHBOURING trade on the same market cost only the
//! stretch of its focus no earlier window fetched — and nothing at all when there is none. The
//! fourth is that store's disk, `trades.sqlite` ([`super::trade_cache`]): the tick stage hydrates
//! the tiles from it before deciding what to fetch and writes every harvest through, so the
//! same question survives a restart — behind a switch in the Storage tab.
//!
//! # The degrade ladder
//!
//! A tick stage never throws away what it already paid for. Cancellation is the one thing that
//! discards everything collected so far, because the window itself is gone; every other stop —
//! the page or tick budget, the job deadline, a venue's own refusal — instead SERVES what was
//! already fetched and names the reason in [`TradeReplaySeries::tick_status`], rather than
//! abandoning the whole stage and falling back to bars with no explanation. Only a harvest that
//! ends up genuinely empty reaches the candles-only outcome, and even then the bar layer drawn is
//! never blank: [`TickStage::candles`] carries the exchange's own one-minute klines forward from
//! the candle stage that ran first, so the window always has SOMETHING to show while the reasoned
//! caption explains what is missing and why.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::gate::ReplayGate;
use super::tick_tiles::{TickTileStore, TileKey, TileSource, residual_plan};
use super::venue_caps::{TradeRoute, bybit_category, kline_route, trade_route};
use super::{
    Coverage, ReplayIntent, ReplayWindow, TickPlan, TickStatus, TradeReplayEmpty,
    TradeReplayFailure, TradeReplayOutcome, TradeReplaySeries, TradeReplaySource, fit_ticks, pages,
    rest, tick_plan,
};
use crate::feed::types::Tick;
use crate::market::candles::ChartCandle;
use crate::market::kline_cache::{KlineCache, MergeItem};
use crate::market::source::{CoreReplayTicks, ReplayAddress};

use super::{cache_covers, gate, trade_cache, venue_caps};
#[cfg(doc)]
use super::{long_position_ms, margin_ms};

mod capture;
mod held;
mod ingress;
mod outcome;
mod paging;
mod serve;
mod ticks;
mod types;

pub use ingress::{capture, forget_tiles, query_held, request};
pub(crate) use outcome::rows_for_cache;
pub(crate) use paging::paginate_ticks;
pub use serve::inside_retention;
pub use types::{CaptureRequest, TickAnswer, TickQuery, TradeReplayRequest};
pub(crate) use types::{
    Job, NativeWait, TICK_BUDGET, TickAbandon, TickHarvest, TickObserver, TickStage, TickVerdict,
};

use capture::*;
use held::*;
use outcome::*;
#[cfg(test)]
use paging::walked_part;
use serve::*;
use ticks::*;
use types::*;

/// What every lane shares with the coordinator and with each other. The gate and the two
/// stores were built for one thread and are already behind their own locks; nothing here is
/// thread-affine.
struct Shared {
    agent: ureq::Agent,
    gate: ReplayGate,
    cache: Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    tiles: Mutex<TickTileStore>,
    /// Back to the coordinator, for the native follow-ups a lane arms.
    back: Sender<Inbound>,
}

/// Handle to the process-wide replay coordinator.
struct Worker {
    tx: Sender<Inbound>,
}

/// The one coordinator, started on the first request and never stopped; its lanes start on the
/// first request to each host.
static WORKER: OnceLock<Worker> = OnceLock::new();

#[cfg(test)]
mod tests;
