//! A dedicated window showing ONE closed trade in its market context.
//!
//! # What it answers
//!
//! Clicking a closed trade in the Report used to move the MAIN chart's viewport onto the trade's
//! interval and fetch nothing, so the entry and exit arrows regularly landed over empty space and
//! the user's own chart was dragged away from whatever they were watching. This window answers the
//! question that gesture was really asking — "show me this trade" — without touching the main
//! chart at all: it fetches the market around the trade from the exchange's public REST and draws
//! it here, beside the trade's own figures.
//!
//! # How the frozen data reaches the chart
//!
//! The window owns its own [`ChartPanel`], and therefore its own chart engine. That engine is
//! handed a `moon_core::market::trade_replay::TradeReplaySeries` and reads from it INSTEAD of the
//! live market source. Because the override lives on the engine rather than in any registry, a
//! replay is structurally incapable of reaching the user's main chart: that engine holds `None`
//! and there is no shared key the two could collide on.
//!
//! # Why a whole `ChartPanel` and not a bare engine
//!
//! The engine draws pixels; everything AROUND them — theme and settings application, scene
//! visibility, sizing, wheel and hover input, and crucially the userdata pass that turns closed
//! trades into the entry/exit arrows — lives in `ChartPanel`. Hosting a bare engine here would
//! mean either a window with no arrows and no crosshair, or a second copy of that machinery, which
//! is exactly the parallel mechanism this project forbids.
//!
//! # Window ownership and dismissal
//!
//! The root owns the focus handle because Escape is a window command, not a chart command: a bare
//! Escape closes this independent window even after the chart takes focus, while modified Escape
//! stays available to the chart's own hotkey layer. The header mounts the frame's close control as
//! the visible dismissal affordance; the key is the fallback for a taskbar-hidden window whose
//! chrome cannot be reached.

mod figures;
pub(crate) mod frame;
mod render;
#[cfg(test)]
mod tests;
mod window;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::Backend;
use crate::panels::chart::ChartPanel;
use gpui::*;
use moon_core::db::{ChartTradeRecord, TradeMeta};
use moon_core::market::trade_replay::worker::{self, TradeReplayRequest};
use moon_core::market::trade_replay::{
    TradeReplayEmpty, TradeReplayFailure, TradeReplayOutcome, TradeReplaySeries, TradeReplaySource,
    replay_window,
};
use moon_core::session::CoreId;

pub(crate) use window::open_trade_window;

/// What the window is currently able to show.
///
/// Every arm renders inside the SAME layout: the header and the figures rail never move, so a
/// state change reads as the chart area answering rather than as the window rebuilding itself.
#[derive(Clone, Debug)]
pub(crate) enum TradeWindowState {
    /// The fetch is in flight. The figures rail is already complete.
    Loading,
    /// Rows are on screen; the caption states which kind they are.
    ///
    /// `tf_min` is the timeframe of the rows ACTUALLY DRAWN, in minutes, carried here so the
    /// caption can state it instead of asserting one. It used to be a constant string reading
    /// "minute candles" while the panel resampled the minute rows into the user's global
    /// five-minute bucket — a wrong label over real-money data, which is worse than an honest gap.
    /// A typed number rather than a formatted string, because this value is produced beside
    /// `moon-core` data and only the UI can localize the sentence around it.
    Ready {
        source: TradeReplaySource,
        tf_min: u16,
    },
    /// Nothing to draw, for a reason the window names.
    Empty(TradeReplayEmpty),
    /// The fetch did not produce an answer.
    Failed(TradeReplayFailure),
}

impl TradeWindowState {
    /// Whether a Retry button can achieve anything from this state.
    ///
    /// A missing endpoint and a degenerate window are permanent facts about this trade, and
    /// offering a retry for them would be a button that is guaranteed to do nothing.
    ///
    /// Returns:
    ///     `true` when retrying may change the answer.
    fn retryable(&self) -> bool {
        match self {
            Self::Loading | Self::Ready { .. } => false,
            Self::Empty(empty) => matches!(
                empty,
                TradeReplayEmpty::NoDataInWindow | TradeReplayEmpty::CoreNotConnected
            ),
            Self::Failed(_) => true,
        }
    }
}

