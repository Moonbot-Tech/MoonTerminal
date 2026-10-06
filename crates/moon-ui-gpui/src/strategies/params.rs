//! Right pane of the Strategies window: the parameter-pane model and renderer, including
//! selected-strategy badges/value editors (read-only YES/NO, input/memo, formula helper), the
//! per-section/full-mode body dispatch, and the full-value popover. The methods extend
//! `StrategiesView` from [`super`].

/// Parameter editors implementation.
mod editors;
/// Parameter formula implementation.
mod formula;
/// Parameter labels implementation.
mod labels;
/// Parameter list dialog implementation.
mod list_dialog;
/// Parameter model implementation.
mod model;
/// Parameter panel implementation.
mod panel;

use labels::field_keys;

use std::rc::Rc;

use super::actions::ListEditTarget;
use super::actions::list_edit::{ListEdit, is_list_field};
use super::param_entries::{self, FlatParams};
use super::versions::StagedOutcome;
use super::*;
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// The parameter pane's body content: one schema section or every surviving section in full mode.
///
/// `Rc` lets `full_params::full_params_list` move the flattened model into its retained row
/// factory without cloning its entries for each row.
pub(super) enum ParamsBody {
    Section(SchemaSection),
    Full(Rc<FlatParams>),
}

/// Return `v` up to its first newline, appending `…` when content follows it.
///
/// Used only for a compact full-mode row's memo preview and its version notes: a fixed row pitch
/// clips an embedded newline instead of wrapping it, and `.truncate()` alone only elides overflow
/// within one line.
fn compact_first_line(v: &str) -> String {
    match v.split_once('\n') {
        Some((first, _)) => format!("{first}…"),
        None => v.to_string(),
    }
}

// `Content` owns the prepared parameter body. Boxing it allocates on every selection change.
#[allow(clippy::large_enum_variant)]
pub(super) enum ParamsPanelModel {
    NoSelection,
    NoSchema,
    Content {
        /// Prepared per-section or full-mode body for the current selection.
        body: ParamsBody,
        values: Values,
        row_pairs: Vec<(Key, StrategyRow)>,
        multi: bool,
        common: Option<HashSet<String>>,
        differ: bool,
        /// Still-open strategy edit per selected key, cloned while `store` is in scope so the
        /// renderer (which needs `&mut self`/`cx.listener` and cannot hold a live store borrow)
        /// can resolve the pending value tier and the row marker without it.
        pending: HashMap<Key, StrategyEditRow>,
        /// Resolved-edit notes not yet acknowledged by each note's OWN core cursor
        /// (`StrategiesView::last_edit_note_seq`), for the `edit_state_banner` Adjusted/Superseded
        /// tiers. Also cloned here for the same store-borrow reason as `pending`. Paired with the
        /// core that produced each note: `StrategyEditNote` carries no core id of its own and
        /// strategy ids are core-local and repeat across cores, so flattening notes from more
        /// than one selected core without keeping this association would let a note from one
        /// core match a row on another.
        edit_notes: Vec<(CoreId, StrategyEditNote)>,
    },
}
