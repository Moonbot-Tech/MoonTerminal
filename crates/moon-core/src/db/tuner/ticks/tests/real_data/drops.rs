//! Calibration on the UI's held tape; extended history is diagnostic and cannot enable entry.

use std::collections::BTreeMap;

use super::*;
use crate::db::tuner::ticks::drops::{
    DropsEntry, DropsParams, KIND_DROPS, PRICE_TOLERANCE_PCT, SEARCH_BEFORE_MS,
};
use crate::db::tuner::ticks::params::drops_params;

/// All detector inputs, including the fixed buy-condition switches omitted at core defaults.
const INPUTS: [(&str, bool); 9] = [
    ("DropsMaxTime", false),
    ("DropsPriceMA", false),
    ("DropsLastPriceMA", false),
    ("DropsPriceDelta", false),
    ("DropsPriceIsLow", true),
    ("DropsUseLastPrice", true),
    ("buyPrice", false),
    ("buyPriceLastTrade", true),
    ("Use30SecOldASK", true),
];

/// Counts partition every replayable row, including absent detections and unfilled limits.
#[derive(Default)]
struct Metrics {
    refused: usize,
    degenerate: usize,
    missing_tape: usize,
    replayable: usize,
    detect_hit: usize,
    price_hit: usize,
    joint: usize,
    no_detect: usize,
    no_fill: usize,
    early: usize,
    late: usize,
    price_off: usize,
    detect_errors: Vec<f64>,
    price_errors: Vec<f64>,
    comment_errors: Vec<f64>,
    comment_hit: usize,
    at_buy: usize,
    at_buy_hit: usize,
    fact_vs_buy_print: Vec<f64>,
    fact_vs_limit: Vec<f64>,
    fact_vs_detect_print: Vec<f64>,
    early_signed_dt: Vec<f64>,
}

impl Metrics {
    /// Add one covered tape; missing warm-up is a refusal rather than a detection miss.
    fn observe(&mut self, deal: &Deal, ticks: &[Tick], params: &DropsParams) {
        let trace = DropsEntry(params).trace(deal, ticks);
        if trace.refused {
            self.refused += 1;
            return;
        }
        if trace.degenerate {
            self.degenerate += 1;
            return;
        }
        self.replayable += 1;
        let stamp = deal.buy_set_ms.expect("calibration requires buy_set");
        let print_at = |stamp: i64| {
            ticks
                .iter()
                .rev()
                .find(|tick| {
                    tick.time_ms <= stamp as f64 && tick.price.is_finite() && tick.price > 0.0
                })
                .map(|tick| f64::from(tick.price))
        };
        if let Some(price) = print_at(stamp) {
            self.fact_vs_buy_print
                .push((deal.buy_price / price - 1.0) * 100.0);
        }
        if let Some(price) = trace.limit {
            self.fact_vs_limit
                .push((deal.buy_price / price - 1.0) * 100.0);
        }
        if let Some(price) = trace.detect_ms.and_then(print_at) {
            self.fact_vs_detect_print
                .push((deal.buy_price / price - 1.0) * 100.0);
        }
        if let Some(reading) = params.drop_at(ticks, stamp) {
            self.at_buy += 1;
            self.at_buy_hit += usize::from(reading.drop_pct >= params.price_delta_pct);
        }
        if let (Some(reading), Some(comment)) = (trace.reading, deal.drops_detect) {
            let error = (reading.drop_pct - comment.x_price_delta_pct).abs();
            self.comment_errors.push(error);
            self.comment_hit += usize::from(error <= 0.3);
        }
        let dt = trace.detect_ms.map(|t| t - stamp);
        let detect_ok = dt.is_some_and(|dt| dt.abs() <= 2_000);
        if let Some(dt) = dt {
            self.detect_errors.push(dt.abs() as f64 / 1_000.0);
        }
        let error = trace
            .limit
            .map(|p| (p / deal.buy_price - 1.0).abs() * 100.0);
        let price_ok = error.is_some_and(|e| e <= PRICE_TOLERANCE_PCT);
        if let Some(error) = error {
            self.price_errors.push(error);
        }
        self.detect_hit += usize::from(detect_ok);
        self.price_hit += usize::from(price_ok);
        // An unfilled order is never evidence of reproducing an actual deal.
        if detect_ok && price_ok && trace.fill.is_some() {
            self.joint += 1;
        } else if dt.is_none() {
            self.no_detect += 1;
        } else if trace.fill.is_none() {
            self.no_fill += 1;
        } else if dt.is_some_and(|dt| dt < -2_000) {
            self.early += 1;
            self.early_signed_dt.push(dt.unwrap() as f64 / 1_000.0);
        } else if dt.is_some_and(|dt| dt > 2_000) {
            self.late += 1;
        } else {
            self.price_off += 1;
        }
    }

