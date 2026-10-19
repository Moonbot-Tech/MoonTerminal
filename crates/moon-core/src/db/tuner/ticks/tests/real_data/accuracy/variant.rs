//! Replay B's effective knobs on A and compare the resulting trade with B's report row.

use super::{compat, load::Loaded};
use crate::db::tuner::ticks::{
    ModelSettings, PRICE_TOLERANCE, TICK_PARAMS, entry_model_for,
    params::{StrategyValues, exit_params},
    profit_pct, simulate, verify,
};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Replace only tick knobs with B's values or defaults, preserving A's other model inputs.
pub(super) fn effective_overlay(
    a: &HashMap<String, String>,
    b: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut laid = a.clone();
    for p in TICK_PARAMS {
        if let Some(value) = b.get(p.key) {
            laid.insert(p.key.to_owned(), value.clone());
        } else {
            laid.remove(p.key);
        }
    }
    laid
}

/// Emit coverage and provenance even for rows neither model admits to a search.
pub(super) fn deal_line(row: &Loaded) -> Value {
    json!({"type":"deal","core_uid":row.deal.core_uid,"report_uid":row.deal.report_uid,
        "strategy_id":row.deal.strategy_id,"kind":row.deal.kind,"fit":row.fit,
        "version_covers":row.version_covers,"has_exit_points":row.exit_points.is_some()})
}

/// Compare a replay with the partner using the verdict's common time and price tolerances.
/// Fill error is direction-signed: positive means worse (long paid more, short sold lower).
/// Variant replay keeps A's archived entry line even when B's entry knobs differ.
pub(super) fn score_pair(
    a: &Loaded,
    b: &Loaded,
    defaults: &HashMap<String, f64>,
    effective_diff: bool,
    entry_knobs_differ: bool,
) -> Value {
    let laid = effective_overlay(&a.values, &b.values);
    let sv = StrategyValues {
        values: &laid,
        defaults,
    };
    let model = ModelSettings {
        latency_ms: a.latency_ms,
        ..Default::default()
    };
    let entry = compat::entry_of(&a.deal.kind, &sv, model);
    let exit = exit_params(&sv, ModelSettings::default());
    let outcome = simulate(&a.deal, &a.ticks, &entry, &exit, a.entry_line.as_deref());
    let stop = verify::is_stop_reason(&b.deal.sell_reason);
    let time_tol = if stop {
        verify::BOOK_STOP_TIME_TOLERANCE_MS
    } else {
        verify::POINT_TIME_TOLERANCE_MS
    };
    let price_tol = if stop {
        verify::STOP_PRICE_TOLERANCE
    } else {
        PRICE_TOLERANCE
    };
    let b_profit = profit_pct(&b.deal, b.deal.buy_price, b.deal.sell_price);
    let closed = outcome.exit.filter(|_| outcome.profit_pct.is_some());
    let same_name = a.name.as_ref().zip(b.name.as_ref()).map(|(a, b)| a == b);
    let bad_price = [b.deal.buy_price, b.deal.sell_price]
        .iter()
        .any(|p| !p.is_finite() || *p <= 0.0);
    json!({"type":"pair","a_core":a.deal.core_uid,"a_uid":a.deal.report_uid,
        "b_core":b.deal.core_uid,"b_uid":b.deal.report_uid,"kind":a.deal.kind,
        "a_fit":a.fit,"b_fit":b.fit,"same_name":same_name,"bad_price":bad_price,
        "a_version_covers":a.version_covers,"b_version_covers":b.version_covers,
        "effective_diff":effective_diff,"entry_knobs_differ":entry_knobs_differ,
        "entry_line_used":a.entry_line.is_some(),"is_short":a.deal.is_short,
        "entry_modelled":entry_model_for(&a.deal.kind),"filled":outcome.fill.is_some(),
        "fill_err_pct":outcome.fill.map(|f| (f.price / b.deal.buy_price-1.0)*100.0 * if a.deal.is_short { -1.0 } else { 1.0 }),
        "exit_kind":outcome.exit.map(|e| format!("{:?}",e.kind)),
        "exit_time_ok":closed.is_some_and(|e| e.t_ms.abs_diff(b.deal.close_ms)<=time_tol as u64),
        "exit_price_ok":closed.is_some_and(|e| (e.price/b.deal.sell_price-1.0).abs()<=price_tol),
        "profit_err_pp":outcome.profit_pct.zip(b_profit).map(|(a,b)| a-b),
        "variant_profit":outcome.profit_pct,"b_profit":b_profit,
        "buys_agree":(a.deal.buy_price/b.deal.buy_price-1.0).abs()<=PRICE_TOLERANCE})
}

#[cfg(test)]
mod tests;
