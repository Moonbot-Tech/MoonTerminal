//! Stable Report row selection, range arithmetic, mutation targets, and clipboard projection.

use std::collections::{HashMap, HashSet};

use moon_ui::{MoonDataTable, MoonDataTableState};
use rusqlite::types::Value;

use super::query::ReportData;
use super::{columns, export};

/// Stable identity of one displayed report row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ReportRowKey {
    /// Typed report-replica row addressed by the MoonProto `newRecID` contract.
    Replicated { core_uid: u64, rec_id: i64 },
    /// Read-only legacy row addressed locally by the old per-core `db_id` alias `id`.
    Legacy { core_uid: u64, db_id: i64 },
}

/// Controlled Report multi-selection with one stable Shift-range anchor.
#[derive(Clone, Default)]
pub(super) struct ReportSelection {
    selected: HashSet<ReportRowKey>,
    anchor: Option<ReportRowKey>,
}

impl ReportSelection {
    /// Select every valid row identity in the current table without changing the Shift anchor.
    ///
    /// Args:
    ///     order: Current filtered and sorted row identities, including read-only legacy rows.
    ///
    /// Returns:
    ///     Nothing. Malformed rows without a stable identity are excluded.
    pub(super) fn select_all(&mut self, order: &[Option<ReportRowKey>]) {
        self.selected = order.iter().filter_map(|key| *key).collect();
    }

    /// Apply one row click using platform-independent modifier meaning.
    ///
    /// Args:
    ///     clicked: Stable identity of the clicked row, or `None` for an invalid legacy row.
    ///     order: Current rendered row identities in visual order.
    ///     shift: Whether Shift was held.
    ///     secondary: Whether Ctrl on Windows/Linux or Command on macOS was held.
    ///
    /// Returns:
    ///     Nothing. Shift takes precedence over the secondary modifier.
    pub(super) fn click(
        &mut self,
        clicked: Option<ReportRowKey>,
        order: &[Option<ReportRowKey>],
        shift: bool,
        secondary: bool,
    ) {
        let Some(clicked) = clicked else {
            return;
        };
        if shift {
            let span = self.anchor.and_then(|anchor| {
                let from = order.iter().position(|key| *key == Some(anchor))?;
                let to = order.iter().position(|key| *key == Some(clicked))?;
                Some(if from <= to { from..=to } else { to..=from })
            });
            self.selected.clear();
            if let Some(span) = span {
                self.selected
                    .extend(order[span].iter().filter_map(|key| *key));
            } else {
                self.selected.insert(clicked);
                self.anchor = Some(clicked);
            }
            return;
        }
        self.anchor = Some(clicked);
        if secondary {
            if !self.selected.insert(clicked) {
                self.selected.remove(&clicked);
            }
        } else {
            self.selected.clear();
            self.selected.insert(clicked);
        }
    }

    /// Remove selections no longer present in a newly published query result.
    ///
    /// Args:
    ///     visible: Stable row identities in the new result.
    ///
    /// Returns:
    ///     Nothing. A missing anchor is cleared with its vanished row.
    pub(super) fn retain_visible(&mut self, visible: &[Option<ReportRowKey>]) {
        let visible: HashSet<ReportRowKey> = visible.iter().filter_map(|key| *key).collect();
        self.selected.retain(|key| visible.contains(key));
        if self.anchor.is_some_and(|key| !visible.contains(&key)) {
            self.anchor = None;
        }
    }

    /// Clear every selected row and the Shift anchor.
    ///
    /// Returns:
    ///     Nothing after selection state becomes empty.
    pub(super) fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /// Return whether one stable row is selected.
    ///
    /// Args:
    ///     key: Stable row identity, or `None` for an unselectable malformed row.
    ///
    /// Returns:
    ///     `true` only when a concrete identity belongs to the controlled set.
    pub(super) fn contains(&self, key: Option<ReportRowKey>) -> bool {
        key.is_some_and(|key| self.selected.contains(&key))
    }

    /// Return the number of selected rows.
    ///
    /// Returns:
    ///     Current controlled selection size.
    pub(super) fn len(&self) -> usize {
        self.selected.len()
    }

    /// Count selected rows addressable by MoonProto command 48.
    ///
    /// Returns:
    ///     Replicated selections with a protocol `newRecID`; legacy identities are excluded.
    pub(super) fn mutable_count(&self) -> usize {
        self.selected
            .iter()
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

/// Resolve visible source indices through MoonDataTable's authoritative render ordering.
///
/// Args:
///     cols: Runtime report schema in source order.
///     visible: Current visible-column names.
///     state: Retained MoonDataTable drag order and widths.
///
/// Returns:
///     Visible source indices in the exact order used for table rendering.
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
    MoonDataTable::ordered_columns(columns::report_columns(cols, &source_indices), state)
        .iter()
        .filter_map(|column| cols.iter().position(|name| name == column.key.as_ref()))
        .collect()
}

/// Build spreadsheet-friendly TSV for selected rows in current visual order.
///
/// Args:
///     data: Current report rows and stable identities.
///     cols: Runtime report schema.
///     indices: Visible column indices in visual order.
///     selection: Controlled selected row set.
///
/// Returns:
///     Header plus selected rows. Tabs and hard line breaks inside cells are flattened.
pub(super) fn selected_tsv(
    data: &ReportData,
    cols: &[String],
    indices: &[usize],
    selection: &ReportSelection,
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
        lines.push(
            indices
                .iter()
                .filter_map(|index| {
                    let column = cols.get(*index)?;
                    let value = row.get(*index).unwrap_or(&Value::Null);
                    Some(tsv_cell(&export::field_text(column, value)))
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