    /// Print denominators explicitly so absent model outcomes cannot inflate the joint share.
    fn print(&self, label: &str, authorises: bool) {
        eprintln!("PASS: {label} authorises_gate={authorises}");
        eprintln!("degenerate params (excluded): {}", self.degenerate);
        eprintln!(
            "refused (short tape): {} missing tape/coverage: {} replayable: {}",
            self.refused, self.missing_tape, self.replayable
        );
        eprintln!(
            "detect within +-2 s: {}/{} ({:.2}%) median |dt|={} s",
            self.detect_hit,
            self.replayable,
            share(self.detect_hit, self.replayable),
            median(&self.detect_errors)
        );
        eprintln!(
            "limit within {PRICE_TOLERANCE_PCT}%: {}/{} ({:.2}%) median error={} %",
            self.price_hit,
            self.replayable,
            share(self.price_hit, self.replayable),
            median(&self.price_errors)
        );
        eprintln!(
            "JOINT detect AND price AND fill: {}/{} ({:.2}%)",
            self.joint,
            self.replayable,
            share(self.joint, self.replayable)
        );
        eprintln!(
            "|model drop at fire - comment xPriceDelta|: median={} pp within 0.3 pp={}/{} ({:.2}%)",
            median(&self.comment_errors),
            self.comment_hit,
            self.comment_errors.len(),
            share(self.comment_hit, self.comment_errors.len())
        );
        eprintln!(
            "model drop at buy_set >= DropsPriceDelta: {}/{} ({:.2}%)",
            self.at_buy_hit,
            self.at_buy,
            share(self.at_buy_hit, self.at_buy)
        );
        eprintln!(
            "miss buckets: no detect={} no fill={} early={} late={} price off={}",
            self.no_detect, self.no_fill, self.early, self.late, self.price_off
        );
        for (reference, values) in [
            ("buy_set print", &self.fact_vs_buy_print),
            ("model limit", &self.fact_vs_limit),
            ("model detect print", &self.fact_vs_detect_print),
        ] {
            eprintln!(
                "fact buy_price vs {reference} signed %: n={} p25={} median={} p75={} within {PRICE_TOLERANCE_PCT}%={}/{} ({:.2}%)",
                values.len(),
                percentile(values, 0.25),
                median(values),
                percentile(values, 0.75),
                values
                    .iter()
                    .filter(|v| v.abs() <= PRICE_TOLERANCE_PCT)
                    .count(),
                values.len(),
                share(
                    values
                        .iter()
                        .filter(|v| v.abs() <= PRICE_TOLERANCE_PCT)
                        .count(),
                    values.len()
                )
            );
        }
        eprintln!(
            "early bucket median signed detect dt (model - fact)={} s",
            median(&self.early_signed_dt)
        );
        if authorises {
            if self.replayable == 0 {
                eprintln!("GATE: FAIL (no eligible)");
            }
            eprintln!(
                "GATE: {} joint={:.2}% n={}",
                if self.replayable > 0 && self.joint * 5 >= self.replayable * 4 {
                    "PASS"
                } else {
                    "FAIL"
                },
                share(self.joint, self.replayable),
                self.replayable
            );
        } else {
            eprintln!("EXTENDED: diagnostic only; does not authorise the gate");
        }
    }
}

/// Zero denominators remain zero; the gate separately refuses the empty population.
fn share(hits: usize, n: usize) -> f64 {
    if n == 0 {
        0.0
    } else {
        hits as f64 * 100.0 / n as f64
    }
}

/// Median of observed outcomes only, with absence printed explicitly.
fn median(values: &[f64]) -> String {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n == 0 {
        "n/a".to_string()
    } else {
        format!("{:.4}", (sorted[(n - 1) / 2] + sorted[n / 2]) / 2.0)
    }
}

