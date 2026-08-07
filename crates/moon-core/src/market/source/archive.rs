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
//! [`SharedMoonClient`]: crate::feed::SharedMoonClient

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
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

/// What became of this market's archive request under one installed client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The client took the request. MoonProto owns it from here, retry included, so this is final
    /// for this client whether or not `Ready` ever arrives.
    Accepted,
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

/// Everything the archive demand side remembers, behind one lock.
///
/// The per-market state and the send clock are always consulted together on the same pass, so they
/// share a lock rather than nesting two.
#[derive(Default)]
pub(super) struct ArchiveGate {
    state: Mutex<GateState>,
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
        Outcome::Accepted => Decision::SkipAccepted,
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
                    Outcome::Accepted => 0,
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

    /// Forget everything about one provider.
    pub(super) fn forget_provider(&self, provider: CoreId) {
        let mut state = self.state.lock().expect("archive gate poisoned");
        state.markets.remove(&provider);
    }

    /// Keep only the providers still in use.
    pub(super) fn retain_providers(&self, keep: &HashSet<CoreId>) {
        let mut state = self.state.lock().expect("archive gate poisoned");
        state.markets.retain(|provider, _| keep.contains(provider));
    }

    /// Forget every provider and reset the send clock.
    pub(super) fn clear(&self) {
        let mut state = self.state.lock().expect("archive gate poisoned");
        state.markets.clear();
        state.last_send = None;
    }
}

#[cfg(test)]
mod tests;