/// Fold one observed window rectangle into the geometry every trade window is restored from.
///
/// A free function, and a pure one, so the RULE can be exercised without a window — the same reason
/// [`render::rail_wraps`] is one. A test that only compared constants would stay green through an
/// inverted branch, and this branch is the difference between a remembered position and one that
/// walks off the screen.
///
/// # The cascade must not round-trip
///
/// Two trade windows may be open at once, and the second is opened at a small offset so it does not
/// land exactly on the first. That offset is a placement for THIS open, not something the user
/// chose. Persisting it would add another offset on the next open, and another after that: the
/// remembered position walks by one step per open, forever, and is discovered only once the window
/// has crawled off the display.
///
/// So a window that was cascaded persists its SIZE ONLY, keeping the origin already remembered.
/// Size never walks — the cascade moves the origin and nothing else — so there is no reason to
/// withhold it. The honest cost, stated rather than hidden: if the user closes the first window and
/// then drags the second, that drag does not redefine the remembered origin. That is bounded to one
/// offset; the alternative is unbounded drift.
///
/// With nothing remembered yet there is no origin to KEEP, so the cascade is SUBTRACTED back out
/// instead. Taking the observed rectangle whole there would seed the shared memory one step away
/// from where the window would have opened, and every later window would inherit that. Undoing a
/// known offset is exact, not a guess: it is the same number this window was opened with.
///
/// Args:
///     previous: Geometry remembered so far, if any.
///     observed: The rectangle this window currently occupies.
///     cascade_px: Offset this window was opened with; `0.0` for an uncascaded window.
///
/// Returns:
///     The geometry to remember.
pub(super) fn remembered_geometry(
    previous: Option<moon_core::config::layout::GeomRect>,
    observed: moon_core::config::layout::GeomRect,
    cascade_px: f32,
) -> moon_core::config::layout::GeomRect {
    let mut next = observed.keeping_display_of(previous);
    if cascade_px != 0.0 {
        match previous {
            // The display travels with the origin: keeping one and not the other would remember a
            // point on a monitor it was never measured against.
            Some(previous) => {
                next.x = previous.x;
                next.y = previous.y;
                next.display_uuid = previous.display_uuid;
            }
            // Saturating, because the offset is applied to a signed coordinate that a window
            // dragged to the far edge of a large desktop can already have near the type's bound.
            None => {
                let step = cascade_px as i32;
                next.x = next.x.saturating_sub(step);
                next.y = next.y.saturating_sub(step);
            }
        }
    }
    next
}

/// Resolve one trade's metadata into the strings its chart captions print.
///
/// The STRATEGY is the half that has to happen up here: the replica stores a Delphi-signed id, and
/// the name behind it lives in the session's strategy store — the same lookup the Report row menu
/// makes, so a trade names its strategy identically in both places. A core the terminal is no
/// longer connected to has no name to give, and the id is printed instead: a number the reader can
/// still match against their own strategy list beats an empty caption.
///
/// Args:
///     backend: Live state holding the session and its strategy store.
///     core: Core that recorded the trade.
///     meta: What the replica answered for it.
///
/// Returns:
///     The caption strings, and whether the strategy could actually be NAMED — `false` means the
///     number is standing in, and the window keeps asking until the core's strategy list arrives.
pub(super) fn trade_labels(
    backend: &Backend,
    core: CoreId,
    meta: &TradeMeta,
) -> (crate::chartdx::TradeLabels, bool) {
    let name = meta
        .strategy_id
        .and_then(|id| strategy_name(backend, core, id));
    // Named when there was nothing to name, too: a trade that carries no strategy at all is not
    // waiting for one to arrive.
    let named = meta.strategy_id.is_none() || name.is_some();
    let strategy = name
        // The signed number, because that is what the Report's own `strategyid` cell shows the
        // reader — a caption standing in for a name has to match the table the window opened from.
        .or_else(|| meta.strategy_id.map(|id| id.to_string()))
        .unwrap_or_default();
    (
        crate::chartdx::TradeLabels {
            strategy,
            detect: meta.detect.clone(),
            sell_reason: meta.sell_reason.clone(),
        },
        named,
    )
}

