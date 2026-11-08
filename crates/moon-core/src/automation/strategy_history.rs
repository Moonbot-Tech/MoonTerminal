//! `strategy_versions`: a strategy's saved history from the archive, read-only.
//!
//! The archive is the one the Station keeps in step with every terminal, so its versions start
//! when a terminal or the Station first saw the strategy; `history_since` says when that was.

use anyhow::{Context, Result, ensure};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Arguments of `strategy_versions`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub core: String,
    pub strategy_id: String,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    /// Instead of the list: the version in effect at this instant, with its values.
    pub at_ms: Option<i64>,
    /// With `at_ms`: only these fields, matched ignoring case.
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub offset: usize,
}

/// Versions one page holds.
const PAGE: usize = 50;
/// A changed value longer than this is shown as its length; coin lists run to kilobytes.
const LONG_VALUE: usize = 160;

/// Run `strategy_versions`.
///
/// Errors:
///     A bad id, an unreadable archive.
pub fn run(args: Args) -> Result<Value> {
    let core: u64 = args.core.parse().context("core must be a decimal id")?;
    let sid = args
        .strategy_id
        .parse::<u64>()
        .context("strategy_id must be a decimal id")? as i64;
    let Some(conn) = crate::strat_db::open_reader() else {
        return Ok(
            json!({"versions":[],"history_since_ms":null,"note":"no strategy archive on this Station yet"}),
        );
    };
    let since: Option<i64> = conn
        .query_row(
            "SELECT MIN(valid_from) FROM strategy_versions WHERE core_uid=?1 AND strategy_id=?2",
            rusqlite::params![core as i64, sid],
            |r| r.get(0),
        )
        .context("strategy archive unreadable")?;
    if let Some(at) = args.at_ms {
        return at_instant(&conn, core, sid, at, &args.fields, since);
    }
    let from = args.from_ms.unwrap_or(0);
    let to = args.to_ms.unwrap_or(i64::MAX);
    ensure!(to > from, "to_ms must be after from_ms");
    let mut stmt = conn.prepare(
        "SELECT valid_from, valid_to, change_kind, origin, n_changed, changed_json
         FROM strategy_versions WHERE core_uid=?1 AND strategy_id=?2 AND valid_from>=?3 AND valid_from<?4
         ORDER BY valid_from DESC LIMIT ?5 OFFSET ?6",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            core as i64,
            sid,
            from,
            to,
            (PAGE + 1) as i64,
            args.offset as i64
        ],
        |r| {
            Ok(json!({
                "valid_from_ms": r.get::<_, i64>(0)?,
                "valid_to_ms": r.get::<_, Option<i64>>(1)?,
                "change_kind": r.get::<_, String>(2)?,
                "origin": r.get::<_, Option<String>>(3)?,
                "n_changed": r.get::<_, i64>(4)?,
                "changed": changed(r.get::<_, Option<String>>(5)?.as_deref()),
            }))
        },
    )?;
    let mut versions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = versions.len() > PAGE;
    versions.truncate(PAGE);
    Ok(json!({
        "core": args.core, "strategy_id": args.strategy_id,
        "history_since_ms": since,
        "versions": versions, "has_more": has_more, "next_offset": args.offset + versions.len(),
        "values_are_data": true,
    }))
}

/// A version's diff with long values shown as their length.
fn changed(raw: Option<&str>) -> Value {
    let Some(Ok(Value::Object(map))) = raw.map(serde_json::from_str::<Value>) else {
        return Value::Null;
    };
    let short = |v: &Value| match v {
        Value::String(s) if s.chars().count() > LONG_VALUE => {
            json!({"long_text_chars": s.chars().count(), "items": s.split(',').filter(|i| !i.trim().is_empty()).count()})
        }
        other => other.clone(),
    };
    map.into_iter()
        .map(|(name, pair)| {
            let old = pair.get("old").map(short).unwrap_or(Value::Null);
            let new = pair.get("new").map(short).unwrap_or(Value::Null);
            (name, json!({"old": old, "new": new}))
        })
        .collect::<Map<_, _>>()
        .into()
}

/// The version in effect at `at`, with its values, narrowed to `fields` when given.
fn at_instant(
    conn: &rusqlite::Connection,
    core: u64,
    sid: i64,
    at: i64,
    fields: &[String],
    since: Option<i64>,
) -> Result<Value> {
    let found: Option<(i64, Option<i64>, String)> = conn
        .query_row(
            "SELECT valid_from, valid_to, raw_json FROM strategy_versions
             WHERE core_uid=?1 AND strategy_id=?2 AND valid_from<=?3 ORDER BY valid_from DESC LIMIT 1",
            rusqlite::params![core as i64, sid, at],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((from, to, raw)) = found else {
        return Ok(
            json!({"at_ms": at, "version": null, "history_since_ms": since,
            "meaning": if since.is_some() { "the instant predates the archived history" } else { "no version of this strategy was ever archived" }}),
        );
    };
    let Value::Object(all) =
        serde_json::from_str::<Value>(&raw).context("archived version unreadable")?
    else {
        anyhow::bail!("archived version is not an object");
    };
    let values: Map<String, Value> = all
        .into_iter()
        .filter(|(name, _)| !name.starts_with("__"))
        .filter(|(name, _)| {
            fields.is_empty() || fields.iter().any(|f| f.eq_ignore_ascii_case(name))
        })
        .collect();
    let absent: Vec<&String> = fields
        .iter()
        .filter(|f| !values.keys().any(|k| k.eq_ignore_ascii_case(f)))
        .collect();
    let mut answer = json!({"at_ms": at, "valid_from_ms": from, "valid_to_ms": to, "fields": values, "values_are_data": true});
    if !absent.is_empty() {
        answer["absent_fields"] = json!(absent);
    }
    Ok(answer)
}

#[cfg(test)]
mod tests;
