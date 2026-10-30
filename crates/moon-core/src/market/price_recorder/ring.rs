//! The recent past of every market an exchange's list carries a mark for: one short ring per
//! market, cut by time rather than by count, so a trade that opens now still finds the minutes
//! before its entry.
//!
//! Only changes are kept. A row equal to the market's last point adds nothing: between two
//! points the market stood still, and the older point is its state until the newer one. That is
//! also why trimming keeps the newest point at or before the cut — it is the state from the cut
//! onwards.
//!
//! Whether the source was listening at all is kept apart, per exchange ([`ExchangeRing::valid`]):
//! "no new point" means "unchanged" only while lists keep arriving.

use std::collections::{HashMap, VecDeque};

use crate::market::trade_replay::Coverage;

/// A list arriving later than this after the previous one leaves a gap: the stretch between
/// them is not known to be unchanged. The core sends its list about every two seconds.
pub(super) const GAP_MS: i64 = 8_000;

/// One market's prices as one list carried them at `t_ms` (terminal UTC).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub t_ms: i64,
    pub bid: f64,
    pub ask: f64,
    pub mark: f64,
}

/// Funding as the list carried it: the rate and the next charge's wire time, Unix milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Funding {
    pub t_ms: i64,
    pub rate: f64,
    pub next_ms: i64,
}

/// What one list carried for one market that has a mark.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub market: String,
    pub bid: f64,
    pub ask: f64,
    pub mark: f64,
    /// `(rate, next charge ms)` when the list carried funding at all.
    pub funding: Option<(f64, i64)>,
}

/// The points of one market inside one stretch, ready to be filed.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Segment {
    /// First millisecond the segment speaks for; its first point is the state at this moment.
    pub from_ms: i64,
    /// Last millisecond the segment speaks for.
    pub to_ms: i64,
    pub points: Vec<Point>,
    pub funding: Vec<Funding>,
}

/// Bitwise equality, so a NaN the core might send compares equal to itself and is not re-added.
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

/// One market's changes over the ring's depth.
#[derive(Default)]
pub(super) struct MarketRing {
    points: VecDeque<Point>,
    funding: VecDeque<Funding>,
}

impl MarketRing {
    /// Add what one list carried; returns whether the prices changed.
    ///
    /// A stamp behind the last point (two sources one after the other) is lifted to it, so the
    /// ring stays ordered.
    fn push(&mut self, t_ms: i64, row: &Row) -> bool {
        let t_ms = self.points.back().map_or(t_ms, |p| t_ms.max(p.t_ms));
        if let Some((rate, next_ms)) = row.funding {
            let changed = self
                .funding
                .back()
                .is_none_or(|f| !same(f.rate, rate) || f.next_ms != next_ms);
            if changed {
                let t_ms = self.funding.back().map_or(t_ms, |f| t_ms.max(f.t_ms));
                self.funding.push_back(Funding {
                    t_ms,
                    rate,
                    next_ms,
                });
            }
        }
        let changed = self.points.back().is_none_or(|p| {
            !same(p.bid, row.bid) || !same(p.ask, row.ask) || !same(p.mark, row.mark)
        });
        if changed {
            self.points.push_back(Point {
                t_ms,
                bid: row.bid,
                ask: row.ask,
                mark: row.mark,
            });
        }
        changed
    }

    /// Drop what is older than `cutoff_ms`, keeping the newest point at or before it: that point
    /// is the market's state from the cut onwards.
    fn trim(&mut self, cutoff_ms: i64) {
        while self.points.len() >= 2 && self.points[1].t_ms <= cutoff_ms {
            self.points.pop_front();
        }
        while self.funding.len() >= 2 && self.funding[1].t_ms <= cutoff_ms {
            self.funding.pop_front();
        }
    }

