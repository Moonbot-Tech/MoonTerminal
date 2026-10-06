use super::*;

/// Read a cached rate outcome for one quote minute.
///
/// Args:
///     conn: Open valuation store.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     minute_utc: UTC minute start in Unix seconds.
///
/// Returns:
///     Cached conversion with provenance, or no cached rate.
pub(crate) fn cached_rate(
    conn: &Connection,
    quote_ordinal: i64,
    minute_utc: i64,
) -> rusqlite::Result<Option<ResolvedRate>> {
    conn.prepare_cached(
        "SELECT resolved_minute_utc, rate_usdt, price_basis, provider, symbol, orientation,
                candle_open_ms, candle_close_ms, leg1_rate, leg2_provider, leg2_symbol,
                leg2_orientation, leg2_rate
         FROM rates
         WHERE algorithm_version=?1 AND quote_ordinal=?2 AND minute_utc=?3",
    )?
    .query_row(
        params![ALGORITHM_VERSION, quote_ordinal, minute_utc],
        |row| decode_rate_row(row, quote_ordinal, minute_utc),
    )
    .optional()
}

/// Reuse a proven successor across another requested minute inside the same candle gap.
///
/// If an earlier request proved that its selected path had no observation until minute `R`, every
/// later request before `R` has the same first available observation. This turns a sparse-history
/// batch into one provider search per gap instead of one search per trade.
///
/// Args:
///     conn: Open valuation store.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     minute_utc: New requested UTC trade minute.
///
/// Returns:
///     Re-keyed successor provenance when a cached proof covers the requested minute.
pub(crate) fn covering_successor_rate(
    conn: &Connection,
    quote_ordinal: i64,
    minute_utc: i64,
) -> rusqlite::Result<Option<ResolvedRate>> {
    conn.prepare_cached(
        "SELECT resolved_minute_utc, rate_usdt, price_basis, provider, symbol, orientation,
                candle_open_ms, candle_close_ms, leg1_rate, leg2_provider, leg2_symbol,
                leg2_orientation, leg2_rate
         FROM rates
         WHERE algorithm_version=?1 AND quote_ordinal=?2 AND price_basis=?3
           AND minute_utc<?4 AND resolved_minute_utc>?4
         ORDER BY minute_utc DESC LIMIT 1",
    )?
    .query_row(
        params![
            ALGORITHM_VERSION,
            quote_ordinal,
            RatePriceBasis::SuccessorOpen.code(),
            minute_utc
        ],
        |row| decode_rate_row(row, quote_ordinal, minute_utc),
    )
    .optional()
}

/// Decode the shared persisted-rate projection into one provenance-rich result.
///
/// Args:
///     row: SQLite row using the canonical 13-column rate projection.
///     quote_ordinal: Persisted MoonBot quote ordinal supplied by the query key.
///     minute_utc: Requested UTC minute supplied by the query key.
///
/// Returns:
///     Decoded rate or a typed SQLite conversion failure.
fn decode_rate_row(
    row: &rusqlite::Row<'_>,
    quote_ordinal: i64,
    minute_utc: i64,
) -> rusqlite::Result<ResolvedRate> {
    let price_basis = match row.get::<_, i64>(2)? {
        0 => RatePriceBasis::ExactClose,
        1 => RatePriceBasis::SuccessorOpen,
        value => return Err(rusqlite::Error::IntegralValueOutOfRange(2, value)),
    };
    let orientation = decode_orientation(row.get::<_, i64>(5)?, 5)?;
    let leg2_orientation = row
        .get::<_, Option<i64>>(11)?
        .map(|value| decode_orientation(value, 11))
        .transpose()?;
    Ok(ResolvedRate {
        quote_ordinal,
        minute_utc,
        resolved_minute_utc: row.get(0)?,
        rate_usdt: row.get(1)?,
        price_basis,
        provider: row.get(3)?,
        symbol: row.get(4)?,
        orientation,
        candle_open_ms: row.get(6)?,
        candle_close_ms: row.get(7)?,
        leg1_rate: row.get(8)?,
        leg2_provider: row.get(9)?,
        leg2_symbol: row.get(10)?,
        leg2_orientation,
        leg2_rate: row.get(12)?,
    })
}

/// Decode one persisted rate orientation with a useful SQLite column index on failure.
fn decode_orientation(value: i64, column: usize) -> rusqlite::Result<RateOrientation> {
    match value {
        0 => Ok(RateOrientation::Direct),
        1 => Ok(RateOrientation::Inverse),
        2 => Ok(RateOrientation::Identity),
        value => Err(rusqlite::Error::IntegralValueOutOfRange(column, value)),
    }
}

