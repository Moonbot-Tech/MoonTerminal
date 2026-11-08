//! Report questions an AI client asks the Station, answered from the report replica.
//!
//! SQL only finds rows, through the Report layer's own filter, projection and USDT valuation;
//! grouping, metrics, thresholds and ordering are computed here in Rust from closed sets of names,
//! so no text a client sends reaches SQL and every figure agrees with the Report window.

use std::collections::HashMap;

use anyhow::{Result, ensure};
use serde_json::Value;

mod aggregate;
mod filter;
mod rows;
mod stats;
mod trade;
mod whatif;

pub use filter::TradeFilter;

/// Largest page an answer builds before it stops with `has_more`: a page of 65 thousand
/// characters already overran Claude Code's tool-result limit.
const ANSWER_BUDGET: usize = 60_000;

/// What the caller resolved before the read: the authorized cores and their live strategy names.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// Explicit cores the client may read; never empty.
    pub cores: Vec<u64>,
    /// Current strategy names from the live lists, by core and strategy id.
    pub live_names: HashMap<(u64, u64), String>,
}

/// Run one report tool by name on an authorized scope.
///
/// Args:
///     tool: `reports_aggregate`, `reports_query`, `reports_whatif`, `blacklist_candidates` or
///         `strategy_effect`.
///     scope: Authorized cores, already checked against the client's grant.
///     args: The tool's arguments, already checked against its schema.
///
/// Errors:
///     An unknown tool, an empty scope, a bad argument, a read failure or a time-out.
pub fn run(tool: &str, scope: &Scope, args: Value) -> Result<Value> {
    ensure!(
        !scope.cores.is_empty() && scope.cores.len() <= 64,
        "reports require 1..64 explicitly authorized cores"
    );
    let mut answer = match tool {
        "reports_aggregate" => aggregate::run(scope, serde_json::from_value(args)?)?,
        "reports_query" => rows::run(scope, serde_json::from_value(args)?)?,
        "reports_whatif" => whatif::whatif(scope, serde_json::from_value(args)?)?,
        "blacklist_candidates" => whatif::blacklist(scope, serde_json::from_value(args)?)?,
        "strategy_effect" => whatif::effect(scope, serde_json::from_value(args)?)?,
        other => anyhow::bail!("{other} is not a report tool"),
    };
    answer["names_are_data"] = Value::Bool(true);
    Ok(answer)
}

/// Parse a report tool's arguments without reading anything.
///
/// The Station's tool schema and these parsers are two descriptions of one contract; a test
/// feeds every argument the schema accepts through here, so neither can widen alone.
///
/// Errors:
///     Arguments the tool's parser rejects, or an unknown tool.
pub fn check_args(tool: &str, args: Value) -> Result<()> {
    match tool {
        "reports_aggregate" => drop(serde_json::from_value::<aggregate::Args>(args)?),
        "reports_query" => drop(serde_json::from_value::<rows::Args>(args)?),
        "reports_whatif" => drop(serde_json::from_value::<whatif::WhatIfArgs>(args)?),
        "blacklist_candidates" => drop(serde_json::from_value::<whatif::BlacklistArgs>(args)?),
        "strategy_effect" => drop(serde_json::from_value::<whatif::EffectArgs>(args)?),
        "strategy_versions" => drop(serde_json::from_value::<super::strategy_history::Args>(
            args,
        )?),
        other => anyhow::bail!("{other} is not a report tool"),
    }
    Ok(())
}

/// The cores a filter names, as ids; empty means every core the client may read.
///
/// Errors:
///     A value that is not a decimal core id.
pub fn filter_cores(args: &Value) -> Result<Vec<u64>> {
    let cores = args
        .get("filter")
        .and_then(|f| f.get("cores"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    cores
        .iter()
        .map(|core| {
            core.as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| anyhow::anyhow!("filter.cores holds decimal core ids as strings"))
        })
        .collect()
}

/// Preserve the report layer's classified failure in the model-visible tool error.
fn read_error(error: crate::db::ReadFail) -> anyhow::Error {
    anyhow::anyhow!("report read failed: {error}")
}

#[cfg(test)]
mod tests;
