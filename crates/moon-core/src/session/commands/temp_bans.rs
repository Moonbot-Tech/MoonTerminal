//! Reconcile temporary-ban commands with the snapshots the core has published.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::feed::CoreCmd;
use crate::session::{CoreId, SessionManager};
use crate::util::now_unix_ms_i64;

use super::BLACKLIST_ECHO_WAIT;

#[cfg(test)]
mod tests;

/// The feed tolerates one minute of decay and quantization between snapshots.
const SNAPSHOT_TOLERANCE_MS: u64 = 60_000;

/// A local write owns a deadline. The timestamp distinguishes repeated presets.
#[derive(Clone, Copy)]
struct TempBanState {
    deadline: Option<i64>,
    owner: Option<Instant>,
}

/// The latest command and the states that can still arrive ahead of its echo.
struct TempBanIntent {
    state: TempBanState,
    sent_at_ms: i64,
    sent_at: Instant,
    preceding: Vec<Option<i64>>,
}

impl TempBanIntent {
    /// Keep pending intent across an echo of a command that preceded it.
    fn effective(&self, published: Option<i64>, published_at_ms: Option<i64>) -> TempBanState {
        if self.sent_at.elapsed() < BLACKLIST_ECHO_WAIT
            && (published_at_ms.is_none_or(|at| at <= self.sent_at_ms)
                || self
                    .preceding
                    .iter()
                    .any(|&old| same_deadline(old, published))
                || same_deadline(self.state.deadline, published))
        {
            return self.state;
        }
        TempBanState {
            deadline: published,
            owner: None,
        }
    }
}

/// Compare deadlines with the snapshot's rounding tolerance.
fn same_deadline(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.abs_diff(b) <= SNAPSHOT_TOLERANCE_MS,
        _ => a == b,
    }
}

/// Temporary-ban intentions share a session across all of the app's ban and lift controls.
#[derive(Default)]
pub(in crate::session) struct TempBanIntents(HashMap<(CoreId, String), TempBanIntent>);

impl TempBanIntents {
    /// Drop ownership when a core is removed from the session.
    pub(in crate::session) fn remove_core(&mut self, core: CoreId) {
        self.0.retain(|(id, _), _| *id != core);
    }

    /// Discard obsolete predecessors once the latest write has been observed.
    fn observe(
        &mut self,
        core: CoreId,
        mut snapshot: impl FnMut(&str) -> (Option<i64>, Option<i64>),
    ) {
        self.0.retain(|(id, symbol), intent| {
            if intent.sent_at.elapsed() >= BLACKLIST_ECHO_WAIT {
                return false;
            }
            if *id != core {
                return true;
            }
            let (published, at) = snapshot(symbol);
            if same_deadline(intent.state.deadline, published) {
                intent.preceding.clear();
                true
            } else {
                intent.effective(published, at).owner == intent.state.owner
            }
        });
    }

    /// Resolve the effective deadline from the published row and this session's writes.
    fn effective(
        &self,
        key: &(CoreId, String),
        (published, at): (Option<i64>, Option<i64>),
    ) -> TempBanState {
        self.0.get(key).map_or(
            TempBanState {
                deadline: published,
                owner: None,
            },
            |intent| intent.effective(published, at),
        )
    }

    /// Retain the predecessor deadlines only while those writes can still echo.
    fn record(
        &mut self,
        key: (CoreId, String),
        (published, at): (Option<i64>, Option<i64>),
        state: TempBanState,
        sent_at_ms: i64,
        sent_at: Instant,
    ) {
        self.0
            .retain(|_, intent| intent.sent_at.elapsed() < BLACKLIST_ECHO_WAIT);
        let mut preceding = vec![published];
        let previous = self.0.remove(&key).filter(|previous| {
            !same_deadline(previous.state.deadline, published)
                && previous.effective(published, at).owner == previous.state.owner
        });
        if let Some(mut previous) = previous {
            preceding.append(&mut previous.preceding);
            preceding.push(previous.state.deadline);
            preceding.sort_unstable();
            preceding.dedup();
        }
        self.0.insert(
            key,
            TempBanIntent {
                state,
                sent_at_ms,
                sent_at,
                preceding,
            },
        );
    }
}

/// Restore the old deadline using the time still left at the undo click.
fn temp_restore_span(prior: Option<i64>, now: i64) -> Option<Duration> {
    let remaining = prior?.saturating_sub(now);
    (remaining > 0).then(|| Duration::from_millis(remaining as u64))
}

/// Compute the intended deadline before queueing a duration.
fn temp_deadline(now: i64, span: Duration) -> i64 {
    now.saturating_add(i64::try_from(span.as_millis()).unwrap_or(i64::MAX))
}

/// A local write must match the click exactly. Only published snapshots use rounding slack.
fn temp_undo_matches(current: TempBanState, clicked: TempBanState) -> bool {
    match current.owner {
        Some(owner) => clicked.owner == Some(owner) && current.deadline == clicked.deadline,
        None => same_deadline(current.deadline, clicked.deadline),
    }
}

