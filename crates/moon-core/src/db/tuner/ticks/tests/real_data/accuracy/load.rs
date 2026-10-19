//! Bench-equivalent loading; model fit never selects the pair population.

use super::super::{
    Coverage, KlineCache, OwnLines, Query, StrategyValues, coin_match_key, coin_of_market, deltas,
    exit_params, fit_for_search, infer_tick, is_tunable, long_position_ms, model_window,
    param_keys, paths, prepare_deal, read_deals, required_spans, strategy_values_at, verify,
};
use super::super::{archived_lines, btc_markets, core_step_lags, core_venues, held_ticks};
use crate::db::tuner::ticks::{Deal, ModelSettings};
use crate::feed::types::Tick;
use std::collections::{HashMap, HashSet};

/// One covered report row, including everything required for a paired replay or a search.
pub(super) struct Loaded {
    pub(super) deal: Deal,
    pub(super) ticks: Vec<Tick>,
    pub(super) entry_line: Option<Vec<(i64, f64)>>,
    pub(super) exit_points: Option<Vec<(i64, f64)>>,
    pub(super) values: HashMap<String, String>,
    pub(super) venue: String,
    pub(super) latency_ms: f64,
    pub(super) fit: bool,
    pub(super) name: Option<String>,
    pub(super) version_covers: Option<bool>,
}

