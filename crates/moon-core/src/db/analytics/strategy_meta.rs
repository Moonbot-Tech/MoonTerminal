//! Batched strategy-head and version metadata for Analytics labels.
//!
//! Loaded in BATCHES keyed on `(strategy_id, core_uid)` — two statements per 400 pairs — and
//! never per group. The per-group form this replaced ran six correlated scalar subqueries for
//! every row of the strategy list, four of them parsing a whole `raw_json` blob, and measured
//! 550 ms of a 709 ms Strategies read at 2 400 groups. Both the Summary stream and
//! `groups::groups` use this loader when both metadata queries succeed, so ordinary labels stay
//! aligned without repeating JSON parsing per group.
//!
//! They DO still differ on what to do when the strategy database half-answers, and that is a
//! PRE-EXISTING split this module neither introduced nor resolves. [`SummaryMetadata`] separates
//! `heads` from `groups` because the Summary was built to fall back differently for top-trade
//! rows than for group rows: when the head query succeeds and the VERSION query fails, `groups`
//! is `None`, so `summary_stream`'s group labels drop to bare ids while its top-trade rows keep
//! their names. `groups::enrich` takes the other reading — `groups` when it has it, `heads`
//! otherwise — and so keeps names and status while emptying only the version-derived fields.
//! Aligning the two is a change to the Summary's FAILURE path rather than a performance
//! question, and is deliberately left to whoever picks it up next.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, types::Value};

use crate::db::ReadResult;
use crate::db::read_fail::read_fail_on;

/// Metadata for one numeric strategy/core identity.
#[derive(Clone, Debug, Default)]
pub(super) struct StrategyMetadata {
    pub(super) name: Option<String>,
    pub(super) alive: i64,
    pub(super) has_head: bool,
    pub(super) deleted: bool,
    pub(super) kind: String,
    pub(super) lastedit: String,
    pub(super) blacklist: Option<String>,
    pub(super) whitelist: Option<String>,
}

impl StrategyMetadata {
    /// Distinct coins this strategy's blacklist names, or zero when no live head can show one.
    ///
    /// Returns:
    ///     Normalized token count, or zero for a deleted or headless strategy.
    pub(super) fn blacklist_count(&self) -> i64 {
        self.list_count(self.blacklist.as_deref())
    }

    /// Distinct coins this strategy's whitelist names, or zero when no live head can show one.
    ///
    /// Returns:
    ///     Normalized token count, or zero for a deleted or headless strategy.
    pub(super) fn whitelist_count(&self) -> i64 {
        self.list_count(self.whitelist.as_deref())
    }

    /// Apply the live-head gate both coin-list counts sit behind.
    ///
    /// THE ONE PLACE. Both the Strategies grouping and the Summary stream ask for these counts,
    /// and before this method each carried its own copy of `has_head && !deleted` — with a
    /// comment on each worrying about drifting from a THIRD copy in
    /// `db::tuner::strategy_current_values`, which is what the coin table reads the same lists
    /// through. The gate is a property of the metadata, not of either caller: without it a
    /// DELETED strategy reports a list count its own coin table cannot reproduce.
    ///
    /// Args:
    ///     list: Raw `CoinsBlackList` / `CoinsWhiteList` field text, when the version supplied one.
    ///
    /// Returns:
    ///     Distinct normalized coin tokens, or zero when the gate rejects this identity.
    fn list_count(&self, list: Option<&str>) -> i64 {
        if !self.has_head || self.deleted {
            return 0;
        }
        // Counting stays in `groups::count_list`, which is the rule the analytics coin table
        // matches by; this method owns only the GATE. That function's owned-`String` signature
        // costs one small allocation per gated group and is a leftover of the reference decoder
        // in `groups/tests.rs`, which still calls it that way.
        super::groups::count_list(list.map(str::to_owned))
    }
}

/// Metadata split at the historical fallback boundary between Summary top rows and groups.
#[derive(Clone, Debug, Default)]
pub(super) struct SummaryMetadata {
    pub(super) heads: HashMap<(i64, i64), StrategyMetadata>,
    pub(super) groups: Option<HashMap<(i64, i64), StrategyMetadata>>,
}