/// Name one strategy through the session's own store.
///
/// `as u64` because the store keys strategies by the same bits the wire carries, which is the
/// conversion every other id-bearing lookup in the terminal makes.
///
/// Args:
///     backend: Live state holding the session.
///     core: Core that owns the strategy.
///     id: Delphi-signed identity from the replica.
///
/// Returns:
///     The user's own name for it, or `None` while that core's list has not arrived.
fn strategy_name(backend: &Backend, core: CoreId, id: i64) -> Option<String> {
    crate::strategies::logic::row(backend.session.store(), core, id as u64).map(|s| s.name.clone())
}

/// One trade-detail window.
pub(crate) struct TradeWindowView {
    backend: Entity<Backend>,
    /// The chart this window draws, carrying the frozen replay.
    panel: Entity<ChartPanel>,
    /// The trade being shown; every figure comes from here and needs no network.
    record: ChartTradeRecord,
    /// Core and exchange-native market the trade was resolved to.
    core: CoreId,
    market: String,
    /// Entry and exit stamps, already formatted in the Report's own clock.
    ///
    /// Passed in rather than computed: the Report already owns the user's display zone, and a
    /// window that formatted its own would be a second clock free to disagree with the table the
    /// user clicked.
    stamps: (String, String),
    state: TradeWindowState,
    /// Monotonic dispatch counter, so a Retry supersedes an in-flight fetch instead of racing it.
    sequence: u64,
    /// What the replica said this trade carried, kept so the strategy can be named LATER.
    ///
    /// A core still connecting has no strategy list yet, and the window would otherwise print the
    /// raw id for its whole life — the Report's dedup path re-focuses an open window rather than
    /// rebuilding it, so there is no second chance anywhere else.
    meta: TradeMeta,
    /// Whether the strategy still has to be named. Cleared the moment it is, so the backend
    /// observer stops walking a core's strategy list — which runs into the thousands — per notify.
    strategy_pending: bool,
    /// Strategy revision this window last searched, so an unchanged list costs one compare.
    ///
    /// `None` covers "that core is not in the store at all", which is its own state and must not
    /// read as revision zero — a core that arrives with an empty list would then never be searched.
    strategies_rev: Option<u64>,
    /// Set when the window closes, so the worker abandons the remaining pages.
    cancel: Arc<AtomicBool>,
    /// Identity of this window's own series, so two windows never share a chart revision.
    identity: u64,
    /// Native window id, used to unregister exactly this window and no other.
    window_id: WindowId,
    /// Window-root focus handle, so a key press reaches this view rather than only the chart.
    ///
    /// The root owns the handle instead of the [`ChartPanel`] because the key this window cares
    /// about is a WINDOW command — Escape closes the window — and a descendant that took focus
    /// would otherwise be the only thing hearing it.
    focus: FocusHandle,
    /// Live taskbar-suppression burst; replaced on every activation and cancelled on release.
    taskbar_hide: crate::window::windowing::TaskbarHideTask,
    /// Offset this window was opened at, so [`remembered_geometry`] knows not to persist it back.
    cascade_px: f32,
}

