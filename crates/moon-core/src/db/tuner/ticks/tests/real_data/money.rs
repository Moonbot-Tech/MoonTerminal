//! The money a variant is scored in, against a real replica: every deal read the way the axis
//! reads it, in both metrics, and a variant that fills and exits where the fact did held against
//! the fact's own result. Nothing is replayed, so it runs in seconds.
//!
//! Run as `MOON_TICKS_DATA_DIR=<exe dir> cargo test -p moon-core --target
//! x86_64-pc-windows-msvc --lib db::tuner::ticks::tests::real_data::money -- --ignored
//! --nocapture` with the terminal closed on that data root (the replica lease).

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::paths;
use crate::db::analytics::Query;
use crate::db::tuner::ticks::{Deal, Exit, ExitKind, Fill, Outcome, profit_pct, read_deals};

/// The fact as a variant: filled at the report's buy, closed at its sell.
fn replay_of_fact(deal: &Deal) -> Outcome {
    Outcome {
        fill: Some(Fill {
            t_ms: deal.buy_ms,
            price: deal.buy_price,
        }),
        exit: Some(Exit {
            t_ms: deal.close_ms,
            price: deal.sell_price,
            kind: ExitKind::Take,
        }),
        profit_pct: profit_pct(deal, deal.buy_price, deal.sell_price),
    }
}

/// Per core: deals, deals with a sizing, the largest gap between the replayed fact and the fact,
/// and the median leverage (`notional / spent`) and cost (per cent of the notional).
#[derive(Default)]
struct CoreLine {
    n: usize,
    sized: usize,
    max_gap: f64,
    leverage: Vec<f64>,
    cost_pct: Vec<f64>,
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

#[test]
#[ignore = "needs a live data root in MOON_TICKS_DATA_DIR"]
fn real_data_money() {
    let Some(root) = std::env::var_os("MOON_TICKS_DATA_DIR") else {
        eprintln!("MOON_TICKS_DATA_DIR is not set; nothing to do");
        return;
    };
    assert!(paths::set_data_dir_override(PathBuf::from(root)));
    let Some(_permit) = crate::db::report_recovery::prepare() else {
        eprintln!(
            "reports replica lease unavailable ({:?}): close the terminal on this data root first",
            crate::db::report_recovery::status()
        );
        return;
    };
    for metric in [
        crate::db::ProfitMetric::Quote,
        crate::db::ProfitMetric::Percent,
    ] {
        let scope = Query {
            from: -1,
            to: crate::db::analytics::ANALYTICS_HORIZON_SECS,
            metric,
            ..Default::default()
        };
        let read = match read_deals(&scope) {
            Ok(read) => read,
            Err(error) => {
                eprintln!("{metric:?}: the deals did not read ({error:?})");
                continue;
            }
        };
        let mut cores: BTreeMap<String, CoreLine> = BTreeMap::new();
        let mut not_a_trade = 0usize;
        for deal in &read.deals {
            let line = cores.entry(deal.core_name.clone()).or_default();
            line.n += 1;
            if let Some(sizing) = deal.sizing {
                line.sized += 1;
                if deal.spent > 0.0 {
                    line.leverage.push(sizing.notional / deal.spent);
                }
                line.cost_pct.push(sizing.cost / sizing.notional * 100.0);
            }
            match replay_of_fact(deal).profit_metric(deal) {
                // Only a sized deal is held to the identity: an unsized one keeps the gross move
                // on its spend, which is what it always was.
                Some(value) if deal.sizing.is_some() => {
                    line.max_gap = line.max_gap.max((value - deal.fact_pnl).abs());
                }
                Some(_) => {}
                None => not_a_trade += 1,
            }
        }
        eprintln!(
            "{metric:?}: {} deal(s), {not_a_trade} not a trade under the replayed fact",
            read.deals.len()
        );
        for (core, mut line) in cores {
            eprintln!(
                "  {core:14} n={:5} sized={:5} max|replayed fact - fact|={:.3e} lev~{:.3} cost~{:.4}%",
                line.n,
                line.sized,
                line.max_gap,
                median(&mut line.leverage),
                median(&mut line.cost_pct),
            );
        }
    }
}