/// Read schema defaults in the bench's lowercase numeric format.
pub(super) fn defaults_from_env() -> HashMap<String, f64> {
    std::env::var("MOON_TICKS_DEFAULTS")
        .map(|spec| {
            spec.split(',')
                .filter_map(|pair| pair.split_once('='))
                .filter_map(|(k, v)| Some((k.trim().to_ascii_lowercase(), v.trim().parse().ok()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Prepare covered rows with population timing for calibration, or zero timing before A/B splits.
pub(super) fn load_all(defaults: &HashMap<String, f64>, calibration: bool) -> Vec<Loaded> {
    let scope = Query {
        from: -1,
        to: crate::db::analytics::ANALYTICS_HORIZON_SECS,
        metric: crate::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let read = read_deals(&scope).expect("read measurement deals");
    let inventory = crate::market::trade_replay::trade_cache::handle()
        .expect("held tape cache")
        .inventory()
        .expect("held tape inventory response")
        .expect("held tape inventory")
        .keys;
    let venues = core_venues();
    let keys = param_keys();
    let lags = if calibration {
        core_step_lags(&read.deals, &keys, defaults)
    } else {
        HashMap::new()
    };
    let klines = KlineCache::open(paths::klines_db_path());
    let btc = btc_markets();
    let margin = crate::market::trade_replay::margin_ms();
    let identity = rusqlite::Connection::open_with_flags(
        paths::strategies_db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok();
    let has_deleted = identity.as_ref().is_some_and(|conn| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('strategies') WHERE name='deleted')",
            [],
            |r| r.get::<_, bool>(0),
        )
        .expect("read strategy identity schema")
    });
    let mut loaded = Vec::new();
    for mut deal in read.deals {
        if !is_tunable(&deal.kind, &deal.sell_reason) {
            continue;
        }
        let Some(values) =
            strategy_values_at(deal.strategy_id, Some(deal.core_uid), deal.buy_ms, &keys)
        else {
            continue;
        };
        let Some(venue) = venues.get(&deal.core_uid) else {
            continue;
        };
        let Some(window) = model_window(&deal, margin, long_position_ms()) else {
            continue;
        };
        let coin = coin_match_key(&deal.coin);
        let mut ticks = Vec::new();
        let mut covered = Coverage::none();
        let mut address = None;
        for (exchange, market) in inventory
            .iter()
            .filter(|(e, m)| e == venue && coin_match_key(coin_of_market(m)) == coin)
        {
            let (held, spans) = held_ticks(exchange, market, &window.focus_spans());
            if address.is_none() && !held.is_empty() {
                address = Some((exchange, market));
            }
            ticks.extend(held);
            for &span in spans.spans() {
                covered.add(span);
            }
        }
        ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        ticks.dedup_by(|a, b| a.time_ms == b.time_ms && a.price == b.price && a.qty == b.qty);
        if ticks.is_empty() || !covered.covers(&required_spans(&window)) {
            continue;
        }
        deal.tick = infer_tick(&ticks);
        deal.step_lag_ms = lags.get(&deal.core_uid).copied().unwrap_or(0.0);
        let (entry_line, exit_points, answered) = archived_lines(&deal);
        let latency_ms = crate::db::tuner::ticks::mshot::DEFAULT_LATENCY_MS;
        let model = ModelSettings {
            latency_ms,
            ..Default::default()
        };
        let sv = StrategyValues {
            values: &values,
            defaults,
        };
        let entry = super::compat::entry_of(&deal.kind, &sv, model);
        let exit = exit_params(&sv, ModelSettings::default());
        if let (Some(cache), Some((exchange, market))) = (klines.as_ref(), address) {
            let history = deltas::track_for(
                cache,
                exchange,
                market,
                btc.get(exchange).map(String::as_str),
                &deal,
                &ticks,
                &covered,
            );
            deal.bars = history.bars;
            deal.delta_track = history.track;
        }
        prepare_deal(
            &mut deal,
            &entry,
            &exit,
            OwnLines {
                entry: entry_line.as_deref(),
                exit: exit_points.as_deref(),
                answered,
            },
            &covered,
        );
        let fit = fit_for_search(&verify(
            &deal,
            &ticks,
            &entry,
            &exit,
            entry_line.as_deref(),
            exit_points.as_deref(),
        ));
        let (name, version_covers) = identity_of(identity.as_ref(), &deal, has_deleted);
        loaded.push(Loaded {
            deal,
            ticks,
            entry_line,
            exit_points,
            values,
            venue: venue.clone(),
            latency_ms,
            fit,
            name,
            version_covers,
        });
    }
    loaded.sort_by_key(|l| (l.deal.core_uid, l.deal.report_uid));
    eprintln!(
        "loaded {} covered deals; {} without millisecond stamps",
        loaded.len(),
        read.without_ms
    );
    loaded
}

/// Read current strategy identity and whether a version covers the stamp, without creating a DB.
/// Missing rows stay null; query failures panic rather than masquerade as absent provenance.
fn identity_of(
    conn: Option<&rusqlite::Connection>,
    deal: &Deal,
    has_deleted: bool,
) -> (Option<String>, Option<bool>) {
    let Some(conn) = conn else {
        return (None, None);
    };
    let args = rusqlite::params![deal.strategy_id, deal.core_uid as i64, deal.buy_ms];
    let name_sql = if has_deleted {
        "SELECT name FROM strategies WHERE strategy_id=?1 AND core_uid=?2 AND COALESCE(deleted,0)=0"
    } else {
        "SELECT name FROM strategies WHERE strategy_id=?1 AND core_uid=?2"
    };
    let name = conn.query_row(
        name_sql,
        rusqlite::params![deal.strategy_id, deal.core_uid as i64],
        |r| r.get::<_, String>(0),
    );
    let name = match name {
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        result => Some(result.expect("read strategy identity name")),
    };
    let covers = conn.query_row("SELECT EXISTS(SELECT 1 FROM strategy_versions WHERE strategy_id=?1 AND core_uid=?2 AND valid_from<=?3 AND (valid_to IS NULL OR valid_to>?3))",args,|r| r.get::<_,bool>(0));
    let covers = match covers {
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        result => Some(result.expect("read strategy identity version coverage")),
    };
    (name, covers)
}

/// Fit timing on train and unpicked report rows only, then refresh A/B replay eligibility.
pub(super) fn fit_ab_lags(
    loaded: &mut [Loaded],
    holdout: &HashSet<(u64, i64)>,
    defaults: &HashMap<String, f64>,
) {
    let scope = Query {
        from: -1,
        to: crate::db::analytics::ANALYTICS_HORIZON_SECS,
        metric: crate::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let read = read_deals(&scope).expect("read AB timing deals");
    let train: Vec<_> = read
        .deals
        .into_iter()
        .filter(|d| !holdout.contains(&(d.core_uid, d.report_uid)))
        .collect();
    let lags = core_step_lags(&train, &param_keys(), defaults);
    for row in loaded {
        row.deal.step_lag_ms = lags.get(&row.deal.core_uid).copied().unwrap_or(0.0);
        let sv = StrategyValues {
            values: &row.values,
            defaults,
        };
        let entry = super::compat::entry_of(
            &row.deal.kind,
            &sv,
            ModelSettings {
                latency_ms: row.latency_ms,
                ..Default::default()
            },
        );
        let exit = exit_params(&sv, ModelSettings::default());
        row.fit = fit_for_search(&verify(
            &row.deal,
            &row.ticks,
            &entry,
            &exit,
            row.entry_line.as_deref(),
            row.exit_points.as_deref(),
        ));
    }
}
