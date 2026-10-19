//! The only model-version adapter needed by the shared measurement harness.

use crate::db::tuner::ticks::{
    EntryParams, ModelSettings,
    params::{self, StrategyValues},
    search::SearchResult,
};

/// Model label recorded with every run.
pub(super) const MODEL: &str = "new";

/// Build the entry through this revision's production dispatcher.
pub(super) fn entry_of(kind: &str, sv: &StrategyValues<'_>, model: ModelSettings) -> EntryParams {
    params::entry_params_for(kind, sv, model)
}

/// Expose embargo accounting when this revision supplies it.
pub(super) fn purged_of(r: &SearchResult) -> Option<usize> {
    Some(r.purged)
}
