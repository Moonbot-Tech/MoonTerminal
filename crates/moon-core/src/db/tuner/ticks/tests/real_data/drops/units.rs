//! Bucket fits reuse one sampled tape per deal and retain missing inputs as misses.
use super::*;
use crate::db::tuner::ticks::drops::units::Units;
use std::collections::BTreeMap;

/// Cached high means and current means at factual placement, independent of fitted parameters.
pub(super) struct Cached {
    high: Vec<Vec<Option<f64>>>,
    last: Vec<Option<f64>>,
    target: f64,
}

impl Cached {
    /// Sample each endpoint once, then extend each high maximum backwards through the window.
    pub(super) fn new(row: &Replay, ticks: &[Tick]) -> Self {
        let t = row.deal.buy_set_ms.expect("placement");
        let v: Vec<_> = (0..=140)
            .map(|i| {
                let time = t - (140 - i) * 2000;
                crate::db::tuner::ticks::drops::measurement_last_price(ticks, time)
            })
            .collect();
        let mut high = vec![vec![None; 120]; 20];
        for k in 1..=20 {
            let mut best: Option<f64> = None;
            let mut missing = false;
            for n in 1..=120 {
                if n >= k {
                    let zone = &v[141 - n..141 - n + k];
                    if let Some(sum) = zone.iter().copied().sum::<Option<f64>>() {
                        let mean = sum / k as f64;
                        best = Some(best.map_or(mean, |x| x.max(mean)));
                    } else {
                        missing = true;
                    }
                }
                high[k - 1][n - 1] = if missing || v[140 - n].is_none() {
                    None
                } else {
                    best
                };
            }
        }
        let last = (0..=6)
            .map(|j| {
                if j == 0 {
                    v[140]
                } else {
                    v[141 - j..]
                        .iter()
                        .copied()
                        .sum::<Option<f64>>()
                        .map(|s| s / j as f64)
                }
            })
            .collect();
        Self {
            high,
            last,
            target: row.deal.drops_detect.expect("comment").x_price_delta_pct,
        }
    }
    /// Missing readings receive infinite loss so no grid point improves by dropping hard deals.
    pub(super) fn error(&self, w: usize, k: usize, j: usize) -> f64 {
        self.high[k - 1][w - 1]
            .zip(self.last[j])
            .map_or(f64::INFINITY, |(h, l)| {
                ((h / l - 1.0) * 100.0 - self.target).abs()
            })
    }
}

/// Numeric median retains infinities for fixed-population fit comparisons.
fn med(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    (v[(v.len() - 1) / 2] + v[v.len() / 2]) / 2.0
}

/// Fit each sufficiently populated parameter bucket, then score all unit combinations.
pub(super) fn batch(pool: &Pool<'_>, ex: (usize, usize, usize)) {
    score_probe(pool, "best_today_include_baseline", Probe::default(), ex);
    let mut buckets: BTreeMap<(u32, u32, u32), Vec<Cached>> = BTreeMap::new();
    for &(r, t, p) in pool {
        buckets
            .entry((p.max_time_s, p.price_ma_s, p.last_price_ma))
            .or_default()
            .push(Cached::new(r, t));
    }
    let mut fits = Vec::new();
    for (&key, rows) in &buckets {
        if rows.len() < 10 {
            continue;
        }
        let mut best = (f64::INFINITY, 0, 1, 1, 0);
        for w in 1..=120 {
            for k in 1..=20 {
                for j in 0..=6 {
                    let mut errors: Vec<_> = rows.iter().map(|r| r.error(w, k, j)).collect();
                    let hits = errors.iter().filter(|e| **e <= 0.3).count();
                    let loss = med(&mut errors);
                    if loss < best.0 || (loss == best.0 && hits > best.1) {
                        best = (loss, hits, w, k, j);
                    }
                }
            }
        }
        eprintln!(
            "FIT max_time={} price_ma={} last_ma={} n={} window_s={} k={} j={} median_abs_err_pp={:.4} within03={:.2} missing_as_infinite=true",
            key.0,
            key.1,
            key.2,
            rows.len(),
            best.2 * 2,
            best.3,
            best.4,
            best.0,
            share(best.1, rows.len())
        );
        fits.push((key, (best.2 as u32 * 2, best.3, best.4)));
    }
    let mut selected = (f64::INFINITY, Units::default(), String::new());
    for window in 0..3 {
        for high in 0..3 {
            for last_seconds in [false, true] {
                for extend in [false, true] {
                    let u = Units {
                        window,
                        high,
                        last_seconds,
                        extend,
                        explicit: None,
                    };
                    let name = format!(
                        "units_w{window}_h{high}_l{}_e{}",
                        u8::from(last_seconds),
                        u8::from(extend)
                    );
                    score_probe(
                        pool,
                        &name,
                        Probe {
                            units: Some(u),
                            ..Default::default()
                        },
                        ex,
                    );
                    let mut errors = Vec::new();
                    for &(r, t, p) in pool {
                        if buckets[&(p.max_time_s, p.price_ma_s, p.last_price_ma)].len() < 10 {
                            continue;
                        }
                        let target = r.deal.drops_detect.expect("comment").x_price_delta_pct;
                        errors.push(
                            u.reading(p, t, r.deal.buy_set_ms.expect("placement"))
                                .map_or(f64::INFINITY, |v| (v.drop_pct - target).abs()),
                        );
                    }
                    if !errors.is_empty() {
                        let loss = med(&mut errors);
                        if loss < selected.0 {
                            selected = (loss, u, name);
                        }
                    }
                }
            }
        }
    }
    let exact = fits
        .iter()
        .filter(|(key, point)| {
            let p = DropsParams {
                max_time_s: key.0,
                price_ma_s: key.1,
                last_price_ma: key.2,
                ..Default::default()
            };
            let (w, k, j, step) = selected.1.dimensions(&p);
            step == 2000 && !selected.1.extend && (w, k, j) == *point
        })
        .count();
    eprintln!(
        "FIT_MAPPING selected={} pool_n={} selection_n={} bucket_exact={} buckets={} universal_exact={} median_abs_err_pp={:.4} selection=in_sample gate_authorised=false w0=seconds w1=double_seconds w2=floor30 h0=seconds h1=samples h2=one_second_zones l0=samples l1=seconds e1=extended",
        selected.2,
        pool.len(),
        buckets
            .values()
            .filter(|rows| rows.len() >= 10)
            .map(Vec::len)
            .sum::<usize>(),
        exact,
        fits.len(),
        !fits.is_empty() && exact == fits.len(),
        selected.0
    );
    score_probe(
        pool,
        "mapping_selected",
        Probe {
            units: Some(selected.1),
            ..Default::default()
        },
        ex,
    );
    score_probe(
        pool,
        "mapping_selected_lag1s",
        Probe {
            units: Some(selected.1),
            lag_s: 1,
            ..Default::default()
        },
        ex,
    );
    price_diagnostics(pool, selected.1, &fits);
    // Bucket lookup is an in-sample diagnostic upper bound, not a proposed production formula.
    for lag_s in [0, 1] {
        let mut metrics = Metrics::default();
        let mut fallback = 0;
        for &(row, ticks, p) in pool {
            let point = fits
                .iter()
                .find(|(key, _)| *key == (p.max_time_s, p.price_ma_s, p.last_price_ma))
                .map(|(_, point)| *point);
            fallback += usize::from(point.is_none());
            let units = point.map_or(selected.1, |point| Units {
                explicit: Some(point),
                ..Default::default()
            });
            let probe = Probe {
                units: Some(units),
                lag_s,
                ..Default::default()
            };
            let mut deal = row.deal.clone();
            deal.tick = infer_tick(ticks);
            metrics.observe_trace(
                row,
                probe.trace(p, &deal, ticks, ticks, None),
                probe.reading(p, ticks, deal.buy_set_ms.expect("placement")),
            );
        }
        metrics.print(
            &format!("mapping_bucket_lookup_lag{lag_s}s"),
            ex.0,
            ex.1,
            ex.2,
        );
        eprintln!(
            "DIAG fitted_bucket_mapping lag_s={lag_s} fallback={fallback} selection=in_sample lookup_not_universal=true gate_authorised=false"
        );
    }
}