impl SessionManager {
    /// Reconcile temp-ban writes at publication time, including echoes between UI clicks.
    pub(in crate::session) fn observe_temp_ban_echo(&self, core: CoreId) {
        self.store
            .temp_ban_intents
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .observe(core, |symbol| self.temp_ban_snapshot(core, symbol));
    }

    /// Ban one market temporarily, or lift its ban with `None`.
    pub fn set_temp_ban(&self, core: CoreId, symbol: String, span: Option<Duration>) -> Result<()> {
        let (adds, removes) = temp_ban_delta(symbol, span);
        self.set_temp_blacklist(core, adds, removes)
    }

    /// Submit a temp-ban delta and remember it before any of its echoes can be consumed.
    pub fn set_temp_blacklist(
        &self,
        core: CoreId,
        adds: Vec<(String, Duration)>,
        removes: Vec<String>,
    ) -> Result<()> {
        let mut intents = self
            .store
            .temp_ban_intents
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let now = now_unix_ms_i64();
        let at = Instant::now();
        let writes: Vec<_> = removes
            .iter()
            .map(|symbol| (symbol, None))
            .chain(
                adds.iter()
                    .map(|(symbol, span)| (symbol, Some(temp_deadline(now, *span)))),
            )
            .map(|(symbol, deadline)| {
                (
                    (core, symbol.to_ascii_uppercase()),
                    self.temp_ban_snapshot(core, symbol),
                    deadline,
                )
            })
            .collect();
        self.send_temp_blacklist(core, adds, removes)?;
        for (key, snapshot, deadline) in writes {
            intents.record(
                key,
                snapshot,
                TempBanState {
                    deadline,
                    owner: Some(at),
                },
                now,
                at,
            );
        }
        Ok(())
    }

    /// Queue the ban and return its inverse, which refuses if the deadline has since changed.
    pub fn add_temp_ban_with_undo(
        &self,
        core: CoreId,
        symbol: String,
        span: Duration,
    ) -> Result<impl FnOnce(&Self) -> Result<bool> + use<>> {
        let key = (core, symbol.to_ascii_uppercase());
        let mut intents = self
            .store
            .temp_ban_intents
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let snapshot = self.temp_ban_snapshot(core, &symbol);
        let prior = intents.effective(&key, snapshot);
        let now = now_unix_ms_i64();
        let at = Instant::now();
        let clicked = TempBanState {
            deadline: Some(temp_deadline(now, span)),
            owner: Some(at),
        };
        self.send_temp_blacklist(core, vec![(symbol.clone(), span)], Vec::new())?;
        intents.record(key.clone(), snapshot, clicked, now, at);
        Ok(move |session: &Self| session.undo_temp_ban(key, symbol, clicked, prior))
    }

    /// Reverse just this ban, serializing the deadline check with other local writes.
    fn undo_temp_ban(
        &self,
        key: (CoreId, String),
        symbol: String,
        clicked: TempBanState,
        prior: TempBanState,
    ) -> Result<bool> {
        let mut intents = self
            .store
            .temp_ban_intents
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let snapshot = self.temp_ban_snapshot(key.0, &symbol);
        if !intents.0.contains_key(&key)
            || !temp_undo_matches(intents.effective(&key, snapshot), clicked)
        {
            return Ok(false);
        }
        let now = now_unix_ms_i64();
        let at = Instant::now();
        let remaining = temp_restore_span(prior.deadline, now);
        let restored = TempBanState {
            deadline: remaining.and(prior.deadline),
            owner: prior.owner.or(Some(at)),
        };
        let (adds, removes) = temp_ban_delta(symbol, remaining);
        self.send_temp_blacklist(key.0, adds, removes)?;
        intents.record(key, snapshot, restored, now, at);
        Ok(true)
    }

    /// Read the symbol's deadline and the timestamp of the containing snapshot together.
    fn temp_ban_snapshot(&self, core: CoreId, symbol: &str) -> (Option<i64>, Option<i64>) {
        self.store().core(core).map_or((None, None), |data| {
            (data.temp_ban_until_ms(symbol), data.temp_blacklist_at_ms)
        })
    }

    /// Send a delta while the caller holds the intent tracker.
    fn send_temp_blacklist(
        &self,
        core: CoreId,
        adds: Vec<(String, Duration)>,
        removes: Vec<String>,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetTempBlacklist { adds, removes },
            "set temp blacklist",
        )
    }
}

/// The temporary blacklist protocol represents a lift as a remove-only delta.
fn temp_ban_delta(
    symbol: String,
    span: Option<Duration>,
) -> (Vec<(String, Duration)>, Vec<String>) {
    match span {
        Some(span) => (vec![(symbol, span)], Vec::new()),
        None => (Vec::new(), vec![symbol]),
    }
}
