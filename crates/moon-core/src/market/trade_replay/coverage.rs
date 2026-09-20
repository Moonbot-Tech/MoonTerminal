//! The stretches of time a tick answer is exhaustive over — disjoint, ascending, inclusive
//! millisecond spans.
//!
//! One contiguous span was the whole story while a window's ticks were fetched around the position
//! as a single focus. A long position asks for ticks only around its entry and its exit, so what
//! the walk proves exhaustive is two stretches with bars between them, and the bars inside EACH
//! stretch are the ones to withhold — a hull over both would hide the middle's bars over ground
//! the points never reached. This type is that list, with the one invariant every reader leans on:
//! no two spans overlap or abut, so "inside a span" is a plain per-span test.

/// Disjoint, ascending, inclusive `(from_ms, to_ms)` spans. Empty means no coverage at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    spans: Vec<(i64, i64)>,
}

impl Coverage {
    /// No coverage: the series walked no ticks.
    pub const fn none() -> Self {
        Self { spans: Vec::new() }
    }

    /// One span, or none when it is inverted.
    pub fn one(span: (i64, i64)) -> Self {
        let mut out = Self::none();
        out.add(span);
        out
    }

    /// Every span of `spans`, merged by [`Self::add`].
    pub fn from_spans(spans: impl IntoIterator<Item = (i64, i64)>) -> Self {
        let mut out = Self::none();
        for span in spans {
            out.add(span);
        }
        out
    }

    /// Union one span in, coalescing with every span it overlaps or abuts (`to + 1 == from` on
    /// either side — tiles are cut that way, so two adjacent tiles read as one stretch).
    /// An inverted span adds nothing.
    pub fn add(&mut self, span: (i64, i64)) {
        let (mut from, mut to) = span;
        if from > to {
            return;
        }
        let mut merged = Vec::with_capacity(self.spans.len() + 1);
        let mut placed = false;
        for &(s_from, s_to) in &self.spans {
            let touches = s_from <= to.saturating_add(1) && s_to >= from.saturating_sub(1);
            if touches {
                from = from.min(s_from);
                to = to.max(s_to);
            } else if s_to < from {
                merged.push((s_from, s_to));
            } else {
                if !placed {
                    merged.push((from, to));
                    placed = true;
                }
                merged.push((s_from, s_to));
            }
        }
        if !placed {
            merged.push((from, to));
        }
        self.spans = merged;
    }

    /// The spans, ascending and disjoint.
    pub fn spans(&self) -> &[(i64, i64)] {
        &self.spans
    }

    /// Whether nothing is covered.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The outer bounds over every span, or `None` when empty. Ground BETWEEN two spans is not
    /// covered; the hull is for logging and for callers that only need an extent.
    pub fn hull(&self) -> Option<(i64, i64)> {
        match (self.spans.first(), self.spans.last()) {
            (Some(first), Some(last)) => Some((first.0, last.1)),
            _ => None,
        }
    }

    /// Whether `span` lies WHOLLY inside one covered span. An inverted `span` is inside nothing.
    pub fn contains(&self, span: (i64, i64)) -> bool {
        span.0 <= span.1
            && self
                .spans
                .iter()
                .any(|&(from, to)| span.0 >= from && span.1 <= to)
    }

    /// Whether the millisecond `at` lies inside a covered span.
    pub fn contains_ms(&self, at: i64) -> bool {
        self.contains((at, at))
    }

    /// Whether every span of `other` lies inside this coverage; an empty `other` is covered by
    /// anything, including nothing.
    pub fn covers(&self, other: &Coverage) -> bool {
        other.spans.iter().all(|&span| self.contains(span))
    }

    /// The part of this coverage inside `bounds`: each span intersected with each bound span.
    pub fn clip(&self, bounds: &Coverage) -> Coverage {
        let mut out = Self::none();
        for &(from, to) in &self.spans {
            for &(b_from, b_to) in &bounds.spans {
                out.add((from.max(b_from), to.min(b_to)));
            }
        }
        out
    }

    /// Total covered milliseconds, spans summed.
    pub fn width_ms(&self) -> i64 {
        self.spans
            .iter()
            .map(|&(from, to)| to.saturating_sub(from).saturating_add(1))
            .sum()
    }

    /// Whether the coverage is more than one stretch, whatever split it — a long position's two
    /// neighbourhoods, or a store that holds islands of one focus with unfetched ground between.
    pub fn is_split(&self) -> bool {
        self.spans.len() > 1
    }
}

impl std::fmt::Display for Coverage {
    /// `a..b`, `a..b+c..d`, or `-` for none — the form the tick stage logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.spans.is_empty() {
            return f.write_str("-");
        }
        for (index, (from, to)) in self.spans.iter().enumerate() {
            if index > 0 {
                f.write_str("+")?;
            }
            write!(f, "{from}..{to}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