/// A fitted parameter bucket and its window/high/last sample dimensions.
type BucketFit = ((u32, u32, u32), (u32, usize, usize));

/// Compare factual prices against the selected formula and contemporaneous print on the UI cohort.
fn price_diagnostics(pool: &Pool<'_>, units: Units, fits: &[BucketFit]) {
    let mut fact_print = Vec::new();
    let mut fact_limit = Vec::new();
    let mut best_limit = Vec::new();
    let mut missing_dump = 0;
    let mut signs = [0; 3];
    for &(row, ticks, p) in pool {
        let stamp = row.deal.buy_set_ms.expect("placement");
        if let Some(buy_price) = raw_number(row, "buyPrice") {
            signs[if buy_price < 0.0 {
                0
            } else if buy_price == 0.0 {
                1
            } else {
                2
            }] += 1;
        } else {
            missing_dump += 1;
        }
        if let Some(print) = crate::db::tuner::ticks::drops::measurement_last_price(ticks, stamp) {
            let value = (row.deal.buy_price / print - 1.0) * 100.0;
            fact_print.push(value);
        }
        let mut deal = row.deal.clone();
        deal.tick = infer_tick(ticks);
        let probe = Probe {
            units: Some(units),
            ..Default::default()
        };
        if let Some(limit) = probe.trace(p, &deal, ticks, ticks, None).limit {
            fact_limit.push((row.deal.buy_price / limit - 1.0) * 100.0);
        }
        let point = fits
            .iter()
            .find(|(key, _)| *key == (p.max_time_s, p.price_ma_s, p.last_price_ma))
            .map(|(_, point)| *point);
        let fitted = point.map_or(units, |point| Units {
            explicit: Some(point),
            ..Default::default()
        });
        let best = Probe {
            units: Some(fitted),
            ..Default::default()
        };
        if let Some(limit) = best.trace(p, &deal, ticks, ticks, None).limit {
            best_limit.push((row.deal.buy_price / limit - 1.0) * 100.0);
        }
    }
    distribution("price_fact_over_buy_set_print_pct", &fact_print, false);
    distribution("price_fact_over_selected_limit_pct", &fact_limit, false);
    distribution("price_fact_over_best_bucket_limit_pct", &best_limit, false);
    distribution("price_implied_buy_price_pct", &fact_print, false);
    eprintln!(
        "DIAG dump_buy_price_sign n={} missing={missing_dump} negative_pct={:.4} zero_pct={:.4} positive_pct={:.4}",
        pool.len(),
        share(signs[0], pool.len()),
        share(signs[1], pool.len()),
        share(signs[2], pool.len())
    );
}
