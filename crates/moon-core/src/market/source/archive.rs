//! Demand-driven core chart archive.
//!
//! A live trades subscription only starts filling MoonProto's retained rings once the client is
//! connected, so a chart opened later shows crosses, liquidations and the price line beginning at
//! connect time and nothing before it. The core, however, already holds that history. One
//! `request_chart` per open market pulls it and MoonProto merges it into the SAME retained rings
//! the chart already reads — deduplicated against the live tail, oldest row first.
//!
//! This module owns only the DEMAND side: who asks and when to stop asking. The archive arrives as
//! `Event::MarketHistory`, which the feed turns into
//! [`MarketDirtyFlags::HISTORY_ARCHIVE`](crate::feed::MarketDirtyFlags::HISTORY_ARCHIVE) so open
//! charts re-read their window; see [`super::bump_market_revisions`].
//!
//! # Why there is no resend timer here
//!
//! The obvious design — "re-ask if no `Ready` arrives within a minute" — is wrong against this
//! server. MoonProto's `poll_market_history` keeps a request registered and re-issues the WHOLE
//! archive every 15 seconds of silence, with no attempt cap, and emits no `Failed` for a core that
//! simply never answers (`runtime_loop/pending.rs`). A terminal-side resend therefore does not
//! replace a lost request; it adds a second endlessly-retrying multi-megabyte transfer beside the
//! first. Retry belongs to MoonProto, so this module sends at most once per installed client and
//! then stops. If that request never completes, the market keeps live-only history until the next
//! reconnect — which installs a new client, bumps [`SharedMoonClient`]'s epoch, and asks again.
//!
//! # Why the demand lives on the chart read path
//!
//! The natural-looking home is the front where a market becomes wanted
//! (`session::coordinator::set_open`), which already fires once per market. It is the wrong one:
//! that front is crossed while a restored layout still has no connected client, and the request
//! needs a live snapshot. Readiness is asynchronous, so the demand has to be re-offered until it
//! can be met — which is what a read path naturally does, and why the sibling CoinCard requests
//! live there too. The gate below is that readiness retry, not a scheduler.
//!
//! # Waiting for the answer
//!
//! The tuner's tape stage files what the ring holds into its tiles and sends the rest to the
//! venue, so a ring copied before the archive lands costs the venue the whole stretch the archive
//! would have held — and a venue with no route (Bybit, Hyperliquid) answers Missing. That reader
//! therefore waits for the core's answer ([`ArchiveGate::request_and_wait`]), bounded by
//! [`ARCHIVE_WAIT`]. Only the FIRST answer per market and client is ever waited for: once it is
//! [`Outcome::Answered`] the ring holds the archive plus the live tail since, and every later row
//! of that market copies at once. The answer reaches the gate through the core's feed
//! (`FeedMsg::ChartArchiveAnswered`), from any core — not through the chart's dirty flags, which
//! only an elected provider with an open chart raises.
//!
//! [`SharedMoonClient`]: crate::feed::SharedMoonClient

use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use moonproto::MoonClient;

use super::market_diag;
use crate::session::CoreId;

/// Wait before retrying a request the client REFUSED to send.
///
/// This is not the server saying no — it is `MoonClient` rejecting locally, because its own fresh
/// snapshot does not (yet) place the market in the retained trades scope, or because its runtime
/// has stopped. The first resolves within seconds of connecting, so the wait is short.
const ARCHIVE_SEND_RETRY: Duration = Duration::from_secs(3);

/// Refusals per market and client epoch before giving up until the next reconnect.
///
/// A refusal costs nothing on the wire, but once the runtime is gone every refusal repeats, one
/// per open chart every [`ARCHIVE_SEND_RETRY`]. About a minute of trying is enough.
const ARCHIVE_MAX_REFUSALS: u32 = 20;

/// Minimum spacing between archive sends, across every provider and market.
///
/// Restoring a saved layout opens every chart in the same frame, and each archive is a separate
/// multi-megabyte transfer that MoonProto merges on ONE history-worker thread — the same thread
/// that feeds every market's live tail. Firing them together stalls it. Spacing the sends costs a
/// few hundred milliseconds of history arriving later and keeps the live tail moving.
const ARCHIVE_SEND_SPACING: Duration = Duration::from_millis(200);

