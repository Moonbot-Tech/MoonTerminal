//! Questions about other choices: removing trades, blacklisting coins, what an edit changed.
//!
//! Each answer that suggests a change checks itself on a later, held-out part of the window: on a
//! real replica only half the losing coin+strategy pairs of three weeks lost again afterwards, so a
//! figure fitted on the whole window alone would read as more certain than it is.

use std::collections::HashMap;

use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Scope;
use super::filter::{Residual, TradeFilter};
use super::stats::{self, Stats};
use super::trade::{self, Trade, Want};

/// Arguments of `reports_whatif`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhatIfArgs {
    pub filter: TradeFilter,
    /// The trades to take away: the filter's own conditions, window and cores ignored.
    pub variant: TradeFilter,
    #[serde(default = "default_holdout")]
    pub holdout_pct: u32,
}

/// Thirty percent of the window held out unless asked otherwise.
fn default_holdout() -> u32 {
    30
}

/// The metrics a comparison prints for each side.
const COMPARE: &[&str] = &[
    "trades",
    "winrate_pct",
    "profit_usdt_sum",
    "profit_factor",
    "max_drawdown_usdt",
    "unvalued",
];

/// A compact metrics object for a comparison.
fn brief(stats: &Stats) -> Value {
    COMPARE
        .iter()
        .map(|m| (m.to_string(), stats.output(m)))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// Split trades in close order into the fitting part and the held-out later part.
fn split(trades: &[Trade], holdout_pct: u32) -> (&[Trade], &[Trade]) {
    let held = trades.len() * holdout_pct as usize / 100;
    trades.split_at(trades.len() - held)
}

/// Run `reports_whatif`.
///
/// Errors:
///     A bad argument, a read failure or a time-out.
pub fn whatif(scope: &Scope, args: WhatIfArgs) -> Result<Value> {
    ensure!(args.holdout_pct <= 90, "holdout_pct must be 0..90");
    let mut entry: Vec<&'static str> = Vec::new();
    for bound in &args.variant.entry {
        let field = trade::entry_field(&bound.field).ok_or_else(|| {
            anyhow::anyhow!("variant entry field {} is not a tuner field", bound.field)
        })?;
        if !entry.contains(&field) {
            entry.push(field);
        }
    }
    let ids = super::filter::strategy_ids(&args.variant)?;
    let want = Want {
        text: false,
        entry: entry.clone(),
    };
    let (loaded, ()) = trade::load(scope, &args.filter, &want, |_| Ok(()))?;
    // The variant is matched on the same trades; its entry values sit at the front of each
    // trade's list, in the order requested above.
    let removed = Residual::new(&args.variant, &entry)?;
    let side = |part: &[Trade]| {
        let (gone, kept): (Vec<&Trade>, Vec<&Trade>) = part.iter().partition(|t| {
            (ids.is_empty() || ids.contains(&(t.sid as i64)))
                && (args.variant.side.is_none()
                    || args.variant.side
                        == Some(if t.short {
                            super::filter::Side::Short
                        } else {
                            super::filter::Side::Long
                        }))
                && removed.keeps(t)
        });
        json!({"fact": brief(&stats::of(part.iter())), "variant": brief(&stats::of(kept)), "removed": brief(&stats::of(gone))})
    };
    let (train, holdout) = split(&loaded.trades, args.holdout_pct);
    let totals = stats::of(&loaded.trades);
    Ok(json!({
        "train": side(train),
        "holdout": side(holdout),
        "holdout_pct": args.holdout_pct,
        "coverage": super::aggregate::coverage(&loaded, &totals),
        "model": "Only removes the matching trades; it cannot know which other entry a freed slot would have taken. Trust a gain that holds on the holdout part.",
    }))
}

/// Arguments of `blacklist_candidates`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlacklistArgs {
    pub filter: TradeFilter,
    #[serde(default = "default_min_trades")]
    pub min_trades: u32,
    #[serde(default = "default_holdout")]
    pub holdout_pct: u32,
}

/// Five trades of a coin before it is a candidate, as in the tuner.
fn default_min_trades() -> u32 {
    5
}