/// Linearly interpolate the ordered diagnostic population without fabricating absent values.
fn percentile(values: &[f64], fraction: f64) -> String {
    if values.is_empty() {
        return "n/a".to_string();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = (sorted.len() - 1) as f64 * fraction;
    let lower = index.floor() as usize;
    let upper = index.ceil() as usize;
    format!(
        "{:.4}",
        sorted[lower] + (sorted[upper] - sorted[lower]) * index.fract()
    )
}

/// Query a single core-venue market, never merge competing market tapes into a synthetic one.
fn tape_for(
    deal: &Deal,
    venue: &str,
    pairs: &[(String, String)],
    spans: &Coverage,
    required: &Coverage,
) -> Option<Vec<Tick>> {
    let coin = coin_match_key(&deal.coin);
    pairs
        .iter()
        .filter(|(e, m)| e == venue && coin_match_key(coin_of_market(m)) == coin)
        .find_map(|(exchange, market)| {
            let (mut ticks, covered) = held_ticks(exchange, market, spans);
            if ticks.is_empty() || !covered.covers(required) {
                return None;
            }
            ticks.retain(|tick| {
                spans
                    .spans()
                    .iter()
                    .any(|&(from, to)| tick.time_ms >= from as f64 && tick.time_ms <= to as f64)
            });
            ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
            (!ticks.is_empty()).then_some(ticks)
        })
}

/// Compare real detects with clipped and extended replays, failing closed on unknown defaults.
#[test]
#[ignore = "needs a live data root in MOON_TICKS_DATA_DIR"]
fn real_data_drops_calibration() {
    let Some(root) = std::env::var_os("MOON_TICKS_DATA_DIR") else {
        eprintln!("MOON_TICKS_DATA_DIR is not set; GATE: FAIL (no eligible)");
        return;
    };
    if !paths::set_data_dir_override(PathBuf::from(root)) {
        eprintln!("data root override unavailable; GATE: FAIL (no eligible)");
        return;
    }
    let Some(_permit) = crate::db::report_recovery::prepare() else {
        eprintln!("reports replica lease unavailable; GATE: FAIL (no eligible)");
        return;
    };
    let scope = Query {
        from: -1,
        to: crate::db::analytics::ANALYTICS_HORIZON_SECS,
        metric: crate::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let read = match read_deals(&scope) {
        Ok(read) => read,
        Err(_) => {
            eprintln!("deals read failed; GATE: FAIL (no eligible)");
            return;
        }
    };
    let pairs = match crate::market::trade_replay::trade_cache::handle()
        .and_then(|cache| cache.inventory())
    {
        Some(Ok(inventory)) => inventory.keys,
        _ => {
            eprintln!("held tape inventory unavailable; GATE: FAIL (no eligible)");
            return;
        }
    };
    let defaults: HashMap<String, f64> = std::env::var("MOON_TICKS_DEFAULTS")
        .map(|spec| {
            spec.split(',')
                .filter_map(|p| p.split_once('='))
                .filter_map(|(k, v)| {
                    let value: f64 = v.trim().parse().ok()?;
                    value
                        .is_finite()
                        .then(|| (k.trim().to_ascii_lowercase(), value))
                })
                .collect()
        })
        .unwrap_or_default();
    let layout = crate::config::layout::WindowLayout::load(crate::config::ProfileAge::Established);
    let min_tail_s = layout
        .analytics_ticks
        .as_ref()
        .and_then(|t| t.min_tail_s)
        .unwrap_or(tail::DEFAULT_MIN_TAIL_S);
    let margin = crate::market::trade_replay::margin_ms();
    let tail_ms = tail::record_tail_ms(min_tail_s, margin);
    let venues = core_venues();
    let keys = param_keys();
    let mut kinds = BTreeMap::<String, usize>::new();
    let (mut seen, mut no_buy_set, mut no_strategy, mut no_venue, mut short) = (0, 0, 0, 0, 0);
    let (mut fallback_rows, mut unknown_defaults) = (0, 0);
    let mut fallback_fields = BTreeMap::<&str, usize>::new();
    let mut clipped = Metrics::default();
    let mut extended = Metrics::default();
    let mut explicit = (Metrics::default(), Metrics::default());
    let candidates = [2, 5, 10, 20];
    let mut sensitivity: Vec<(Metrics, Metrics)> =
        candidates.iter().map(|_| Default::default()).collect();
    let mut explicit_ma = BTreeMap::<u32, usize>::new();
    let mut sensitivity_rows = 0;
    for mut deal in read.deals {
        *kinds.entry(deal.kind.clone()).or_default() += 1;
        if deal.kind != KIND_DROPS {
            continue;
        }
        seen += 1;
        if seen % 100 == 0 {
            eprintln!("calibration progress: seen={seen}");
        }
        let Some(stamp) = deal.buy_set_ms else {
            no_buy_set += 1;
            continue;
        };
        if !deal.is_long() {
            short += 1;
            continue;
        }
        let Some(values) =
            strategy_values_at(deal.strategy_id, Some(deal.core_uid), deal.buy_ms, &keys)
        else {
            no_strategy += 1;
            continue;
        };
        let sv = StrategyValues {
            values: &values,
            defaults: &defaults,
        };
        let empty_defaults = HashMap::new();
        let raw = StrategyValues {
            values: &values,
            defaults: &empty_defaults,
        };
        let ma = raw.num("DropsPriceMA", f64::NAN);
        if ma.is_finite() {
            *explicit_ma.entry(ma.max(0.0) as u32).or_default() += 1;
        }
        let mut fallback = false;
        let mut unknown = false;
        let mut only_unknown_ma = true;
        for &(key, is_bool) in &INPUTS {
            let valid = if is_bool {
                raw.bool(key, false) == raw.bool(key, true)
            } else {
                // NaN is a sentinel: num() filters malformed raw numbers before using it.
                raw.num(key, f64::NAN).is_finite()
            };
            if !valid {
                fallback = true;
                *fallback_fields.entry(key).or_default() += 1;
                let missing_default = !defaults.contains_key(&key.to_ascii_lowercase());
                unknown |= missing_default;
                only_unknown_ma &= !missing_default || key == "DropsPriceMA";
            }
        }
        fallback_rows += usize::from(fallback);
        unknown_defaults += usize::from(unknown);
        if unknown && !only_unknown_ma {
            continue;
        }
        let Some(venue) = venues.get(&deal.core_uid) else {
            no_venue += 1;
            continue;
        };
        let ui_ticks = model_window(&deal, margin, long_position_ms()).and_then(|window| {
            let spans = tail::focus_with_tail(&window, tail_ms);
            tape_for(&deal, venue, &pairs, &spans, &required_spans(&window))
        });
        // Reuse exactly the same UI tape across sensitivity candidates; only extended queries vary.
        let mut replay = |params: &DropsParams, clipped: &mut Metrics, extended: &mut Metrics| {
            if let Some(ticks) = &ui_ticks {
                deal.tick = infer_tick(ticks);
                clipped.observe(&deal, ticks, params);
            } else {
                clipped.missing_tape += 1;
            }
            let spans = Coverage::one((
                stamp - SEARCH_BEFORE_MS - params.needed_history_ms(),
                deal.close_ms,
            ));
            if let Some(ticks) = tape_for(&deal, venue, &pairs, &spans, &spans) {
                deal.tick = infer_tick(&ticks);
                extended.observe(&deal, &ticks, params);
            } else {
                extended.missing_tape += 1;
            }
        };
        if unknown {
            sensitivity_rows += 1;
            for (candidate, (ui, extra)) in candidates.iter().zip(&mut sensitivity) {
                let mut assumed = defaults.clone();
                assumed.insert("dropspricema".to_string(), f64::from(*candidate));
                let params = drops_params(&StrategyValues {
                    values: &values,
                    defaults: &assumed,
                });
                replay(&params, ui, extra);
            }
        } else {
            let params = drops_params(&sv);
            replay(&params, &mut clipped, &mut extended);
            if !fallback {
                replay(&params, &mut explicit.0, &mut explicit.1);
            }
        }
    }
    eprintln!(
        "DROPS CALIBRATION margin_ms={margin} min_tail_s={min_tail_s} long_position_ms={}",
        long_position_ms()
    );
    eprintln!(
        "seen={seen} no buy_set={no_buy_set} short unsupported={short} no strategy={no_strategy} no venue={no_venue}"
    );
    eprintln!(
        "strategy default/fallback rows={fallback_rows} unknown-default rows ineligible={unknown_defaults} supplied defaults={}",
        defaults.len()
    );
    eprintln!("fallback fields: {fallback_fields:?}");
    eprintln!("kinds_seen: {kinds:?}");
    eprintln!(
        "DropsPriceMA explicit values (each excluded as an omitted default): {explicit_ma:?}"
    );
    eprintln!(
        "No unique omitted numeric default established by exclusion; sensitivity rows={sensitivity_rows}"
    );
    explicit
        .1
        .print("fully explicit raw inputs / extended", false);
    explicit
        .0
        .print("fully explicit raw inputs / UI held tape", false);
    for (candidate, (ui, extra)) in candidates.iter().zip(&sensitivity) {
        eprintln!(
            "SENSITIVITY DropsPriceMA={candidate} explicitly_seen={} omitted rows only; other defaults supplied assumptions",
            explicit_ma.contains_key(candidate)
        );
        extra.print("sensitivity / extended", false);
        ui.print("sensitivity / UI held tape", false);
    }
    eprintln!(
        "RESOLVED INPUTS: excludes unknown numeric defaults; MOON_TICKS_DEFAULTS are caller-supplied assumptions"
    );
    extended.print("extended", false);
    clipped.print("UI held tape", true);
}

/// Empty and even populations must not silently become an inflated median or percentage.
#[test]
fn calibration_summary_handles_empty_and_even_samples() {
    assert_eq!(median(&[]), "n/a");
    assert_eq!(median(&[9.0, 1.0, 5.0, 3.0]), "4.0000");
    assert_eq!(share(0, 0), 0.0);
    assert_eq!(share(1, 4), 25.0);
}
