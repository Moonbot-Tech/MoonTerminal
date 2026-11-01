//! Which Core Status columns are identical across the cores that actually report them.
//!
//! A column leaves the table for the "same for all" line only when at least two cores carry
//! the same known value. An unknown reading, and any reading on a failed or dropped core, stays
//! out of that comparison: those cells are a dash, not a second opinion.

use super::model::{ApiKeyState, CoreStatusRow};
use super::presentation::{LoadLevel, api_expiry_text, api_quota_text, status_level};
use rust_i18n::t;

use super::time_offset::{
    TzOffsetCell, tz_offset_cell, tz_offset_cell_text, tz_offset_facts, tz_offset_tooltip,
};

/// One column that every reporting core agrees on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UniformFact {
    /// Flat column id, one of the keys in [`UNIFORM_KEYS`].
    pub(super) key: &'static str,
    /// The shared value, formatted the way that column's cell formats it.
    pub(super) text: String,
    /// Clock-offset hover, taken from one core that reported the shared offset.
    /// Other facts have none: their value is the whole statement.
    pub(super) tip: Option<String>,
}

/// Candidate columns, in "same for all" line order.
///
/// The strings are the Flat column ids from `table.rs::COLUMN_KEYS`.
const UNIFORM_KEYS: [&str; 4] = ["tz_off", "cpus", "api_key", "api_quota"];

/// Columns whose known values are identical across the visible cores.
///
/// A value is known when the core is not failed or dropped (`LoadLevel::Critical`) and the
/// field itself has been reported: a measured clock offset, a logical-CPU count, or a quota.
/// The API key is compared on the text its cells display, so it counts every
/// state other than [`ApiKeyState::Unknown`] (the dash), on a dropped core too. Fewer than two known
/// values is not agreement — the column stays in the table. Comparison uses the underlying
/// value. MoonBot builds always stay in each core's version cell, even when every build agrees.
///
/// Args:
///     rows: Visible core snapshots, in any order.
///
/// Returns:
///     One fact per uniform column, in [`UNIFORM_KEYS`] order. Empty when nothing qualifies.
pub(super) fn uniform_columns(rows: &[CoreStatusRow]) -> Vec<UniformFact> {
    UNIFORM_KEYS
        .into_iter()
        .filter_map(|key| uniform_one(key, rows))
        .collect()
}

/// Whether this core's cells show reported numbers rather than the offline dash.
///
/// Failed and dropped are the alarm colour ([`status_level`] returns [`LoadLevel::Critical`]).
/// A core that is still connecting keeps whatever it has already reported.
fn reports(row: &CoreStatusRow) -> bool {
    status_level(&row.status) != LoadLevel::Critical
}

fn uniform_one(key: &'static str, rows: &[CoreStatusRow]) -> Option<UniformFact> {
    match key {
        "tz_off" => {
            let mut fact = agree(rows, known_tz, key, |offset_secs| {
                tz_offset_cell_text(TzOffsetCell::Measured { offset_secs })
            })?;
            fact.tip = rows.iter().find_map(|row| {
                known_tz(row)?;
                Some(tz_offset_tooltip(&tz_offset_facts(&row.time_offset)))
            });
            Some(fact)
        }
        "cpus" => agree(rows, known_cpus, key, |count| count.to_string()),
        "api_key" => agree(rows, known_api, key, |state| match state {
            ApiKeyState::Perpetual => t!("core_status.same.no_expiry").to_string(),
            state => api_expiry_text(state),
        }),
        "api_quota" => agree(rows, known_quota, key, |quota| api_quota_text(Some(quota)))
            .or_else(|| no_quota(rows, key)),
        _ => None,
    }
}

/// The shared fact when at least two rows produce the same known value.
fn agree<T, K, F>(
    rows: &[CoreStatusRow],
    known: K,
    key: &'static str,
    text: F,
) -> Option<UniformFact>
where
    T: PartialEq,
    K: Fn(&CoreStatusRow) -> Option<T>,
    F: Fn(T) -> String,
{
    let mut values = rows.iter().filter_map(known);
    let first = values.next()?;
    let mut count = 1usize;
    for value in values {
        if value != first {
            return None;
        }
        count += 1;
    }
    (count >= 2).then(|| UniformFact {
        key,
        text: text(first),
        tip: None,
    })
}

/// "Not set" when at least two cores have answered and none of them publishes a quota.
///
/// The quota column already leaves the table while no row has a number. Stating it on the line
/// says the limit is absent rather than leaving the reader to wonder where the column went. A
/// core counts as answered once it reported its build: before that a missing quota is only
/// "not heard yet".
fn no_quota(rows: &[CoreStatusRow], key: &'static str) -> Option<UniformFact> {
    let mut reporting = rows
        .iter()
        .filter(|row| reports(row) && row.server_version.is_some());
    let none = reporting.clone().all(|row| row.api_quota.is_none());
    (none && reporting.nth(1).is_some()).then(|| UniformFact {
        key,
        text: t!("core_status.same.not_set").to_string(),
        tip: None,
    })
}

fn known_tz(row: &CoreStatusRow) -> Option<i32> {
    if !reports(row) {
        return None;
    }
    match tz_offset_cell(&row.time_offset) {
        TzOffsetCell::Measured { offset_secs } => Some(offset_secs),
        TzOffsetCell::Unknown => None,
    }
}

fn known_cpus(row: &CoreStatusRow) -> Option<u8> {
    if reports(row) {
        row.sys.logical_cpu_count
    } else {
        None
    }
}

/// The API-key state a row's cell displays, or `None` for the dash.
///
/// Unlike the telemetry columns, an offline core keeps its key text: expiry is not a live
/// sample, so `table.rs::api_key_cell` still prints it. A dated key on a dropped core is
/// therefore on screen and must keep the column.
fn known_api(row: &CoreStatusRow) -> Option<ApiKeyState> {
    match row.api_key {
        ApiKeyState::Unknown => None,
        state => Some(state),
    }
}

fn known_quota(row: &CoreStatusRow) -> Option<u64> {
    if reports(row) { row.api_quota } else { None }
}

#[cfg(test)]
mod tests;