/// Longest a tape reader waits for a market's first archive answer.
///
/// The core answers in 0.3–2.3 s (27.09, 25 answers across six venues); the spacing above queues a
/// burst of markets behind each other at five a second. Past this the ring is copied as it stands
/// and the venue asked for the rest, as before the wait existed.
pub(crate) const ARCHIVE_WAIT: Duration = Duration::from_secs(5);

/// How long [`ArchiveGate::request_and_wait`] sleeps between looks at its own client: an answer
/// wakes it at once, a reconnect only shows on the next look.
const WAIT_SLICE: Duration = Duration::from_millis(250);

/// What became of this market's archive request under one installed client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The client took the request. MoonProto owns it from here, retry included, so this is final
    /// for this client whether or not `Ready` ever arrives.
    Accepted,
    /// The core answered an accepted request — merged it into the retained rings, or failed it.
    /// Final for this client like [`Self::Accepted`]; the difference is that nobody waits on it.
    Answered,
    /// The client refused to send it. Bounded and spaced; a refusal cannot exist without the time
    /// it happened, which is why this carries both.
    Refused { count: u32, at: Instant },
}

/// One market's archive-request state for one installed client.
struct ArchiveRequest {
    /// The [`crate::feed::SharedMoonClient`] epoch this state describes. A different epoch is a
    /// different client with empty retained rings, so the archive must be asked for again.
    epoch: u64,
    outcome: Outcome,
}

/// How [`ArchiveGate::request_and_wait`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveWait {
    /// The core answered: the rings hold whatever archive it had.
    Answered,
    /// The client refused to send; nothing is coming under this client for now.
    Refused,
    /// The deadline passed first. MoonProto keeps the request and may still merge it later.
    TimedOut,
    /// The client was replaced while waiting — a reconnect. Its rings are not the ones to copy.
    Superseded,
}

/// Everything the archive demand side remembers, behind one lock.
///
/// The per-market state and the send clock are always consulted together on the same pass, so they
/// share a lock rather than nesting two.
#[derive(Default)]
pub(super) struct ArchiveGate {
    state: Mutex<GateState>,
    /// Signalled on every [`Outcome::Answered`] and on every send, for
    /// [`ArchiveGate::request_and_wait`].
    changed: Condvar,
}

#[derive(Default)]
struct GateState {
    /// Per provider, then per market, so the hot lookup can borrow the market name instead of
    /// allocating a key on every chart read.
    markets: HashMap<CoreId, HashMap<String, ArchiveRequest>>,
    /// When the last archive request of ANY market went out.
    last_send: Option<Instant>,
}

/// Why this market was or was not asked, so a diagnostic can name the reason instead of leaving
/// "nothing happened" indistinguishable from "deliberately skipped".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// No state for this market under this client — never asked.
    SendFirst,
    /// State exists but belongs to a superseded client.
    SendNewClient,
    /// A refusal whose retry window has elapsed.
    SendRetryRefusal,
    /// Already accepted by this client; MoonProto owns it.
    SkipAccepted,
    /// Refused recently; still inside the retry window.
    SkipRefusalWindow,
    /// Refused too many times for this client.
    SkipRefusalCap,
}

impl Decision {
    fn sends(self) -> bool {
        matches!(
            self,
            Self::SendFirst | Self::SendNewClient | Self::SendRetryRefusal
        )
    }
}

/// Whether this market may be asked again, given what is already known about it.
///
/// `None`, or state belonging to a superseded client, always may: a new client means empty
/// retained rings.
fn decide(state: Option<&ArchiveRequest>, epoch: u64, now: Instant) -> Decision {
    let Some(state) = state else {
        return Decision::SendFirst;
    };
    if state.epoch != epoch {
        return Decision::SendNewClient;
    }
    match state.outcome {
        Outcome::Accepted | Outcome::Answered => Decision::SkipAccepted,
        Outcome::Refused { count, .. } if count >= ARCHIVE_MAX_REFUSALS => Decision::SkipRefusalCap,
        Outcome::Refused { at, .. } if now.duration_since(at) < ARCHIVE_SEND_RETRY => {
            Decision::SkipRefusalWindow
        }
        Outcome::Refused { .. } => Decision::SendRetryRefusal,
    }
}

/// Whether enough time has passed since the previous archive send of any market.
fn spacing_allows(last_send: Option<Instant>, now: Instant) -> bool {
    last_send.is_none_or(|previous| now.duration_since(previous) >= ARCHIVE_SEND_SPACING)
}