/// Load strategy heads and current versions in bounded batches for requested Analytics pairs.
///
/// Args:
///     conn: Pinned report snapshot with the strategy database attached.
///     pairs: Numeric strategy/core identities eligible for enrichment.
///
/// Returns:
///     Head metadata plus all-or-nothing version-enriched group metadata.
pub(super) fn read_metadata(
    conn: &Connection,
    pairs: &HashSet<(i64, i64)>,
) -> ReadResult<SummaryMetadata> {
    const CTX: &str = "analytics: summary strategy metadata";
    if pairs.is_empty() {
        return Ok(SummaryMetadata::default());
    }
    let mut metadata = HashMap::<(i64, i64), StrategyMetadata>::new();
    let pairs = pairs.iter().copied().collect::<Vec<_>>();
    for chunk in pairs.chunks(400) {
        let (filter, params) = metadata_filter(chunk);
        let heads_sql = format!(
            "SELECT core_uid, strategy_id, name, deleted, checked
             FROM strat.strategies
             WHERE (core_uid, strategy_id) IN ({filter})"
        );
        let mut heads = conn
            .prepare(&heads_sql)
            .map_err(|error| read_fail_on(conn, CTX, error))?;
        let rows = heads
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                ))
            })
            .map_err(|error| read_fail_on(conn, CTX, error))?;
        for row in rows {
            let (core_uid, strategy_id, name, deleted, checked) =
                row.map_err(|error| read_fail_on(conn, CTX, error))?;
            let pair = (strategy_id, core_uid);
            metadata.entry(pair).or_insert_with(|| StrategyMetadata {
                name,
                alive: if deleted != 0 {
                    0
                } else if checked != 0 {
                    2
                } else {
                    1
                },
                has_head: true,
                deleted: deleted != 0,
                ..StrategyMetadata::default()
            });
        }
        drop(heads);
    }

    let heads = metadata.clone();
    if let Err(error) = read_version_metadata(conn, &pairs, &mut metadata) {
        log::warn!(
            "analytics: strategy versions unavailable, keeping head names for top rows: {error}"
        );
        return Ok(SummaryMetadata {
            heads,
            groups: None,
        });
    }
    Ok(SummaryMetadata {
        heads,
        groups: Some(metadata),
    })
}

/// Add fields from the first current-version row SQLite emits for each loaded head.
///
/// Args:
///     conn: Pinned report snapshot with the strategy database attached.
///     pairs: Numeric strategy/core identities to query in bounded chunks.
///     metadata: Head map updated from the first emitted current-version row for each pair.
///
/// Returns:
///     Success when every version row was decoded and applied.
fn read_version_metadata(
    conn: &Connection,
    pairs: &[(i64, i64)],
    metadata: &mut HashMap<(i64, i64), StrategyMetadata>,
) -> ReadResult<()> {
    const CTX: &str = "analytics: summary strategy versions";
    let mut seen_versions = HashSet::new();
    for chunk in pairs.chunks(400) {
        let (filter, params) = metadata_filter(chunk);
        let versions_sql = format!(
            "SELECT core_uid, strategy_id,
                    json_extract(raw_json, '$.SignalType'),
                    json_extract(raw_json, '$.LastEditDate'),
                    CAST(json_extract(raw_json, '$.CoinsBlackList') AS TEXT),
                    CAST(json_extract(raw_json, '$.CoinsWhiteList') AS TEXT)
             FROM strat.strategy_versions
             WHERE valid_to IS NULL
               AND (core_uid, strategy_id) IN ({filter})"
        );
        let mut versions = conn
            .prepare(&versions_sql)
            .map_err(|error| read_fail_on(conn, CTX, error))?;
        let rows = versions
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|error| read_fail_on(conn, CTX, error))?;
        for row in rows {
            let (core_uid, strategy_id, kind, lastedit, blacklist, whitelist) =
                row.map_err(|error| read_fail_on(conn, CTX, error))?;
            let pair = (strategy_id, core_uid);
            if !seen_versions.insert(pair) {
                continue;
            }
            let item = metadata.entry(pair).or_default();
            item.kind = kind;
            item.lastedit = lastedit;
            item.blacklist = blacklist;
            item.whitelist = whitelist;
        }
    }
    Ok(())
}

/// Build one bounded row-value filter and its `(core_uid, strategy_id)` parameters.
///
/// Args:
///     pairs: Strategy/core identities in strategy-first Rust tuple order.
///
/// Returns:
///     SQL row placeholders and core-first parameter values matching them.
fn metadata_filter(pairs: &[(i64, i64)]) -> (String, Vec<Value>) {
    let mut filter = Vec::with_capacity(pairs.len());
    let mut params = Vec::with_capacity(pairs.len() * 2);
    for (strategy_id, core_uid) in pairs {
        filter.push("(?, ?)");
        params.push(Value::Integer(*core_uid));
        params.push(Value::Integer(*strategy_id));
    }
    (filter.join(", "), params)
}