impl TradeWindowView {
    /// Close this window on a bare Escape.
    ///
    /// A SECOND way out, never the first: the close button in the header is the affordance, and a
    /// key nobody can see is not one. It exists because this window hides its taskbar button, so
    /// a user whose pointer cannot reach the chrome — a window dragged mostly off-screen, a
    /// display that went away — would otherwise have nothing left.
    ///
    /// Bare Escape only. A modifier held over it is a different gesture, and the chart's own
    /// hotkey layer already reads Escape with modifiers as its own; consuming those here would
    /// take a binding away from a surface that has one.
    ///
    /// Args:
    ///     event: The key press.
    ///     window: The window this view is the root of.
    ///     cx: View context.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "escape" || event.keystroke.modifiers.modified() {
            return;
        }
        cx.stop_propagation();
        // The same call the open cap uses to retire a window, so the `on_release` path that
        // cancels the fetch and drops the market refs still runs.
        window.remove_window();
    }

    /// Name the trade's strategy once the core's list has arrived, and hand it to the chart.
    ///
    /// Called from the backend observer. Cheap while there is nothing to do: one boolean.
    ///
    /// Args:
    ///     cx: View context.
    fn retry_strategy_name(&mut self, cx: &mut Context<Self>) {
        if !self.strategy_pending {
            return;
        }
        let Some(id) = self.meta.strategy_id else {
            self.strategy_pending = false;
            return;
        };
        // The core's own strategy revision, compared BEFORE the list is walked: this runs on every
        // backend notification, the list runs into the thousands, and a core that never connects
        // would otherwise be searched for the window's whole life.
        let backend = self.backend.read(cx);
        let rev = backend
            .session
            .store()
            .core(self.core)
            .map(|core| core.strategies_rev);
        if rev == self.strategies_rev {
            return;
        }
        self.strategies_rev = rev;
        // A list that does not hold this id is NOT a reason to stop: the feed republishes the whole
        // set whenever its signature moves, and a set that is still filling publishes as non-empty,
        // so "not there yet" and "deleted since the trade closed" look identical here. The cost of
        // keeping the door open is one walk per REVISION — the gate above is what makes that cheap
        // — and a deleted strategy simply keeps printing its number, which is the honest answer.
        let Some(name) = strategy_name(backend, self.core, id) else {
            return;
        };
        self.strategy_pending = false;
        let labels = std::rc::Rc::new(crate::chartdx::TradeLabels {
            strategy: name,
            detect: self.meta.detect.clone(),
            sell_reason: self.meta.sell_reason.clone(),
        });
        self.panel.update(cx, |panel, pcx| {
            panel.attach_trade_labels(Some(labels), pcx);
        });
    }

    /// Store a caption edit this window's own chart menu produced.
    ///
    /// The panel APPLIES such an edit itself and hands the set up for its owner to persist — the
    /// same relay the tab strip and the detached window use. This window is that owner, and it has
    /// no tab spec to put it in: it is not a tab. So the set becomes the DEFAULT of the kind this
    /// window is, which is the only store it has and the same one a ⧉ press from the main chart
    /// writes.
    ///
    /// The honest cost, stated rather than hidden: this window's edit reaches every trade window,
    /// including one already open beside it. A per-window override would need somewhere to live
    /// past the window's own life, and nothing here has one.
    ///
    /// Args:
    ///     cx: View context.
    fn drain_panel_labels(&mut self, cx: &mut Context<Self>) {
        let Some(cfg) = self
            .panel
            .update(cx, |panel, _| panel.take_pending_labels())
        else {
            return;
        };
        self.backend.update(cx, |b, bcx| {
            // `store_chart_labels`, NOT the ⧉ press's `set_chart_labels_default`: that one also
            // SEPARATES the kinds, freezing the tab kinds at Main's current captions. A right-click
            // toggle in this window is not a statement about anybody's tabs.
            if b.layout
                .store_chart_labels(moon_core::config::ChartTabKind::Trade, cfg)
            {
                b.layout_dirty = true;
            }
            // The other trade window reads this default on the next notification; without this it
            // would keep the old captions until something unrelated woke it. It adopts them only
            // while it holds no set of its own — a window whose own menu has been used keeps what
            // that menu applied until it is reopened, exactly as a tab with an override does.
            bcx.notify();
        });
    }

    /// Whether the chart area should show an overlay instead of the chart.
    ///
    /// Returns:
    ///     `true` while there is nothing on the chart worth looking at.
    fn overlays_chart(&self) -> bool {
        !matches!(self.state, TradeWindowState::Ready { .. })
    }

    /// Start, or restart, the fetch for this trade's window.
    ///
    /// Three independent guards keep a closed window from acting on a late answer, and all three
    /// are wanted: the entity update fails once the view is gone, the sequence check discards an
    /// answer a newer request has superseded, and the cancel flag is the only one of the three
    /// that stops the network work rather than merely its result.
    ///
    /// Args:
    ///     cx: View context.
    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(window) = replay_window(self.record.buy_date, self.record.close_date) else {
            self.state = TradeWindowState::Empty(TradeReplayEmpty::DegenerateWindow);
            cx.notify();
            return;
        };
        let address = match self
            .backend
            .read(cx)
            .session
            .market_source()
            .replay_address(self.core)
        {
            Ok(address) => address,
            Err(error) => {
                // Two different sentences: reconnect the core, or this build does not know that
                // exchange. Collapsing them would tell the user to reconnect something that is
                // already connected and answering.
                self.state = TradeWindowState::Empty(match error {
                    moon_core::market::ReplayAddressError::NotConnected => {
                        TradeReplayEmpty::CoreNotConnected
                    }
                    moon_core::market::ReplayAddressError::UnknownVenue => {
                        TradeReplayEmpty::UnknownVenue
                    }
                });
                cx.notify();
                return;
            }
        };
        // Supersede any request already in flight before arming the new one, or a slow first
        // answer could land after a fast second and overwrite it.
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.sequence = self.sequence.wrapping_add(1);
        let sequence = self.sequence;
        self.state = TradeWindowState::Loading;
        cx.notify();

        let (tx, rx) = mpsc::channel();
        worker::request(TradeReplayRequest {
            address,
            market: self.market.clone(),
            window,
            identity: self.identity,
            cancel: self.cancel.clone(),
            reply: tx,
        });
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            // The blocking receive sits on the background executor; the worker's own job deadline
            // is what bounds it, so this task cannot outlive a stalled fetch indefinitely.
            let Ok(outcome) = executor.spawn(async move { rx.recv() }).await else {
                return;
            };
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| this.apply(sequence, outcome, cx));
            });
        })
        .detach();
    }

    /// Fold one fetch answer into the window.
    ///
    /// Args:
    ///     sequence: Dispatch counter the answer belongs to.
    ///     outcome: What the worker produced.
    ///     cx: View context.
    fn apply(&mut self, sequence: u64, outcome: TradeReplayOutcome, cx: &mut Context<Self>) {
        // An answer from a superseded request is not wrong, merely stale; dropping it silently is
        // the whole point of the counter.
        if sequence != self.sequence {
            return;
        }
        self.state = match outcome {
            TradeReplayOutcome::Ready(series) if series.is_empty() => {
                TradeWindowState::Empty(TradeReplayEmpty::NoDataInWindow)
            }
            TradeReplayOutcome::Ready(series) => {
                let source = series.source;
                // Read off the SERIES, before it is moved, because the series is the only honest
                // answer: it is what the rows actually are. The panel is pinned to one minute and
                // the fetch is one minute, so today this is always 1 — and it stays true rather
                // than merely correct if either ever changes, which is the whole point of not
                // hardcoding the caption. `clamp` makes the conversion total instead of trusting a
                // range: a floor of one minute for anything finer, and no `as` wrap for anything
                // absurdly coarse.
                let tf_min = (series.tf_ms / 60_000).clamp(1, i64::from(u16::MAX)) as u16;
                self.publish(series, cx);
                TradeWindowState::Ready { source, tf_min }
            }
            TradeReplayOutcome::Empty(empty) => TradeWindowState::Empty(empty),
            TradeReplayOutcome::Failed(failure) => {
                // The diagnostic is an English transport fragment and belongs in the log, never in
                // the user's sentence; the window says only that the fetch failed.
                if let TradeReplayFailure::Transient { diagnostic } = &failure {
                    log::warn!("[x] trade replay failed for {}: {diagnostic}", self.market);
                }
                TradeWindowState::Failed(failure)
            }
        };
        cx.notify();
    }

    /// The price scale this window's chart is set to, for its own control to state.
    ///
    /// Read off the PANEL rather than back out of the layout, so the trigger reports what this
    /// chart is actually on. The two agree today, and reading the chart is what keeps them
    /// agreeing if they ever stop.
    ///
    /// Args:
    ///     cx: Application context used to read the panel.
    ///
    /// Returns:
    ///     The configured scale, or `None` for Auto.
    pub(crate) fn scale(&self, cx: &App) -> Option<f32> {
        self.panel.read(cx).scale()
    }

    /// Apply a price scale to THIS window, for as long as it is open.
    ///
    /// Not remembered: a scale that outlived the window meant every later trade opened at a number
    /// chosen for a different one, and a position whose range does not fit that number is drawn
    /// off-screen — the window then looks frozen rather than scaled. Each window opens on Auto and
    /// fits the trade it was opened for; this control is a look at it from another zoom, not a
    /// setting.
    ///
    /// Display-only, like everything else this window can do: it changes how the closed trade is
    /// drawn and reaches no core. The forcing variant is deliberate — see
    /// [`crate::panels::chart::ChartPanel::force_scale`] — so picking the preset already shown
    /// undoes a vertical drag instead of being swallowed as a no-op.
    ///
    /// Args:
    ///     pct: The chosen scale, or `None` for Auto.
    ///     cx: View context.
    pub(crate) fn pick_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        self.panel
            .update(cx, |panel, pcx| panel.force_scale(pct, pcx));
        cx.notify();
    }

    /// Hand a fetched series to this window's chart and focus it on the trade.
    ///
    /// Args:
    ///     series: The frozen rows.
    ///     cx: View context.
    fn publish(&mut self, series: TradeReplaySeries, cx: &mut Context<Self>) {
        // Read off the series BEFORE it is moved into the panel, exactly as `source` and `tf_min`
        // are in `apply`.
        let frame = frame::trade_frame(
            self.record.buy_date.saturating_mul(1_000),
            self.record.close_date.saturating_mul(1_000),
            series.tf_ms,
        );
        let record = self.record.clone();
        self.panel.update(cx, |panel, pcx| {
            panel.attach_trade_replay(Some(std::rc::Rc::new(series)), pcx);
            // THE ENTRY AND EXIT ARROWS. Owning a `ChartPanel` is not enough on its own: the
            // marker geometry is built during the userdata pass, and that pass only ever sees the
            // trades handed to this layer. The live Report path fills it from the open-request
            // drain, which this window deliberately never touches, so the clicked trade has to be
            // published here or the window shows a chart with nothing marked on it — which is the
            // one thing this whole feature exists to fix.
            panel.publish_trade_history(std::rc::Rc::new(vec![record]), pcx);
            // The viewport is placed on the TRADE with its own proportional context, not on the
            // window the rows cover. Those differ on purpose: the fetch is asymmetric by design,
            // so framing it put a short position three quarters of the way to the right while a
            // long one sat centred — two trades, two differently composed pictures, which is the
            // thing the user asked to be made the same everywhere.
            //
            // The padding argument is ZERO because the framing rule has already built the
            // breathing room into the interval. Asking for more here would push the right edge
            // past the twenty-minute trailing margin the fetch guarantees and draw blank.
            //
            // TWO boundaries, both stated rather than defended against, and neither one a framing
            // choice this function is free to make.
            //
            // A position held longer than the fetch's own seven-day budget keeps only its floors,
            // so the framing rule hands back exactly what was downloaded and such a trade opens
            // off-centre rather than centred.
            //
            // And past ROUGHLY A YEAR the exit leaves the opening viewport altogether. The
            // doubling the framing rule would otherwise apply does NOT survive that far: any
            // position over about three and a half days already puts the fetch over its budget,
            // which collapses the downloaded window to the two floors, and the clamp above then
            // holds the interval at the held duration plus those eighty minutes. So it is the
            // POSITION reaching a year that trips the chart's 365-day cap, not twice the position
            // reaching it at six months. The cap anchors what remains at the START. Centring it
            // would show the middle of such a trade and NEITHER end, which is worse: the entry is
            // the more useful of the two to open on. Neither boundary is silent — the chart takes
            // the wheel like any other, so the exit is a scroll away — and no one-year window can
            // hold both ends of a multi-year trade.
            if let Some((start_ms, end_ms)) = frame {
                panel.show_time_range(start_ms, end_ms, 0.0);
            }
        });
        cx.notify();
    }
}