impl ArchiveGate {
    /// Ask `client` for `market`'s accumulated chart archive, at most once per installed client.
    ///
    /// Called from the chart history read, so the settled case — "already asked" — must stay cheap:
    /// one lock and one borrowed-key lookup, no allocation. Failures are diagnostics, not something
    /// the caller can act on; the chart keeps rendering live history either way.
    pub(super) fn request(&self, provider: CoreId, market: &str, client: &MoonClient, epoch: u64) {
        let now = Instant::now();
        // Claim the send under the lock, then RELEASE it before touching the client: the request
        // walks MoonProto's snapshot and posts to its runtime thread.
        //
        // The per-market entry is not written until phase two, so between the two the market looks
        // unasked. What closes that window is the spacing stamp taken here: a second caller for the
        // same market is turned away by it for [`ARCHIVE_SEND_SPACING`], while the send in between
        // is a synchronous local post that returns in microseconds.
        let decision = {
            let mut state = self.state.lock().expect("archive gate poisoned");
            let previous = state
                .markets
                .get(&provider)
                .and_then(|markets| markets.get(market));
            let decision = decide(previous, epoch, now);
            let claimed = decision.sends() && spacing_allows(state.last_send, now);
            // Spacing is checked LAST, and its timestamp is written only for a send that actually
            // goes out: bumping it on a rejected attempt would push the next allowed send further
            // away on every frame and nothing would ever be sent.
            if claimed {
                state.last_send = Some(now);
            }
            (decision, claimed)
        };
        let (decision, claimed) = decision;
        if !claimed {
            // Throttled, and the reason is named: a repeat of an already-accepted market and a
            // market held back by spacing look identical from the outside, and telling them apart
            // is the whole point of this line.
            if super::market_diag_enabled()
                && super::market_diag_due(
                    format!("archive-skip:{provider}:{market}"),
                    Duration::from_secs(5),
                )
            {
                market_diag(format!(
                    "chart archive skipped {market} provider={provider} epoch={epoch}: {}",
                    if decision.sends() {
                        "spacing"
                    } else {
                        match decision {
                            Decision::SkipAccepted => "already accepted by this client",
                            Decision::SkipRefusalWindow => "refusal retry window",
                            Decision::SkipRefusalCap => "refusal cap",
                            _ => "unreachable",
                        }
                    }
                ));
            }
            return;
        }

        let result = client.history().request_chart(market);
        let refusals = {
            let mut state = self.state.lock().expect("archive gate poisoned");
            let markets = state.markets.entry(provider).or_default();
            // Carry the refusal count forward, but only from state for THIS client: a reconnect
            // between the claim and here starts the count over, as it should.
            let carried = match markets.get(market) {
                Some(previous) if previous.epoch == epoch => match previous.outcome {
                    Outcome::Refused { count, .. } => count,
                    Outcome::Accepted | Outcome::Answered => 0,
                },
                _ => 0,
            };
            let outcome = match &result {
                Ok(_) => Outcome::Accepted,
                Err(_) => Outcome::Refused {
                    count: carried + 1,
                    at: now,
                },
            };
            markets.insert(market.to_string(), ArchiveRequest { epoch, outcome });
            carried + 1
        };
        self.changed.notify_all();
        match result {
            Ok(ticket) => market_diag(format!(
                "chart archive requested {market} provider={provider} epoch={epoch} \
                 why={decision:?} ticket={}",
                ticket.id()
            )),
            Err(e) => market_diag(format!(
                "chart archive request refused {market} provider={provider} \
                 ({refusals}/{ARCHIVE_MAX_REFUSALS}): {e}"
            )),
        }
    }

    /// The core answered `market`'s archive request — merged or failed — on `provider`'s feed,
    /// from the client installed under `epoch`.
    ///
    /// Only an [`Outcome::Accepted`] entry of that same epoch moves. A network reconnect keeps the
    /// provider's slot and bumps its epoch without forgetting the gate's entries, so an answer the
    /// previous connection sent can still be in the feed channel when the new client's request for
    /// the same market is already recorded; matched on the market alone it would settle a request
    /// the new core has not answered. An answer for a market this gate never sent, or already
    /// forgot, belongs to nobody waiting here.
    pub(super) fn answered(&self, provider: CoreId, market: &str, epoch: u64) {
        let moved = {
            let mut state = self.state.lock().expect("archive gate poisoned");
            match state
                .markets
                .get_mut(&provider)
                .and_then(|markets| markets.get_mut(market))
            {
                Some(request) if request.epoch == epoch && request.outcome == Outcome::Accepted => {
                    request.outcome = Outcome::Answered;
                    true
                }
                _ => false,
            }
        };
        if moved {
            self.changed.notify_all();
        }
    }