    /// The points that speak for `[from_ms, to_ms]`: the state at `from_ms` stamped `from_ms`,
    /// then every change up to `to_ms`. A market first seen inside the stretch starts at its first
    /// point. `None` when nothing is known of the market up to `to_ms`.
    fn segment(&self, from_ms: i64, to_ms: i64) -> Option<Segment> {
        let first_after = self.points.partition_point(|p| p.t_ms <= from_ms);
        let end = self.points.partition_point(|p| p.t_ms <= to_ms);
        let mut points = Vec::with_capacity(end.saturating_sub(first_after) + 1);
        let from_ms = match first_after.checked_sub(1) {
            Some(at) => {
                points.push(Point {
                    t_ms: from_ms,
                    ..self.points[at]
                });
                from_ms
            }
            None => {
                self.points
                    .get(first_after)
                    .filter(|p| p.t_ms <= to_ms)?
                    .t_ms
            }
        };
        points.extend(self.points.range(first_after..end).copied());
        let f_after = self.funding.partition_point(|f| f.t_ms <= from_ms);
        let f_end = self.funding.partition_point(|f| f.t_ms <= to_ms);
        let mut funding = Vec::new();
        if let Some(at) = f_after.checked_sub(1) {
            funding.push(Funding {
                t_ms: from_ms,
                ..self.funding[at]
            });
        }
        funding.extend(self.funding.range(f_after..f_end).copied());
        Some(Segment {
            from_ms,
            to_ms,
            points,
            funding,
        })
    }

    fn len(&self) -> usize {
        self.points.len() + self.funding.len()
    }
}

/// One unbroken run of lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    first: i64,
    last: i64,
    /// A list after `last` was lost: the run vouches for nothing past its last list.
    cut: bool,
}

impl Run {
    /// The last millisecond the run vouches for.
    fn until(self) -> i64 {
        if self.cut {
            self.last
        } else {
            self.last.saturating_add(GAP_MS)
        }
    }
}

/// Every market of one exchange, and when its lists arrived.
#[derive(Default)]
pub(super) struct ExchangeRing {
    markets: HashMap<String, MarketRing>,
    /// Each unbroken run of lists, oldest first.
    alive: VecDeque<Run>,
}

impl ExchangeRing {
    /// Take one list stamped `t_ms`. `broken` says a list of this exchange was lost on its way
    /// here: the run before ends at its last list, and a new one starts. Returns how many points
    /// were added.
    pub(super) fn apply(&mut self, t_ms: i64, rows: Vec<Row>, broken: bool) -> usize {
        match self.alive.back_mut() {
            Some(run) if !broken && t_ms.saturating_sub(run.last) <= GAP_MS => {
                run.last = run.last.max(t_ms);
            }
            last => {
                if let Some(run) = last.filter(|_| broken) {
                    run.cut = true;
                }
                self.alive.push_back(Run {
                    first: t_ms,
                    last: t_ms,
                    cut: false,
                });
            }
        }
        let mut added = 0;
        for row in rows {
            let changed = match self.markets.get_mut(row.market.as_str()) {
                Some(ring) => ring.push(t_ms, &row),
                None => {
                    let mut ring = MarketRing::default();
                    let changed = ring.push(t_ms, &row);
                    self.markets.insert(row.market, ring);
                    changed
                }
            };
            added += usize::from(changed);
        }
        added
    }

    /// The stretches up to `horizon_ms` the lists vouch for: each run, from its first list to
    /// [`GAP_MS`] past its last — or to its last, when a list after it was lost.
    pub(super) fn valid(&self, horizon_ms: i64) -> Coverage {
        Coverage::from_spans(
            self.alive
                .iter()
                .map(|run| (run.first, run.until().min(horizon_ms))),
        )
    }

    /// Forget what is older than `cutoff_ms`.
    pub(super) fn trim(&mut self, cutoff_ms: i64) {
        for ring in self.markets.values_mut() {
            ring.trim(cutoff_ms);
        }
        while self
            .alive
            .front()
            .is_some_and(|run| run.until() < cutoff_ms)
        {
            self.alive.pop_front();
        }
    }

    /// One market's points for `[from_ms, to_ms]`, see [`MarketRing::segment`].
    pub(super) fn segment(&self, market: &str, from_ms: i64, to_ms: i64) -> Option<Segment> {
        self.markets.get(market)?.segment(from_ms, to_ms)
    }

    /// Markets held and points over all of them, for the summary line.
    pub(super) fn size(&self) -> (usize, usize) {
        (
            self.markets.len(),
            self.markets.values().map(MarketRing::len).sum(),
        )
    }
}

#[cfg(test)]
mod tests;
