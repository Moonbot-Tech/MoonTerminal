//! Typed UI load state for Analytics profit queries.
//!
//! This module preserves the database's scalar-versus-split boundary without owning GPUI
//! entities, asynchronous work, or refresh sequencing.

use std::sync::Arc;

use moon_core::db::{FailKind, ProfitScope, ProfitUnit, QuoteBreakdown, ReadFail, ReadResult};

use crate::load_state::Note;

/// UI load state that preserves the database's profit-scope invariant.
pub(in crate::analytics) enum ProfitLoadState<T> {
    /// A request is in flight and no scalar unit has been verified yet.
    Loading,
    /// Comparable or legitimately empty scalar data and its optional exact unit.
    Ready {
        /// `None` belongs only to an empty query that has no currency to infer.
        unit: Option<ProfitUnit>,
        /// Current scalar payload.
        data: Arc<T>,
    },
    /// Raw money is unsafe as one scalar; only per-quote totals are retained.
    Split(QuoteBreakdown),
    /// The report replica or required schema is not available yet.
    NotReady,
    /// A classified read failure with no stale scalar payload.
    Failed(ReadFail),
}

impl<T> Default for ProfitLoadState<T> {
    /// Begin with a fresh unitless loading state.
    ///
    /// Returns:
    ///     Loading state without stale scalar data.
    fn default() -> Self {
        Self::Loading
    }
}

impl<T> ProfitLoadState<T> {
    /// Publish one typed database result without creating contradictory unit/split fields.
    ///
    /// Args:
    ///     result: Comparable, empty, split-only, or failed database result.
    ///
    /// Returns:
    ///     Nothing.
    pub(in crate::analytics) fn apply(&mut self, result: ReadResult<ProfitScope<T>>) {
        *self = match result {
            Ok(ProfitScope::Comparable { unit, data }) => Self::Ready {
                unit: Some(unit),
                data: Arc::new(data),
            },
            Ok(ProfitScope::Empty(data)) => Self::Ready {
                unit: None,
                data: Arc::new(data),
            },
            Ok(ProfitScope::Split(totals)) => Self::Split(totals),
            Err(ReadFail::NotReady) => Self::NotReady,
            Err(error) => Self::Failed(error),
        };
    }

    /// Borrow scalar data when the scope is comparable or legitimately empty.
    ///
    /// Returns:
    ///     Current scalar payload, or `None` for loading, split, and failed states.
    pub(in crate::analytics) fn data(&self) -> Option<&Arc<T>> {
        match self {
            Self::Ready { data, .. } => Some(data),
            Self::Loading | Self::Split(_) | Self::NotReady | Self::Failed(_) => None,
        }
    }

    /// Return scalar data or the exact placeholder for the current load outcome.
    ///
    /// Args:
    ///     empty: Predicate that classifies a successful scalar payload as empty.
    ///
    /// Returns:
    ///     Ready scalar data or a loading, empty, unavailable, split, or failure note.
    pub(in crate::analytics) fn view(
        &self,
        empty: impl FnOnce(&T) -> bool,
    ) -> Result<&Arc<T>, Note> {
        match self {
            Self::Loading => Err(Note::Loading),
            Self::Ready { data, .. } if empty(data) => Err(Note::Empty),
            Self::Ready { data, .. } => Ok(data),
            Self::Split(_) => Err(Note::IncomparableQuote),
            Self::NotReady => Err(Note::NotReady),
            Self::Failed(ReadFail::IncomparableQuote) => Err(Note::IncomparableQuote),
            Self::Failed(error) => Err(Note::Failed {
                msg: error.to_string().into(),
                kind: error.kind().unwrap_or(FailKind::Other),
            }),
        }
    }

    /// Exact unit carried by the ready scalar payload.
    ///
    /// Returns:
    ///     Quote currency or Percent, or `None` outside a comparable scope.
    pub(in crate::analytics) fn unit(&self) -> Option<ProfitUnit> {
        match self {
            Self::Ready { unit, .. } => *unit,
            Self::Loading | Self::Split(_) | Self::NotReady | Self::Failed(_) => None,
        }
    }

    /// Split totals retained for an incomparable raw-money scope.
    ///
    /// Returns:
    ///     Per-quote totals only for the split state.
    pub(in crate::analytics) fn split(&self) -> Option<&QuoteBreakdown> {
        match self {
            Self::Split(totals) => Some(totals),
            Self::Loading | Self::Ready { .. } | Self::NotReady | Self::Failed(_) => None,
        }
    }
}

/// Regression tests for profit-scope and read-failure publication.
#[cfg(test)]
mod tests;