/// Return the first historical minute that still needs provider search now.
///
/// Args:
///     conn: Open valuation store.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     minute_utc: Requested UTC trade minute.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     No minute while retry pacing is active; otherwise the requested minute for a new search or
///     the minute immediately after the last proven-empty horizon. An outage-only retry keeps
///     the original minute because no candle absence was established.
pub(crate) fn rate_search_start(
    conn: &Connection,
    quote_ordinal: i64,
    minute_utc: i64,
    now_ms: i64,
) -> rusqlite::Result<Option<i64>> {
    let state = conn
        .prepare_cached(
            "SELECT searched_through_minute, next_retry_at_ms FROM rate_searches
             WHERE algorithm_version=?1 AND quote_ordinal=?2 AND minute_utc=?3",
        )?
        .query_row(
            params![ALGORITHM_VERSION, quote_ordinal, minute_utc],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    Ok(match state {
        None => Some(minute_utc),
        Some((_, retry_at)) if retry_at > now_ms => None,
        Some((searched_through, _)) => Some(searched_through.saturating_add(60).max(minute_utc)),
    })
}

/// Read every historical rate key whose retry boundary is still in the future.
///
/// Args:
///     conn: Open valuation store.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     Quote/minute keys that the deferred worker must not request yet.
pub(crate) fn blocked_rate_searches(
    conn: &Connection,
    now_ms: i64,
) -> rusqlite::Result<std::collections::BTreeSet<(i64, i64)>> {
    let mut statement = conn.prepare(
        "SELECT quote_ordinal, minute_utc FROM rate_searches
         WHERE algorithm_version=?1 AND next_retry_at_ms>?2",
    )?;
    let blocked = statement
        .query_map(params![ALGORITHM_VERSION, now_ms], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect();
    blocked
}

/// Persist one successful immutable closed-minute conversion.
///
/// Args:
///     conn: Open valuation store.
///     rate: Validated conversion and provenance.
///     fetched_at_ms: Local fetch time in Unix milliseconds.
///
/// Returns:
///     Number of inserted or replaced rows.
pub(crate) fn store_rate(
    conn: &Connection,
    rate: &ResolvedRate,
    fetched_at_ms: i64,
) -> rusqlite::Result<usize> {
    let changed = conn
        .prepare_cached(
            "INSERT INTO rates (
             algorithm_version, quote_ordinal, minute_utc, resolved_minute_utc, rate_usdt,
             price_basis, provider, symbol, orientation, candle_open_ms, candle_close_ms,
             leg1_rate, leg2_provider, leg2_symbol, leg2_orientation, leg2_rate, fetched_at_ms
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
         ON CONFLICT (algorithm_version, quote_ordinal, minute_utc) DO UPDATE SET
             resolved_minute_utc=excluded.resolved_minute_utc,
             rate_usdt=excluded.rate_usdt, price_basis=excluded.price_basis,
             provider=excluded.provider, symbol=excluded.symbol,
             orientation=excluded.orientation, candle_open_ms=excluded.candle_open_ms,
             candle_close_ms=excluded.candle_close_ms, leg1_rate=excluded.leg1_rate,
             leg2_provider=excluded.leg2_provider, leg2_symbol=excluded.leg2_symbol,
             leg2_orientation=excluded.leg2_orientation, leg2_rate=excluded.leg2_rate,
             fetched_at_ms=excluded.fetched_at_ms",
        )?
        .execute(params![
            ALGORITHM_VERSION,
            rate.quote_ordinal,
            rate.minute_utc,
            rate.resolved_minute_utc,
            rate.rate_usdt,
            rate.price_basis.code(),
            rate.provider,
            rate.symbol,
            rate.orientation.code(),
            rate.candle_open_ms,
            rate.candle_close_ms,
            rate.leg1_rate,
            rate.leg2_provider,
            rate.leg2_symbol,
            rate.leg2_orientation.map(RateOrientation::code),
            rate.leg2_rate,
            fetched_at_ms,
        ])?;
    conn.prepare_cached(
        "DELETE FROM rate_searches
         WHERE algorithm_version=?1 AND quote_ordinal=?2 AND minute_utc=?3",
    )?
    .execute(params![
        ALGORITHM_VERSION,
        rate.quote_ordinal,
        rate.minute_utc
    ])?;
    Ok(changed)
}

/// Persist a retry boundary after every route lacks data through the current closed horizon.
///
/// Args:
///     conn: Open valuation store.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     minute_utc: UTC minute start in Unix seconds.
///     searched_through_minute: Latest fully closed minute checked by the resolver.
///     now_ms: Local verification time in Unix milliseconds.
///     grow: True for a genuine no-route result; a repeat search backs off by
///         `rate_search_retry_ms` once the search has covered an hour after the trade minute, and
///         only genuine no-route searches count as attempts. False keeps the flat 5-minute pace for
///         provider outages and transient prefetch gaps.
///
/// Returns:
///     Number of inserted or replaced rows.
pub(crate) fn store_rate_search(
    conn: &Connection,
    quote_ordinal: i64,
    minute_utc: i64,
    searched_through_minute: i64,
    now_ms: i64,
    grow: bool,
) -> rusqlite::Result<usize> {
    let existing: Option<(i64, i64)> = conn
        .prepare_cached(
            "SELECT attempts, searched_through_minute FROM rate_searches
             WHERE algorithm_version=?1 AND quote_ordinal=?2 AND minute_utc=?3",
        )?
        .query_row(
            params![ALGORITHM_VERSION, quote_ordinal, minute_utc],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let delay_ms = match existing {
        Some((attempts, prior_through))
            if grow && searched_through_minute.max(prior_through) - minute_utc >= 3_600 =>
        {
            rate_search_retry_ms(attempts)
        }
        _ => rate_search_retry_ms(0),
    };
    conn.prepare_cached(
        "INSERT INTO rate_searches (
             algorithm_version, quote_ordinal, minute_utc, searched_through_minute,
             next_retry_at_ms, attempts, updated_at_ms
         ) VALUES (?1,?2,?3,?4,?5,?7,?6)
         ON CONFLICT (algorithm_version, quote_ordinal, minute_utc) DO UPDATE SET
             searched_through_minute=MAX(rate_searches.searched_through_minute,
                                         excluded.searched_through_minute),
             next_retry_at_ms=excluded.next_retry_at_ms,
             attempts=rate_searches.attempts+?7,
             updated_at_ms=excluded.updated_at_ms",
    )?
    .execute(params![
        ALGORITHM_VERSION,
        quote_ordinal,
        minute_utc,
        searched_through_minute,
        now_ms.saturating_add(delay_ms),
        now_ms,
        i64::from(grow)
    ])
}

/// Delay before re-searching a minute that already had `prior_attempts` no-route searches.
///
/// Capped at one hour because a successor-open route can still appear later.
///
/// Args:
///     prior_attempts: Prior no-route searches persisted for the minute.
///
/// Returns:
///     Retry delay in milliseconds.
pub(crate) fn rate_search_retry_ms(prior_attempts: i64) -> i64 {
    const MINUTE_MS: i64 = 60 * 1_000;
    match prior_attempts {
        ..=0 => 5 * MINUTE_MS,
        1 => 10 * MINUTE_MS,
        2 => 20 * MINUTE_MS,
        3 => 40 * MINUTE_MS,
        _ => 60 * MINUTE_MS,
    }
}

/// Persist a prepared USDT valuation guarded by its complete source inputs.
///
/// Args:
///     conn: Open valuation store.
///     input: Current committed report values.
///     rate: Cached finite positive historical rate.
///     valued_at_ms: Local calculation time in Unix milliseconds.
///
/// Returns:
///     Number of inserted or replaced rows.
pub(crate) fn store_trade_value(
    conn: &Connection,
    input: &TradeInput,
    rate: &ResolvedRate,
    valued_at_ms: i64,
) -> rusqlite::Result<usize> {
    let profit_usdt = input.profit_quote * rate.rate_usdt;
    let spent_usdt = input.spent_quote.map(|spent| spent * rate.rate_usdt);
    conn.prepare_cached(
        "INSERT INTO trade_values (
             source_kind, core_uid, row_id, algorithm_version, closedate, quote_ordinal,
             profit_quote, spent_quote, rate_minute_utc, rate_usdt, profit_usdt,
             spent_usdt, valued_at_ms
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
         ON CONFLICT (source_kind, core_uid, row_id) DO UPDATE SET
             algorithm_version=excluded.algorithm_version, closedate=excluded.closedate,
             quote_ordinal=excluded.quote_ordinal, profit_quote=excluded.profit_quote,
             spent_quote=excluded.spent_quote, rate_minute_utc=excluded.rate_minute_utc,
             rate_usdt=excluded.rate_usdt, profit_usdt=excluded.profit_usdt,
             spent_usdt=excluded.spent_usdt, valued_at_ms=excluded.valued_at_ms
         WHERE trade_values.algorithm_version IS NOT excluded.algorithm_version
            OR trade_values.closedate IS NOT excluded.closedate
            OR trade_values.quote_ordinal IS NOT excluded.quote_ordinal
            OR trade_values.profit_quote IS NOT excluded.profit_quote
            OR trade_values.spent_quote IS NOT excluded.spent_quote
            OR trade_values.rate_minute_utc IS NOT excluded.rate_minute_utc
            OR trade_values.rate_usdt IS NOT excluded.rate_usdt
            OR trade_values.profit_usdt IS NOT excluded.profit_usdt
            OR trade_values.spent_usdt IS NOT excluded.spent_usdt",
    )?
    .execute(params![
        input.source.code(),
        input.core_uid,
        input.row_id,
        ALGORITHM_VERSION,
        input.closedate,
        input.quote_ordinal,
        input.profit_quote,
        input.spent_quote,
        rate.minute_utc,
        rate.rate_usdt,
        profit_usdt,
        spent_usdt,
        valued_at_ms,
    ])
}
