//! Fold a bare list of report `newrecid`s into the `(ranges, singles)` shape the soft-delete
//! protocol addresses rows by.
//!
//! The Report panel sends a hand-picked selection, which is small enough to travel as singles.
//! A strategy-scoped purge addresses that strategy's whole history — thousands of ids, almost all
//! of them consecutive — so folding runs into ranges is what keeps the payload from becoming
//! thousands of wire batches.

use moonproto::ReportRecIdRange;

/// Shortest run of consecutive ids worth spending a range on.
///
/// A range and two singles cost the same on the wire, so a pair stays a pair: a range is only
/// emitted where it actually removes ids from the payload. Keeping short runs as singles also
/// keeps the emitted ranges few and wide, which is easier to eyeball in a log line.
const MIN_RUN: usize = 3;

/// Compress report rec ids into inclusive ranges plus leftover singles.
///
/// The input is sorted and deduplicated in place first: the folding below reads neighbours, so an
/// unnormalized input would miss consecutive runs or emit duplicate wire entries.
///
/// # Invariant
///
/// The emitted ranges and singles together cover EXACTLY the input set — never a superset. Every
/// id in a range is an id the caller asked for. Widening a range to swallow a gap would soft-delete
/// trades belonging to another strategy, which the user never selected and cannot easily find
/// again to restore.
///
/// Args:
///     rec_ids: Report `newrecid`s to address; sorted and deduplicated in place.
///
/// Returns:
///     Inclusive ranges for every run of `MIN_RUN` or more consecutive ids, and the remaining ids
///     as singles. Both are empty for an empty input.
pub fn compress_rec_ids(rec_ids: &mut Vec<i64>) -> (Vec<ReportRecIdRange>, Vec<i64>) {
    rec_ids.sort_unstable();
    rec_ids.dedup();

    let mut ranges = Vec::new();
    let mut singles = Vec::new();
    let mut run_start = 0usize;
    for index in 0..rec_ids.len() {
        // A run ends at the last element, or where the next id is not the successor of this one.
        let ends_here = index + 1 == rec_ids.len() || rec_ids[index + 1] != rec_ids[index] + 1;
        if !ends_here {
            continue;
        }
        let run = &rec_ids[run_start..=index];
        if run.len() >= MIN_RUN {
            ranges.push(ReportRecIdRange::new(run[0], run[run.len() - 1]));
        } else {
            singles.extend_from_slice(run);
        }
        run_start = index + 1;
    }
    (ranges, singles)
}

#[cfg(test)]
mod tests;
