use super::*;

/// Stable in-memory identity for one deferred prepared value.
///
/// Args:
///     input: Current report inputs.
///
/// Returns:
///     Source-kind/core/row tuple.
pub(in crate::db::valuation::worker) fn trade_key(input: &TradeInput) -> (i64, i64, i64) {
    (input.source.code(), input.core_uid, input.row_id)
}

/// Publish one valuation-data generation and coalescing UI edge.
///
/// Args:
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
pub(in crate::db::valuation::worker) fn publish(generation: &AtomicU64, dirty: &AtomicBool) {
    generation.fetch_add(1, Ordering::AcqRel);
    dirty.store(true, Ordering::Release);
}

/// Refresh the worker's axis from a report reader it already has open.
///
/// FAILS the stage rather than falling back to the identity axis. On a core running behind UTC the
/// identity axis picks the wrong hour's spot rate, so an unreadable measurement must stop the
/// valuation pass, not quietly value the trades at a price they never traded at. A stage that
/// cannot read the replica is already a stage that has nothing to reconcile.
///
/// Args:
///     conn: Report reader this stage opened.
///     axis: The worker's axis, replaced in place.
///
/// Returns:
///     Nothing, or the classified read failure that must stop this stage.
pub(in crate::db::valuation::worker) fn refresh_axis(
    conn: &Connection,
    axis: &mut ReportAxis,
) -> Result<(), FaultCause> {
    // Loaded at UTC on purpose, and NOT at the axis's own current zone. A display zone is a
    // rendering concern and this worker renders nothing: it needs the per-core OFFSETS and nothing
    // else. Asking for the zone here would also put a second `axis.` call in this file, which the
    // never-routed contract test counts — and it counts it precisely so that a new route through
    // the axis has to be looked at rather than absorbed.
    *axis = ReportAxis::load(conn, chrono_tz::Tz::UTC).map_err(report_fault)?;
    Ok(())
}

/// Resolve the spot-rate minute one trade values at.
///
/// The ONE place a report row's `closedate` becomes a rate key. The stored value is core-local
/// wall clock while the rate series is true UTC, so the axis converts before the minute is
/// floored — flooring first would round on the wrong side of the boundary for an offset that is
/// not a whole number of minutes. On the identity axis this is exactly the raw floor it replaced.
///
/// Args:
///     axis: Per-core time axis the stored timestamp is corrected by.
///     input: Complete current report inputs for one trade.
///
/// Returns:
///     Start of the trade's minute in true UTC seconds.
pub(in crate::db::valuation::worker) fn valuation_minute(
    axis: &ReportAxis,
    input: &TradeInput,
) -> i64 {
    axis.to_utc(input.closedate, input.core_uid as u64)
        .div_euclid(60)
        * 60
}

/// Current UTC minute start in Unix seconds.
///
/// Returns:
///     Wall-clock minute boundary.
pub(in crate::db::valuation::worker) fn current_minute_utc() -> i64 {
    now_unix_ms_i64().div_euclid(60_000) * 60
}

/// Next UTC minute boundary plus a small close-publication margin.
///
/// The one definition of the boundary: [`delay_to_next_minute`] derives from it for the plain
/// retry delay, and [`until_next_minute`] caps a park by the same instant.
///
/// Args:
///     now_ms: Current wall clock in Unix milliseconds.
///
/// Returns:
///     Unix milliseconds of the next minute boundary after the close-publication margin.
pub(in crate::db::valuation::worker) fn next_minute_deadline_ms(now_ms: i64) -> i64 {
    (now_ms.div_euclid(60_000) + 1) * 60_000 + 250
}

/// Delay anchored to the next UTC minute boundary plus a small close-publication margin.
///
/// Returns:
///     Positive wait duration that does not drift from process start.
pub(in crate::db::valuation::worker) fn delay_to_next_minute() -> Duration {
    let now = now_unix_ms_i64();
    let next = next_minute_deadline_ms(now);
    Duration::from_millis(next.saturating_sub(now).max(1) as u64)
}
