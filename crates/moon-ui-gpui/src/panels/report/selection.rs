//! Stable Report row identity, mutation targets, and clipboard projection.
//!
//! The click and range arithmetic used to live here; it is `controls::row_selection` now, shared
//! with Core Status. `ReportSelection` is that helper keyed by [`ReportRowKey`].

use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;

use chrono_tz::Tz;
use moon_core::db::ReportAxis;
use moon_ui::{MoonDataTable, MoonDataTableState};
use rusqlite::types::Value;

use super::query::ReportData;
use super::{columns, export};
use crate::controls::row_selection::RowSelection;

/// Stable identity of one displayed report row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ReportRowKey {
    /// Typed report-replica row addressed by the MoonProto `newRecID` contract.
    Replicated { core_uid: u64, rec_id: i64 },
    /// Read-only legacy row addressed locally by the old per-core `db_id` alias `id`.
    Legacy { core_uid: u64, db_id: i64 },
}

/// Controlled Report multi-selection with one stable Shift-range anchor.
///
/// The click algorithm itself is [`RowSelection`], lifted into `controls::row_selection` and
/// shared with Core Status. What stays here is what only a REPORT row means: which selections
/// MoonProto command 48 can actually address.
pub(super) type ReportSelection = RowSelection<ReportRowKey>;

impl ReportSelection {
    /// Count selected rows addressable by MoonProto command 48.
    ///
    /// Returns:
    ///     Replicated selections with a protocol `newRecID`; legacy identities are excluded.
    pub(super) fn mutable_count(&self) -> usize {
        self.iter()
            .filter(|key| matches!(key, ReportRowKey::Replicated { .. }))
            .count()
    }

    /// Group selected replicated `newRecID` values by exact core.
    ///
    /// Args:
    ///     data: Current report query result with stable row identities.
    ///
    /// Returns:
    ///     Sorted record ids per core. Legacy rows are excluded because command 48 requires a
    ///     protocol `newRecID`; an older core that does not implement it simply sends no echo.
    pub(super) fn mutation_targets(&self, data: &ReportData) -> HashMap<u64, Vec<i64>> {
        let mut targets: HashMap<u64, Vec<i64>> = HashMap::new();
        for key in &data.row_keys {
            if !self.contains(*key) {
                continue;
            }
            if let Some(ReportRowKey::Replicated { core_uid, rec_id }) = key {
                targets.entry(*core_uid).or_default().push(*rec_id);
            }
        }
        for rec_ids in targets.values_mut() {
            rec_ids.sort_unstable();
            rec_ids.dedup();
        }
        targets
    }
}

/// Build stable identities parallel to one report query result.
///
/// Args:
///     cols: Runtime report columns.
///     rows: Generic report rows.
///     core_uids: Exact core id parallel to `rows`.
///     rec_ids: Typed `newRecID`, or zero for legacy rows.
///
/// Returns:
///     Stable typed or legacy keys. A malformed legacy row without integer `id` becomes `None`.
pub(super) fn row_keys(
    cols: &[String],
    rows: &[Vec<Value>],
    core_uids: &[u64],
    rec_ids: &[i64],
) -> Vec<Option<ReportRowKey>> {
    let id_index = cols.iter().position(|column| column == "id");
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let core_uid = core_uids.get(index).copied()?;
            let rec_id = rec_ids.get(index).copied()?;
            if rec_id > 0 {
                return Some(ReportRowKey::Replicated { core_uid, rec_id });
            }
            let db_id = id_index
                .and_then(|column| row.get(column))
                .and_then(integer_value)?;
            Some(ReportRowKey::Legacy { core_uid, db_id })
        })
        .collect()
}

/// Resolve prepared source indices through MoonDataTable's authoritative render ordering.
///
/// Args:
///     cols: Runtime report schema in source order.
///     source_indices: Contextually visible source indices in runtime-schema order.
///     state: Retained MoonDataTable drag order and widths.
///
/// Returns:
///     Visible source indices in the exact order used for table rendering.
pub(super) fn ordered_source_indices(
    cols: &[String],
    source_indices: &[usize],
    state: &MoonDataTableState,
) -> Vec<usize> {
    MoonDataTable::ordered_columns(
        columns::report_columns(cols, source_indices, &std::collections::HashMap::new()),
        state,
    )
    .iter()
    .filter_map(|column| cols.iter().position(|name| name == column.key.as_ref()))
    .collect()
}

/// Resolve a saved visible-name set through the prepared-index ordering helper in tests.
///
/// Args:
///     cols: Runtime report schema in source order.
///     visible: Current visible-column names.
///     state: Retained MoonDataTable drag order and widths.
///
/// Returns:
///     Visible source indices in the exact order used for table rendering.
#[cfg(test)]
pub(super) fn ordered_visible_indices(
    cols: &[String],
    visible: &HashSet<String>,
    state: &MoonDataTableState,
) -> Vec<usize> {
    let source_indices = cols
        .iter()
        .enumerate()
        .filter(|(_, column)| visible.contains(column.as_str()))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    ordered_source_indices(cols, &source_indices, state)
}

/// Build spreadsheet-friendly TSV for selected rows in current visual order.
///
/// Args:
///     data: Current report rows and stable identities.
///     cols: Runtime report schema.
///     indices: Visible column indices in visual order.
///     selection: Controlled selected row set.
///     axis: Time axis for replicated timestamp columns.
///     display_zone: User-selected zone for terminal-written timestamp columns.
///
/// Returns:
///     Header plus selected rows. Tabs and hard line breaks inside cells are flattened.
pub(super) fn selected_tsv(
    data: &ReportData,
    cols: &[String],
    indices: &[usize],
    selection: &ReportSelection,
    axis: &ReportAxis,
    display_zone: Tz,
) -> String {
    let mut lines = Vec::with_capacity(selection.len() + 1);
    lines.push(
        indices
            .iter()
            .filter_map(|index| cols.get(*index))
            .map(|column| tsv_cell(&columns::header_for(column)))
            .collect::<Vec<_>>()
            .join("\t"),
    );
    for (row_index, row) in data.rows.iter().enumerate() {
        if !selection.contains(data.row_keys.get(row_index).copied().flatten()) {
            continue;
        }
        // Resolved per ROW, not once per copy: a multi-row selection routinely spans cores on
        // different clocks, and one shared uid would correct every row by whichever core happened
        // to come first. It comes from the PARALLEL array rather than the row, because `core_uid`
        // is a service column the report schema does not carry -- the grid resolves it the same
        // way in `columns::data_row`.
        let core_uid = data.core_uids.get(row_index).copied().unwrap_or(0);
        lines.push(
            indices
                .iter()
                .filter_map(|index| {
                    let column = cols.get(*index)?;
                    let value = row.get(*index).unwrap_or(&Value::Null);
                    Some(tsv_cell(&export::field_text(
                        column,
                        value,
                        axis,
                        core_uid,
                        display_zone,
                    )))
                })
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    lines.join("\r\n")
}

/// Convert one SQLite number to a stable signed identifier.
///
/// Args:
///     value: SQLite value from a legacy `id` column.
///
/// Returns:
///     The integer identity, accepting SQLite real storage for historical databases.
fn integer_value(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value),
        Value::Real(value) => Some(*value as i64),
        _ => None,
    }
}

/// Flatten delimiter characters that would split one clipboard cell into extra rows or columns.
///
/// Args:
///     value: Formatted report value for one clipboard cell.
///
/// Returns:
///     Single-line text without TSV row or column delimiters.
fn tsv_cell(value: &str) -> String {
    value
        .replace('\t', " ")
        .replace("\r\n", " ")
        .replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests;
