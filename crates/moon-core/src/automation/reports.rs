//! Bounded report queries reuse the report layer's time correction and currency accounting.

use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use serde_json::{Value, json};

/// Query an explicit authorized set. An empty set is rejected before the report API's all-core default.
pub fn query(
    cores: Vec<u64>,
    from_ms: i64,
    to_ms: i64,
    emulator: bool,
    offset: usize,
) -> Result<Value> {
    ensure!(
        !cores.is_empty() && cores.len() <= 64,
        "reports require 1..64 explicitly authorized cores"
    );
    ensure!(
        from_ms > 0 && to_ms > from_ms && to_ms - from_ms <= 366 * 86_400_000,
        "report interval must be positive and at most 366 days"
    );
    ensure!(
        offset <= 10_000,
        "narrow the report interval before paging beyond 10000 trades"
    );
    let conn = crate::db::open_reader().map_err(read_error)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    conn.progress_handler(10_000, Some(move || Instant::now() >= deadline))?;
    let snapshot = crate::db::read_snapshot(&conn).map_err(read_error)?;
    let axis = crate::db::ReportAxis::load(&snapshot, chrono_tz::UTC).map_err(read_error)?;
    let missing_offsets: Vec<_> = cores
        .iter()
        .filter(|id| axis.offset_secs(**id, to_ms / 1000).is_none())
        .copied()
        .collect();
    let filter = crate::db::ReportFilter {
        core_uids: cores,
        date_from: Some(from_ms / 1000),
        date_to: Some(to_ms / 1000),
        emulator: if emulator { None } else { Some(false) },
        rows: crate::db::RowScope::Closed,
        axis,
        ..Default::default()
    };
    let table =
        crate::db::query_mini_trades(&snapshot, &filter, offset + 101).map_err(read_error)?;
    let totals = crate::db::query_totals(&snapshot, &filter).map_err(read_error)?;
    let selected = [
        "buydate",
        "closedate",
        "id",
        "coin",
        "isshort",
        "quantity",
        "buyprice",
        "sellprice",
        "spentbtc",
        "gainedbtc",
        "profitbtc",
        "valuation_profit_usdt",
        "profitpct",
        "valuation_rate",
        "valuation_rate_source",
        "lev",
        "strategyid",
        "basecurrency",
        "emulator",
        "status",
        "sellreason",
    ];
    let rows: Vec<_> = table
        .rows
        .iter()
        .zip(&table.core_uids)
        .zip(&table.rec_ids)
        .skip(offset)
        .take(100)
        .map(|((row, core), record)| {
            let mut result = serde_json::Map::new();
            result.insert("core".into(), json!(core.to_string()));
            result.insert("record_id".into(), json!(record.to_string()));
            for (index, column) in table
                .cols
                .iter()
                .enumerate()
                .filter(|(_, c)| selected.contains(&c.as_str()))
            {
                let value = row.get(index).map(cell).unwrap_or(Value::Null);
                if matches!(column.as_str(), "buydate" | "closedate") {
                    let utc = value
                        .as_i64()
                        .filter(|seconds| *seconds > 0)
                        .filter(|seconds| filter.axis.offset_secs(*core, *seconds).is_some())
                        .map(|seconds| filter.axis.to_utc(seconds, *core).saturating_mul(1000));
                    result.insert(format!("{column}_utc_ms"), json!(utc));
                    result.insert(format!("{column}_core_local_seconds"), value);
                } else if column == "strategyid" {
                    result.insert(
                        column.clone(),
                        value
                            .as_i64()
                            .map(|id| json!((id as u64).to_string()))
                            .unwrap_or(value),
                    );
                } else if column == "id" {
                    result.insert(
                        column.clone(),
                        value
                            .as_i64()
                            .map(|id| json!(id.to_string()))
                            .unwrap_or(value),
                    );
                } else if column == "basecurrency" {
                    result.insert(
                        "currency".into(),
                        json!(
                            value
                                .as_i64()
                                .and_then(crate::db::QuoteCurrency::from_report_ordinal)
                                .map(|currency| currency.ticker())
                        ),
                    );
                } else {
                    result.insert(column.clone(), value);
                }
            }
            Value::Object(result)
        })
        .collect();
    let native: Vec<_> = totals
        .quotes
        .totals
        .iter()
        .map(|v| json!({"currency":v.currency.ticker(),"profit":v.profit,"trades":v.orders}))
        .collect();
    let coverage = totals.quotes.valuation.as_ref().map(|v| json!({"eligible":v.eligible_orders,"valued":v.valued_orders,"unavailable":v.unavailable_orders,
        "profit_usdt":v.usdt.as_ref().map(|total| total.profit)}));
    Ok(
        json!({"source":"Station report replica","from_ms":from_ms,"to_ms":to_ms,"time_zone":"UTC",
        "cores_without_clock_offset":missing_offsets.iter().map(u64::to_string).collect::<Vec<_>>(),"clock_caveat":"Report filters fall back to the core clock when its offset is unknown. Row UTC times are null without a measured offset at that instant; raw core-local seconds remain separate.",
        "rows":rows,"next_offset":offset+rows.len(),"has_more":table.rows.len()>offset+100,
        "total_trades":totals.quotes.orders,"native_profit":native,"unknown_currency_trades":totals.quotes.unknown_orders,"valuation":coverage,
        "read_at_ms":crate::util::now_unix_ms_i64(),"freshness":"Replicated history may still be downloading; compare each core's report_sync in station_overview."}),
    )
}

/// Blob columns never leak binary payloads into an AI response.
fn cell(value: &rusqlite::types::Value) -> Value {
    match value {
        rusqlite::types::Value::Null | rusqlite::types::Value::Blob(_) => Value::Null,
        rusqlite::types::Value::Integer(v) => json!(v),
        rusqlite::types::Value::Real(v) => json!(v),
        rusqlite::types::Value::Text(v) => json!(v),
    }
}

/// Preserve the report layer's classified failure in the model-visible tool error.
fn read_error(error: crate::db::ReadFail) -> anyhow::Error {
    anyhow::anyhow!("report read failed: {error}")
}

#[cfg(test)]
mod tests;