    /// [`Self::request`], then block until the core answers it or `deadline` passes.
    ///
    /// For a reader that COPIES the ring rather than rendering it live — the tuner's tape stage:
    /// a chart re-reads its window on the archive flag, a copy taken before the answer is final.
    /// Returns at once when the market was answered earlier under this client, or when the client
    /// refuses to send; spacing only delays the send, so a throttled request is retried until the
    /// deadline. `is_current` says whether `client` is still the one installed under `epoch`: a
    /// reconnect replaces it without telling the gate, and a send through the dead client would
    /// overwrite whatever the new one has already asked.
    pub(super) fn request_and_wait(
        &self,
        provider: CoreId,
        market: &str,
        client: &MoonClient,
        epoch: u64,
        deadline: Instant,
        is_current: impl Fn() -> bool,
    ) -> ArchiveWait {
        self.wait_for_answer(provider, market, epoch, deadline, is_current, || {
            self.request(provider, market, client, epoch)
        })
    }

    /// The loop behind [`Self::request_and_wait`], with the send passed in so the waiting can be
    /// tested without a live client.
    fn wait_for_answer(
        &self,
        provider: CoreId,
        market: &str,
        epoch: u64,
        deadline: Instant,
        is_current: impl Fn() -> bool,
        mut send: impl FnMut(),
    ) -> ArchiveWait {
        let outcome = |state: &GateState| {
            state
                .markets
                .get(&provider)
                .and_then(|markets| markets.get(market))
                .filter(|request| request.epoch == epoch)
                .map(|request| request.outcome)
        };
        // Whether this reader has seen its own request recorded. Until then a missing entry is a
        // send still held back by spacing, and asking again is right; after it, a missing entry
        // means the gate forgot the provider — a respawn or a departed core — and asking again
        // would send through a client on its way out and record a claim under an epoch the next
        // slot starts over with.
        let mut seen = false;
        loop {
            if !is_current() {
                return ArchiveWait::Superseded;
            }
            if !seen {
                send();
            }
            let state = self.state.lock().expect("archive gate poisoned");
            let now = Instant::now();
            match outcome(&state) {
                Some(Outcome::Answered) => return ArchiveWait::Answered,
                Some(Outcome::Refused { .. }) => return ArchiveWait::Refused,
                None if seen => return ArchiveWait::Superseded,
                _ if now >= deadline => return ArchiveWait::TimedOut,
                // Sent and not answered yet, or not sent yet. Either way wait for a change, but
                // only one slice at a time: a reconnect replaces the client without a word to the
                // gate, and the loop must notice it well before the deadline.
                found => {
                    seen |= found.is_some();
                    let slice = WAIT_SLICE.min(deadline - now);
                    drop(
                        self.changed
                            .wait_timeout(state, slice)
                            .expect("archive gate poisoned"),
                    );
                }
            }
        }
    }

    /// Forget everything about one provider.
    pub(super) fn forget_provider(&self, provider: CoreId) {
        let dropped = {
            let mut state = self.state.lock().expect("archive gate poisoned");
            state.markets.remove(&provider).map_or(0, |m| m.len())
        };
        if dropped > 0 {
            // A tape reader waiting on this provider's answer wakes, finds its request gone and
            // stops waiting ([`ArchiveWait::Superseded`]) without asking again.
            self.changed.notify_all();
            // A respawn hands over a whole new client slot whose epoch restarts, so this is the
            // path that lets an already-answered market ask again. Silent, it is indistinguishable
            // from a reconnect bumping the epoch — which is a different mechanism with a different
            // failure mode.
            market_diag(format!(
                "chart archive forgot provider={provider}: {dropped} market(s)"
            ));
        }
    }

    /// Keep only the providers still in use.
    pub(super) fn retain_providers(&self, keep: &HashSet<CoreId>) {
        let mut state = self.state.lock().expect("archive gate poisoned");
        state.markets.retain(|provider, _| keep.contains(provider));
        drop(state);
        self.changed.notify_all();
    }

    /// Forget every provider and reset the send clock.
    pub(super) fn clear(&self) {
        let mut state = self.state.lock().expect("archive gate poisoned");
        state.markets.clear();
        state.last_send = None;
        drop(state);
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests;
