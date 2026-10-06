use super::*;

/// Attach an existing valuation store to a report reader.
///
/// Missing derived storage is a normal `Ok(false)` while startup initializes it. An existing file
/// that cannot attach is a classified read failure rather than zero conversion coverage.
///
/// Args:
///     conn: Report connection before its read snapshot begins.
///
/// Returns:
///     Whether the valuation schema was attached.
pub(in crate::db) fn attach(conn: &Connection) -> ReadResult<bool> {
    if !cache_is_healthy() {
        return Ok(false);
    }
    let path = crate::config::paths::valuation_db_path();
    attach_store(conn, &path)
}

/// Attach one explicit valuation store after its filesystem and schema checks pass.
///
/// Args:
///     conn: Report connection before its read snapshot begins.
///     path: Canonical valuation path or isolated fixture equivalent.
///
/// Returns:
///     Whether the valuation schema was attached.
pub(in crate::db::valuation) fn attach_store(conn: &Connection, path: &Path) -> ReadResult<bool> {
    let _lifecycle = CACHE_LIFECYCLE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match std::fs::metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(read_fail::io_fail(
                "valuation: database metadata",
                path,
                &error,
            ));
        }
    }
    let uri = sqlite_read_only_uri(path);
    let sql = format!("ATTACH DATABASE '{}' AS {SCHEMA}", uri.replace('\'', "''"));
    if let Err(error) = conn.execute(&sql, []) {
        if prove_detached_store_corruption(conn, path, &error) {
            mark_unhealthy(&error);
            return Ok(false);
        }
        return Err(read_fail::read_fail_at("valuation: attach", path, error));
    }
    match validate_attachment(conn) {
        Ok(()) => Ok(true),
        Err(error) if prove_derived_corruption(conn, &error) => {
            let _ = conn.execute(&format!("DETACH DATABASE {SCHEMA}"), []);
            Ok(false)
        }
        Err(error) => Err(read_fail::read_fail_at(
            "valuation: validate attachment",
            path,
            error,
        )),
    }
}

/// Encode one Windows path as a read-only SQLite file URI for reader attachment.
///
/// Args:
///     path: Existing valuation database path.
///
/// Returns:
///     SQLite URI with reserved path characters escaped and write access disabled.
pub(in crate::db::valuation) fn sqlite_read_only_uri(path: &Path) -> String {
    let escaped = path
        .to_string_lossy()
        .replace('%', "%25")
        .replace('?', "%3F")
        .replace('#', "%23")
        .replace('\\', "/");
    format!("file:{escaped}?mode=ro")
}

/// Test whether a connection currently carries the validated valuation attachment.
///
/// Args:
///     conn: Report reader or read snapshot that may carry the attachment.
///
/// Returns:
///     `true` when startup attachment succeeded and the cache has not since been disabled.
pub(in crate::db) fn is_attached(conn: &Connection) -> bool {
    if !cache_is_healthy() {
        return false;
    }
    conn.query_row(
        "SELECT 1 FROM pragma_database_list WHERE name = ?1 LIMIT 1",
        [SCHEMA],
        |_| Ok(()),
    )
    .is_ok()
}

/// Execute the complete reader-facing valuation schema contract.
///
/// Args:
///     conn: Report reader with a candidate `valuation` attachment.
///
/// Returns:
///     Success for readable current tables, including empty ones.
pub(in crate::db::valuation) fn validate_attachment(conn: &Connection) -> rusqlite::Result<()> {
    validate_schema_with_prefix(conn, "valuation.")
}

/// Execute the reader-facing valuation schema contract with one table-name prefix.
///
/// Args:
///     conn: Direct valuation connection or report reader carrying an attachment.
///     prefix: Empty for a direct connection or `valuation.` for an attachment.
///
/// Returns:
///     Success after both tables and their first reachable rows are readable.
pub(in crate::db::valuation) fn validate_schema_with_prefix(
    conn: &Connection,
    prefix: &str,
) -> rusqlite::Result<()> {
    let probes = [
        format!(
            "SELECT algorithm_version, quote_ordinal, minute_utc, resolved_minute_utc,
                    rate_usdt, price_basis, provider, symbol, orientation, candle_open_ms,
                    candle_close_ms, leg1_rate, leg2_provider, leg2_symbol,
                    leg2_orientation, leg2_rate
             FROM {prefix}rates LIMIT 1"
        ),
        format!(
            "SELECT algorithm_version, quote_ordinal, minute_utc, searched_through_minute,
                    next_retry_at_ms, attempts, updated_at_ms
             FROM {prefix}rate_searches LIMIT 1"
        ),
        format!(
            "SELECT source_kind, core_uid, row_id, algorithm_version, closedate,
                    quote_ordinal, profit_quote, spent_quote, rate_minute_utc,
                    rate_usdt, profit_usdt, spent_usdt
             FROM {prefix}trade_values LIMIT 1"
        ),
    ];
    for sql in probes {
        match conn.query_row(&sql, [], |_| Ok(())) {
            Ok(()) | Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(error) => return Err(error),
        }
    }
    validate_primary_key(
        conn,
        prefix,
        "rates",
        &["algorithm_version", "quote_ordinal", "minute_utc"],
    )?;
    validate_primary_key(
        conn,
        prefix,
        "rate_searches",
        &["algorithm_version", "quote_ordinal", "minute_utc"],
    )?;
    validate_primary_key(
        conn,
        prefix,
        "trade_values",
        &["source_kind", "core_uid", "row_id"],
    )?;
    Ok(())
}

/// Require one table to retain the exact primary key assumed by valuation joins and upserts.
///
/// Args:
///     conn: Direct valuation connection or report reader carrying an attachment.
///     prefix: Empty for a direct connection or `valuation.` for an attachment.
///     table: Valuation table whose key contract is checked.
///     expected: Primary-key column names in key order.
///
/// Returns:
///     Success only when SQLite reports the complete expected primary key.
fn validate_primary_key(
    conn: &Connection,
    prefix: &str,
    table: &str,
    expected: &[&str],
) -> rusqlite::Result<()> {
    let schema = prefix.trim_end_matches('.');
    let sql = if schema.is_empty() {
        format!("PRAGMA table_info({table})")
    } else {
        format!("PRAGMA {schema}.table_info({table})")
    };
    let mut statement = conn.prepare(&sql)?;
    let mut columns = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })?
        .filter_map(|column| match column {
            Ok((name, position)) if position > 0 => Some(Ok((position, name))),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    columns.sort_by_key(|(position, _)| *position);
    if columns
        .iter()
        .map(|(_, name)| name.as_str())
        .eq(expected.iter().copied())
    {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

/// Prove that a corruption error came from the attached derived cache, not `main`.
///
/// Both schema checks are explicit. Any inability to prove a healthy report main database returns
/// `false`, preserving the existing fail-closed report integrity path.
///
/// Args:
///     conn: Report reader carrying the candidate valuation attachment.
///     error: Corruption-class failure from a statement that may reference valuation tables.
///
/// Returns:
///     `true` only when `main` checks healthy and `valuation` checks damaged.
pub(crate) fn prove_derived_corruption(conn: &Connection, error: &rusqlite::Error) -> bool {
    if !is_corruption(error) {
        return false;
    }
    if !main_is_healthy(conn) {
        return false;
    }
    let valuation = conn.query_row("PRAGMA valuation.quick_check(1)", [], |row| {
        row.get::<_, String>(0)
    });
    let damaged = match valuation {
        Ok(result) => result != "ok",
        Err(check_error) => is_corruption(&check_error),
    };
    if damaged {
        mark_unhealthy(error);
    }
    damaged
}
