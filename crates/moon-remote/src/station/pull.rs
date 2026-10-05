//! The terminal's pull from its station (STATION.md §4.9): the tape around closed trades and their
//! order traces, what the station holds of what the terminal lacks. Each request is one `ctl`
//! on the shared administrator session; an idle pull does not keep its login open.

use moon_core::feed::types::Tick;
use moon_core::market::trade_replay::trade_cache::decode_prints;
use moon_core::station_api::{
    Answer, MAX_TAPE_ITEMS, MAX_TAPE_SPANS, MAX_TRACE_UIDS, Request, TapeResume, TapeWant,
    TradeTraces,
};

use super::api::call;
use super::{AdminConn, admin_conn, current_helper_status};
use crate::ssh::Target;

/// A pull's administrator identity and its initially checked helper.
pub struct Pull {
    conn: AdminConn,
}

/// What a tape pull brought.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TapeCount {
    /// Covered stretches filed.
    pub pieces: usize,
    /// Prints in them.
    pub prints: usize,
    /// Requests it took.
    pub requests: usize,
    /// Ended by its `stop` before everything was asked.
    pub stopped: bool,
}

impl Pull {
    /// Connect as the administrator and bring the helper up to date.
    pub fn open(target: &Target) -> anyhow::Result<Self> {
        let conn = admin_conn(target)?;
        current_helper_status(&conn)?;
        Ok(Self { conn })
    }

    /// Ask the station for `wants` and hand every covered stretch it holds to `file`:
    /// `(want, from_ms, to_ms, prints)` — the stretch is covered, the prints are all of it.
    ///
    /// Args:
    ///     wants: Stretches of markets the terminal lacks; split here to the API's bounds.
    ///     stop: Asked before every request; `true` ends the pull there, with what came so far.
    ///     file: Where each answered stretch goes.
    pub fn tape(
        &self,
        wants: Vec<TapeWant>,
        stop: impl Fn() -> bool,
        mut file: impl FnMut(&TapeWant, i64, i64, Vec<Tick>),
    ) -> anyhow::Result<TapeCount> {
        let mut count = TapeCount::default();
        for chunk in bounded(wants).chunks(MAX_TAPE_ITEMS) {
            let mut items = chunk.to_vec();
            loop {
                if stop() {
                    count.stopped = true;
                    return Ok(count);
                }
                count.requests += 1;
                let tape = match call(
                    &self.conn,
                    &Request::TapeFetch {
                        items: items.clone(),
                    },
                )? {
                    Answer::Tape(tape) => tape,
                    other => anyhow::bail!("the station answered {other:?} to a tape request"),
                };
                for piece in &tape.pieces {
                    let want = items
                        .get(piece.item as usize)
                        .ok_or_else(|| anyhow::anyhow!("a piece for item {}", piece.item))?;
                    let prints = decode_prints(&piece.prints)
                        .ok_or_else(|| anyhow::anyhow!("undecodable prints of {}", want.market))?;
                    count.pieces += 1;
                    count.prints += prints.len();
                    file(want, piece.from_ms, piece.to_ms, prints);
                }
                let Some(resume) = tape.resume else {
                    break;
                };
                let next = resume_from(&items, &resume);
                // A station that stops without getting further would loop this forever.
                anyhow::ensure!(
                    next != items && !next.is_empty(),
                    "the station's tape answer did not get further ({resume:?})"
                );
                items = next;
            }
        }
        Ok(count)
    }

    /// The archived traces of these trades of one core, each handed to `each` as it arrives — a
    /// later request failing keeps what came before; a trade the station holds none for is never
    /// handed.
    pub fn traces(
        &self,
        core_uid: u64,
        report_uids: &[i64],
        mut each: impl FnMut(TradeTraces),
    ) -> anyhow::Result<()> {
        let mut left = report_uids;
        while !left.is_empty() {
            let chunk = &left[..left.len().min(MAX_TRACE_UIDS)];
            let request = Request::TracesFetch {
                core_uid,
                report_uids: chunk.to_vec(),
            };
            let traces = match call(&self.conn, &request)? {
                Answer::Traces(traces) => traces,
                other => anyhow::bail!("the station answered {other:?} to a traces request"),
            };
            // An answer over its budget covers fewer than were asked; one covering none would
            // loop here forever.
            let answered = (traces.answered as usize).min(chunk.len());
            anyhow::ensure!(answered > 0, "the station's traces answer covered no trade");
            traces.trades.into_iter().for_each(&mut each);
            left = &left[answered..];
        }
        Ok(())
    }
}

/// Split every want past [`MAX_TAPE_SPANS`] stretches into several of the same market; empty
/// ones are dropped.
fn bounded(wants: Vec<TapeWant>) -> Vec<TapeWant> {
    let mut out = Vec::new();
    for want in wants {
        for spans in want.spans.chunks(MAX_TAPE_SPANS) {
            out.push(TapeWant {
                exchange: want.exchange.clone(),
                market: want.market.clone(),
                spans: spans.to_vec(),
            });
        }
    }
    out
}

/// The request that goes on from where an answer stopped: the items from `resume.item`, the
/// first of them cut to start at `resume.from_ms`.
pub(crate) fn resume_from(items: &[TapeWant], resume: &TapeResume) -> Vec<TapeWant> {
    let mut next: Vec<TapeWant> = items.iter().skip(resume.item as usize).cloned().collect();
    if let Some(first) = next.first_mut() {
        first.spans = first
            .spans
            .iter()
            .filter(|(_, to)| *to >= resume.from_ms)
            .map(|&(from, to)| (from.max(resume.from_ms), to))
            .collect();
        if first.spans.is_empty() {
            next.remove(0);
        }
    }
    next
}

#[cfg(test)]
mod tests;