/// Run `blacklist_candidates`: the tuner's own coin pick, per strategy, with its holdout check.
///
/// Errors:
///     A bad argument, a read failure or a time-out.
pub fn blacklist(scope: &Scope, args: BlacklistArgs) -> Result<Value> {
    ensure!(args.holdout_pct <= 90, "holdout_pct must be 0..90");
    ensure!(args.min_trades >= 1, "min_trades must be at least 1");
    let want = Want::default();
    let (loaded, lists) = trade::load(scope, &args.filter, &want, |conn| {
        Ok(conn_lists(conn, scope))
    })?;
    let lists = lists?;
    let mut keyed = Vec::new();
    let mut unvalued = 0;
    for t in &loaded.trades {
        if t.reason == super::filter::Reason::Funding {
            continue;
        }
        let Some(profit) = t.profit_usdt else {
            unvalued += 1;
            continue;
        };
        keyed.push(crate::db::tuner::coin_pick::KeyedTrade {
            key: key(t.core, t.sid),
            trade: crate::db::tuner::coin_pick::CoinTrade {
                token: t.coin.to_string(),
                close: t.close_ms,
                profit,
            },
        });
    }
    let names: HashMap<String, (u64, u64, String)> = loaded
        .trades
        .iter()
        .map(|t| (key(t.core, t.sid), (t.core, t.sid, t.name.to_string())))
        .collect();
    let settings = crate::db::tuner::coin_pick::PickSettings {
        min_trades: args.min_trades,
        holdout_pct: args.holdout_pct,
        black: true,
        white: false,
    };
    let progress = crate::db::tuner::coin_pick::BatchProgress::default();
    let picks =
        crate::db::tuner::coin_pick::coin_picks_from_rows(keyed, &lists, settings, &progress)
            .unwrap_or_default();
    let mut out: Vec<(bool, Value)> = picks
        .into_iter()
        .map(|(k, pick)| {
            let (core, sid, name) = names.get(&k).cloned().unwrap_or_default();
            let useful = matches!(pick.verdict, crate::db::tuner::coin_pick::PickVerdict::Useful);
            let tally = |t: &crate::db::metrics::Tally| json!({"trades":t.n,"profit_usdt":stats::round(t.profit,2),"winrate_pct":stats::round(t.winrate(),2)});
            (useful, json!({
                "core": core.to_string(), "strategy_id": sid.to_string(), "strategy_name": name,
                "verdict": format!("{:?}", pick.verdict),
                "candidates": pick.black.iter().map(|c| json!({"coin":c.token,"trades":c.n,"profit_usdt":stats::round(c.profit,2)})).collect::<Vec<_>>(),
                "train_trades": pick.train_n, "holdout_trades": pick.holdout_n,
                "holdout": {"fact": tally(&pick.fact_holdout), "with_saved_list": tally(&pick.manual_holdout), "with_candidates": tally(&pick.pick_holdout)},
            }))
        })
        .collect();
    // Strategies whose candidates held up on the later part come first.
    out.sort_by_key(|(useful, _)| std::cmp::Reverse(*useful));
    let total = out.len();
    let strategies: Vec<Value> = out.into_iter().take(100).map(|(_, v)| v).collect();
    Ok(json!({
        "strategies": strategies,
        "total_strategies": total,
        "unvalued_trades_left_out": unvalued,
        "lists": "coins already in the strategy's saved blacklist are not proposed again; lists come from the strategy archive the Station keeps in step with the core",
    }))
}

/// The strategy-list key the coin pick groups by.
fn key(core: u64, sid: u64) -> String {
    format!("{sid}@{core}")
}

/// Saved blacklists of every strategy on the scope's cores, from the archive in the read snapshot.
fn conn_lists(
    conn: &rusqlite::Connection,
    scope: &Scope,
) -> Result<HashMap<String, crate::db::tuner::coin_pick::ManualLists>> {
    let names = crate::db::archive_strategy_names(conn).map_err(super::read_error)?;
    let pairs: HashMap<String, (i64, i64)> = names
        .keys()
        .filter(|(core, _)| scope.cores.contains(core))
        .map(|(core, sid)| (key(*core, *sid as u64), (*core as i64, *sid)))
        .collect();
    crate::db::tuner::coin_pick::saved_lists_for(conn, &pairs).map_err(super::read_error)
}

/// Arguments of `strategy_effect`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectArgs {
    pub core: String,
    pub strategy_id: String,
    /// The edit: a version's `valid_from` from `strategy_versions`.
    pub valid_from_ms: i64,
    #[serde(default = "default_window_days")]
    pub window_days: u32,
}

/// Two weeks on each side unless asked otherwise.
fn default_window_days() -> u32 {
    14
}

/// Run `strategy_effect`: the strategy's trades before and after one edit, in one pass.
///
/// Errors:
///     A bad argument, a read failure or a time-out.
pub fn effect(scope: &Scope, args: EffectArgs) -> Result<Value> {
    ensure!(
        (1..=90).contains(&args.window_days),
        "window_days must be 1..90"
    );
    let window = i64::from(args.window_days) * 86_400_000;
    let now = crate::util::now_unix_ms_i64();
    let from = args.valid_from_ms.saturating_sub(window);
    let to = args.valid_from_ms.saturating_add(window).min(now);
    ensure!(to > args.valid_from_ms, "the edit is in the future");
    let filter = TradeFilter {
        from_ms: Some(from),
        to_ms: Some(to),
        strategy_ids: vec![args.strategy_id.clone()],
        ..Default::default()
    };
    let (loaded, ()) = trade::load(scope, &filter, &Want::default(), |_| Ok(()))?;
    let (before, after): (Vec<&Trade>, Vec<&Trade>) = loaded
        .trades
        .iter()
        .partition(|t| t.close_ms < args.valid_from_ms);
    let (before, after) = (stats::of(before), stats::of(after));
    let small = before.trades < 20 || after.trades < 20;
    Ok(json!({
        "core": args.core, "strategy_id": args.strategy_id, "valid_from_ms": args.valid_from_ms,
        "before": {"from_ms": from, "to_ms": args.valid_from_ms, "metrics": brief(&before)},
        "after": {"from_ms": args.valid_from_ms, "to_ms": to, "metrics": brief(&after)},
        "small_sample": small,
        "caveat": "The market differs between the two windows; a change in results is not proof the edit caused it.",
    }))
}

#[cfg(test)]
mod tests;
