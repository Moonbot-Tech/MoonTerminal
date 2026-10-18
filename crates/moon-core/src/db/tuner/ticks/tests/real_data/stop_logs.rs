//! Core-log agreement and held-out cadence diagnostics on the reproduction bench's population.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};

use super::core_log::{self, CoreEvent, DealKey, Match, StopFire, TakeBranch};
use crate::db::tuner::ticks::exit::line::LineWalk;
use crate::db::tuner::ticks::exit::stops::StopTrigger;
use crate::db::tuner::ticks::{
    Deal, ExitKind, ExitModel, ExitParams, Fill, ModelSettings, record, verify,
};
use crate::feed::types::{Side, Tick};

#[cfg(test)]
mod tests;

/// Missing model stops contribute one minute of absolute timing error.
const MISS_PENALTY_MS: i64 = 60_000;

/// Cap total retained tape memory across all deals.
const RETAIN_BUDGET_PRINTS: usize = 150_000_000;

/// UTC-day split; only held-out scores support later model decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Half {
    Fit,
    Held,
}

/// Split on absolute UTC day parity, independent of the sampled date range.
pub(super) fn day_half(ms: i64) -> Half {
    if ms.div_euclid(86_400_000).rem_euclid(2) == 0 {
        Half::Fit
    } else {
        Half::Held
    }
}

/// One deal with only the bounded warm-up and close tail needed for repeated walks.
struct Kept {
    deal: Deal,
    ticks: Vec<Tick>,
    exit: ExitParams,
    fill: Fill,
}

/// Opt-in collector; disabled runs allocate no retained tapes and perform no extra I/O.
pub(super) struct StopLogDiag {
    active: bool,
    kept: Vec<Kept>,
    skipped_big: usize,
    skipped_window: usize,
    dropped_budget: usize,
    retained_prints: usize,
}

impl StopLogDiag {
    /// Enable diagnostics only when the bench's explicit environment switch exists.
    pub(super) fn from_env() -> Self {
        Self {
            active: std::env::var_os("MOON_TICKS_STOP_LOGS").is_some(),
            kept: Vec::new(),
            skipped_big: 0,
            skipped_window: 0,
            dropped_budget: 0,
            retained_prints: 0,
        }
    }

    /// Retain every covered deal, including factual non-stops for false-stop accounting.
    pub(super) fn add(
        &mut self,
        deal: &Deal,
        ticks: &[Tick],
        exit: &ExitParams,
        exit_points: Option<&[(i64, f64)]>,
    ) {
        if !self.active {
            return;
        }
        let fill = verify::fact_sell_start(deal, exit, exit_points);
        let from = fill.t_ms.saturating_sub(600_000);
        let to = deal.close_ms.saturating_add(60_000);
        if to < from {
            self.skipped_window += 1;
            return;
        }
        let start = ticks.partition_point(|tick| (tick.time_ms as i64) < from);
        let end = ticks.partition_point(|tick| (tick.time_ms as i64) <= to);
        let ticks = &ticks[start..end];
        if ticks.len() > 200_000 {
            self.skipped_big += 1;
            return;
        }
        if self.dropped_budget > 0
            || self.retained_prints.saturating_add(ticks.len()) > RETAIN_BUDGET_PRINTS
        {
            self.dropped_budget += 1;
            return;
        }
        self.retained_prints += ticks.len();
        self.kept.push(Kept {
            deal: deal.clone(),
            ticks: ticks.to_vec(),
            exit: exit.clone(),
            fill,
        });
    }

    /// Read logs once, then print agreement tables and fit-only cadence selection.
    pub(super) fn print(&self) {
        if !self.active {
            return;
        }
        eprintln!(
            "stop logs: kept {} skipped_big {} skipped_window {} dropped_budget {} retained prints {} (trim: fill - 10 min through close + 60 s)",
            self.kept.len(),
            self.skipped_big,
            self.skipped_window,
            self.dropped_budget,
            self.retained_prints
        );
        let Some(from) = self.kept.iter().map(|k| day_start(k.deal.buy_ms)).min() else {
            return;
        };
        let to = self
            .kept
            .iter()
            .map(|k| day_start(k.deal.close_ms))
            .max()
            .unwrap_or(from);
        let dir = crate::config::paths::logs_dir();
        let started = Instant::now();
        let (logs, files) = match core_log::read_core_logs(&dir, from, to) {
            Ok(logs) => logs,
            Err(_) => {
                eprintln!("stop logs: log read failed; agreement unavailable");
                return;
            }
        };
        let stops = logs
            .values()
            .flatten()
            .filter(|e| matches!(e, CoreEvent::Stop(_)))
            .count();
        let sells = logs
            .values()
            .flatten()
            .filter(|e| matches!(e, CoreEvent::Sell(_)))
            .count();
        eprintln!(
            "stop logs: {files} files' events, {stops} stop fires, {sells} sell lines, read in {:.2} s",
            started.elapsed().as_secs_f64()
        );
        let ids = task_ids();
        let mut routes: BTreeMap<(String, Half), Routes> = BTreeMap::new();
        let mut timing: BTreeMap<(String, Half), Timing> = BTreeMap::new();
        let mut population: BTreeMap<(String, Half), Population> = BTreeMap::new();
        let mut readings: BTreeMap<(String, Half), Readings> = BTreeMap::new();
        let mut takes: BTreeMap<(String, Half), TakeScore> = BTreeMap::new();
        let mut clocks: BTreeMap<(String, Half), Vec<f64>> = BTreeMap::new();
        let mut prices: BTreeMap<(String, Half), Vec<f64>> = BTreeMap::new();
        let mut matched = Vec::with_capacity(self.kept.len());
        let mut defaults = Vec::with_capacity(self.kept.len());
        let keys: Vec<_> = self
            .kept
            .iter()
            .map(|kept| {
                let deal = &kept.deal;
                let label = core_log::label_of_core(&deal.core_name);
                (
                    label,
                    DealKey {
                        strategy: None,
                        coin: &deal.coin,
                        task: ids
                            .get(&(deal.core_uid, deal.report_uid))
                            .copied()
                            .filter(|id| *id != 0),
                        buy_ms: deal.buy_ms,
                        close_ms: deal.close_ms,
                    },
                )
            })
            .collect();
        let mut stop_matches: Vec<_> = keys
            .iter()
            .map(|(label, key)| {
                core_log::match_stop(key, logs.get(label).map(Vec::as_slice).unwrap_or(&[]))
            })
            .collect();
        let mut sell_matches: Vec<_> = keys
            .iter()
            .map(|(label, key)| {
                core_log::match_sell(key, logs.get(label).map(Vec::as_slice).unwrap_or(&[]), 10)
            })
            .collect();
        let stop_ids: Vec<_> = keys.iter().zip(&stop_matches).map(|((label, key), result)| {
            matched_value(result).map(|fire| {
                let index = logs[label].iter().position(|event| matches!(event, CoreEvent::Stop(candidate) if std::ptr::eq(candidate, fire))).expect("matched event belongs to label");
                ((label.clone(), index), fire.t_ms.abs_diff(key.close_ms))
            })
        }).collect();
        let sell_ids: Vec<_> = keys.iter().zip(&sell_matches).map(|((label, key), result)| {
            matched_value(result).map(|sell| {
                let index = logs[label].iter().position(|event| matches!(event, CoreEvent::Sell(candidate) if std::ptr::eq(candidate, sell))).expect("matched event belongs to label");
                ((label.clone(), index), sell.t_ms.abs_diff(key.buy_ms))
            })
        }).collect();
        let (lost, shared) = exclusive_owners(&stop_ids);
        let (lost_sells, shared_sells) = exclusive_owners(&sell_ids);
        eprintln!("fires shared by several deals: {shared} (kept on nearest close)");
        eprintln!("sell lines shared by several deals: {shared_sells} (kept on nearest buy)");
        for (i, result) in stop_matches.iter_mut().enumerate() {
            if lost[i] {
                *result = Match::None;
            }
        }
        for (i, result) in sell_matches.iter_mut().enumerate() {
            if lost_sells[i] {
                *result = Match::None;
            }
        }
        for (i, ((kept, stop_match), sell_match)) in self
            .kept
            .iter()
            .zip(stop_matches)
            .zip(sell_matches)
            .enumerate()
        {
            let deal = &kept.deal;
            let half = day_half(deal.buy_ms);
            let kind = ascii(&deal.kind);
            routes
                .entry((format!("{kind} stop"), half))
                .or_default()
                .add(&stop_match);
            routes
                .entry((format!("{kind} sell"), half))
                .or_default()
                .add(&sell_match);
            let fire = matched_value(&stop_match);
            let walked = walk(kept, None);
            let stop_time = stop_time(&walked);
            let pop = population.entry((kind.clone(), half)).or_default();
            if verify::is_stop_reason(&deal.sell_reason) {
                pop.fact_stops += 1;
                pop.no_log += usize::from(matches!(stop_match, Match::None) && !lost[i]);
                pop.lost += usize::from(lost[i]);
                pop.ambiguous += usize::from(matches!(stop_match, Match::Ambiguous(_)));
            } else {
                pop.non_stops += 1;
                pop.false_early += usize::from(is_early(stop_time, deal.close_ms));
            }
            if let Some(fire) = fire {
                for row in [
                    kind.clone(),
                    format!("{kind} / {}", trigger_name(walked.stop_trigger)),
                ] {
                    timing.entry((row, half)).or_default().add(
                        stop_time,
                        fire.t_ms,
                        kept.exit.model.book_stop_time_ms,
                    );
                }
                if let Some(clock) = fire.msg_clock_s {
                    clocks
                        .entry((kind.clone(), half))
                        .or_default()
                        .push(clock_offset_s(fire.t_ms.div_euclid(1000), clock) as f64);
                }
                let group = format!(
                    "{kind} fast={} EMA={}",
                    kept.exit.fast_stop_loss,
                    ema_bucket(kept.exit.stop_loss_ema)
                );
                readings.entry((group, half)).or_default().add(kept, fire);
                if stop_time.is_some() {
                    let logged = if deal.is_long() {
                        fire.bid
                    } else {
                        Some(fire.ask)
                    };
                    if let Some(error) =
                        logged.and_then(|price| error_pct(walked.exit.price, price))
                    {
                        prices
                            .entry((format!("{kind} fire price vs logged bid/ask"), half))
                            .or_default()
                            .push(error);
                    }
                }
                if let (Some(model), Some(fixed)) = (walked.stop_level, fire.fixed) {
                    if let Some(error) = error_pct(model, fixed) {
                        prices
                            .entry((format!("{kind} stop level vs logged fixed"), half))
                            .or_default()
                            .push(error);
                    }
                }
            }
            if deal.kind != "Spread" {
                if let Some(placed) = matched_value(&sell_match) {
                    let branch = core_log::branch_of(
                        placed,
                        deal.buy_price,
                        deal.is_long(),
                        kept.exit.model.price_pct,
                    );
                    let take = ExitModel::new(&kept.exit).take_level(
                        &record::unanchored(deal),
                        &kept.ticks,
                        kept.fill,
                    );
                    let agrees = core_log::within_pct(placed.sell, take, kept.exit.model.price_pct);
                    let row = takes
                        .entry((format!("{kind} / {}", branch_name(branch)), half))
                        .or_default();
                    row.n += 1;
                    row.agree += usize::from(agrees);
                }
            }
            matched.push(fire);
            defaults.push((stop_time, walked.stop_trigger));
        }
        print_table(
            "match routes: Task / CoinTime / Ambiguous deals (candidate lines) / None",
            routes.iter().map(|((k, h), r)| {
                (
                    (k.clone(), *h),
                    format!(
                        "{} / {} / {} ({}) / {}",
                        r.task, r.coin, r.ambiguous, r.candidates, r.none
                    ),
                )
            }),
        );
        print_table(
            "stop timing: matched n; model stop n; dt p10/p50/p90; |dt| p10/p50/p90 ms; <=book; <=1000",
            timing.iter().map(|(key, row)| (key.clone(), row.render())),
        );
        print_table(
            "stop price agreement: n, signed median %, p90 absolute %",
            prices
                .iter()
                .map(|(key, values)| (key.clone(), error_summary(values))),
        );
        print_table(
            "population: non-stop n / false early (<close-1000); factual stop n / no log line / ambiguous / lost to nearer deal",
            population.iter().map(|(key, row)| {
                (
                    key.clone(),
                    format!(
                        "{} / {}; {} / {} / {} / {}",
                        row.non_stops,
                        row.false_early,
                        row.fact_stops,
                        row.no_log,
                        row.ambiguous,
                        row.lost
                    ),
                )
            }),
        );
        print_table(
            "log clock offset: median seconds, TSV event clock minus message clock mod 3600",
            clocks
                .iter()
                .map(|(key, v)| (key.clone(), number(quantile(&mut v.clone(), 0.5)))),
        );
        print_table(
            "readings: n; only bid / EMA / last / ask / several / none; proxy n med%,p90abs%; last n med%,p90abs%",
            readings
                .iter()
                .map(|(key, row)| (key.clone(), row.render())),
        );
        print_table(
            "EMA fire gap: (bid_ema - bid)/bid % n, p10/p50/p90",
            readings
                .iter()
                .filter(|((key, _), _)| !key.ends_with("EMA=0"))
                .map(|(key, row)| {
                    (
                        key.clone(),
                        format!("{} {}", row.ema_gap.len(), triplet(&row.ema_gap)),
                    )
                }),
        );
        eprintln!("EMA span: not identifiable from fire lines (one record per fire)");
        print_table(
            "take agreement (Spread excluded): branch n / agree / share",
            takes.iter().map(|(key, row)| {
                (
                    key.clone(),
                    format!("{} / {} / {}", row.n, row.agree, share(row.agree, row.n)),
                )
            }),
        );
        for half in [Half::Fit, Half::Held] {
            let n: usize = takes
                .iter()
                .filter(|((key, h), _)| key.starts_with("MoonShot / ") && *h == half)
                .map(|(_, r)| r.n)
                .sum();
            let sell = takes
                .get(&("MoonShot / SellPrice".into(), half))
                .map_or(0, |r| r.n);
            eprintln!("F8 coverage {half:?}: {n} matched MoonShot sells, {sell} SellPrice branch");
        }
        self.cadence(&matched, &defaults);
    }

    /// Fit the production period on fit days only, then score both arms on held days.
    fn cadence(
        &self,
        matched: &[Option<&StopFire>],
        defaults: &[(Option<i64>, Option<StopTrigger>)],
    ) {
        let eligible: Vec<usize> = self
            .kept
            .iter()
            .enumerate()
            .filter(|(i, kept)| {
                matched[*i].is_some() && verify::is_stop_reason(&kept.deal.sell_reason)
            })
            .map(|(i, _)| i)
            .collect();
        let fit_n = eligible
            .iter()
            .filter(|&&i| day_half(self.kept[i].deal.buy_ms) == Half::Fit)
            .count();
        let held_n = eligible.len() - fit_n;
        eprintln!("cadence: all matched factual-stop population Fit {fit_n} Held {held_n}");
        let default = ModelSettings::default().ticker_period_ms;
        let mut scores = Vec::new();
        for period in (1800..=2600).step_by(50) {
            let score = cadence_score(&self.kept, matched, &eligible, Half::Fit, period);
            eprintln!("cadence fit P={period}: {}", score.render());
            scores.push((period, score.errors));
        }
        if fit_n == 0 {
            eprintln!("cadence: no fit-half matched deals; P* unavailable");
            return;
        }
        let period = pick_period(&scores, default);
        let fit = cadence_score(&self.kept, matched, &eligible, Half::Fit, period);
        let base = cadence_score(&self.kept, matched, &eligible, Half::Held, default);
        let tuned = cadence_score(&self.kept, matched, &eligible, Half::Held, period);
        eprintln!("cadence held default P={default}: {}", base.render());
        eprintln!("cadence held P*={period}: {}", tuned.render());
        let ticker_only: Vec<_> = eligible
            .iter()
            .copied()
            .filter(|&i| defaults[i].1 == Some(StopTrigger::Ticker))
            .collect();
        for half in [Half::Fit, Half::Held] {
            for p in [default, period] {
                eprintln!(
                    "ticker-only (secondary) {half:?} P={p}: {}",
                    cadence_score(&self.kept, matched, &ticker_only, half, p).render()
                );
            }
        }
        let mut paired = Vec::new();
        let mut phase = [0usize; 10];
        for &i in &eligible {
            let k = &self.kept[i];
            if day_half(k.deal.buy_ms) != Half::Held {
                continue;
            }
            if let Some(fire) = matched[i] {
                let a = stop_time(&walk(k, Some(default)));
                let b = stop_time(&walk(k, Some(period)));
                paired.push((
                    day_start(k.deal.buy_ms) / 86_400_000,
                    penalised_error(a.map(|a| a - fire.t_ms)),
                    penalised_error(b.map(|b| b - fire.t_ms)),
                ));
                let fraction =
                    (fire.t_ms - (k.fill.t_ms + period)).rem_euclid(period) as f64 / period as f64;
                phase[((fraction * 10.0) as usize).min(9)] += 1;
            }
        }
        let (lo, hi) = bootstrap_diff(&paired, 200, 0x9E3779B97F4A7C15);
        eprintln!(
            "cadence: default {default} held med|dt| {}, P* {period} held med|dt| {}, diff CI [{}, {}] (paired {} / {held_n}; 200 day-block resamples)",
            number(quantile(
                &mut paired.iter().map(|p| p.1).collect::<Vec<_>>(),
                0.5
            )),
            number(quantile(
                &mut paired.iter().map(|p| p.2).collect::<Vec<_>>(),
                0.5
            )),
            number(Some(lo)),
            number(Some(hi)),
            paired.len()
        );
        let allowance = quantile(&mut fit.abs.clone(), 0.9);
        let within = allowance.map(|limit| {
            tuned
                .errors
                .iter()
                .flatten()
                .filter(|&&v| (v as f64).abs() <= limit)
                .count()
        });
        eprintln!(
            "allowance: Fit P* real-stop p90|dt| {} ms (fit misses {}); Held within fit allowance {} vs current book {} (denominator all {held_n} matched factual-stop deals; no model stop is a miss)",
            number(allowance),
            fit.n - fit.dt.len(),
            within.map_or_else(|| "NA".into(), |n| share(n, held_n)),
            share(tuned.book, held_n)
        );
        for p in [default, period] {
            let mut by_kind: BTreeMap<String, Population> = BTreeMap::new();
            for k in &self.kept {
                if day_half(k.deal.buy_ms) == Half::Held
                    && !verify::is_stop_reason(&k.deal.sell_reason)
                {
                    let row = by_kind.entry(ascii(&k.deal.kind)).or_default();
                    row.non_stops += 1;
                    row.false_early +=
                        usize::from(is_early(stop_time(&walk(k, Some(p))), k.deal.close_ms));
                }
            }
            for (kind, row) in by_kind {
                eprintln!(
                    "cadence Held P={p} {kind}: false early {} / {}",
                    row.false_early, row.non_stops
                );
            }
        }
        eprintln!("phase Held P*={period}: 10 bins of ((fire-(fill+P)) mod P)/P: {phase:?}");
        eprintln!(
            "phase: flat suggests no fill anchor; a peak away from zero suggests a fixed offset; diagnostic only"
        );
    }
}

/// Assign each event identity to its nearest deal; ties keep the first deal.
fn exclusive_owners(ids: &[Option<((String, usize), u64)>]) -> (Vec<bool>, usize) {
    let mut owners: HashMap<&(String, usize), (usize, u64, usize)> = HashMap::new();
    for (i, id) in ids.iter().enumerate() {
        if let Some((id, distance)) = id {
            let owner = owners.entry(id).or_insert((i, *distance, 0));
            owner.2 += 1;
            if *distance < owner.1 {
                owner.0 = i;
                owner.1 = *distance;
            }
        }
    }
    let lost = ids
        .iter()
        .enumerate()
        .map(|(i, id)| id.as_ref().is_some_and(|(id, _)| owners[id].0 != i))
        .collect();
    (lost, owners.values().filter(|owner| owner.2 > 1).count())
}

/// Read report task identities without opening a writer or changing production DTOs.
fn task_ids() -> HashMap<(u64, i64), i64> {
    let db = match Connection::open_with_flags(
        crate::config::paths::reports_db_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(db) => db,
        Err(_) => {
            eprintln!("stop logs: reports database unavailable; no task ids");
            return HashMap::new();
        }
    };
    let mut stmt = match db
        .prepare("SELECT core_uid, reportuid, taskid FROM orders_rep WHERE taskid IS NOT NULL")
    {
        Ok(stmt) => stmt,
        Err(_) => {
            eprintln!("stop logs: no taskid column");
            return HashMap::new();
        }
    };
    let rows = match stmt.query_map([], |r| {
        Ok((
            (r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?),
            r.get::<_, i64>(2)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => {
            eprintln!("stop logs: taskid query failed");
            return HashMap::new();
        }
    };
    let mut ids = HashMap::new();
    let mut failed = 0;
    for row in rows {
        match row {
            Ok((key, id)) if id != 0 => {
                ids.insert(key, id);
            }
            Ok(_) => {}
            Err(_) => failed += 1,
        }
    }
    if failed > 0 {
        eprintln!("stop logs: {failed} unreadable taskid rows");
    }
    ids
}

/// Exact independent reading crossings; ask is a separate short-side candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FiredBy {
    OnlyBid,
    OnlyEma,
    OnlyLast,
    OnlyAsk,
    Several,
    None,
}

/// Classify only available readings, with the position's crossing direction.
pub(super) fn fired_by(
    long: bool,
    fixed: f64,
    bid: Option<f64>,
    bid_ema: Option<f64>,
    last: Option<f64>,
    ask: Option<f64>,
) -> FiredBy {
    let crossed = |p: Option<f64>| p.is_some_and(|p| if long { p <= fixed } else { p >= fixed });
    let hits = [
        crossed(bid),
        crossed(bid_ema),
        crossed(last),
        !long && crossed(ask),
    ];
    match hits {
        [true, false, false, false] => FiredBy::OnlyBid,
        [false, true, false, false] => FiredBy::OnlyEma,
        [false, false, true, false] => FiredBy::OnlyLast,
        [false, false, false, true] => FiredBy::OnlyAsk,
        [false, false, false, false] => FiredBy::None,
        _ => FiredBy::Several,
    }
}

/// UTC day start for inclusive file-date selection and day-block resampling.
fn day_start(ms: i64) -> i64 {
    ms.div_euclid(86_400_000) * 86_400_000
}

/// Signed nearest-hour clock difference, including hour-boundary crossings.
fn clock_offset_s(tsv_s: i64, msg_s: i64) -> i64 {
    ((tsv_s - msg_s + 1800).rem_euclid(3600)) - 1800
}

/// Penalise absent stops without capping the error of a stop that occurred.
fn penalised_error(dt: Option<i64>) -> f64 {
    dt.map_or(MISS_PENALTY_MS as f64, |dt| (dt as f64).abs())
}

/// Prefer all-deal one-second agreement, then penalised median, then default proximity.
fn pick_period(scores: &[(i64, Vec<Option<i64>>)], default: i64) -> i64 {
    scores
        .iter()
        .min_by(|(pa, a), (pb, b)| {
            let hits = |v: &[Option<i64>]| {
                v.iter()
                    .filter(|v| v.is_some_and(|dt| dt.abs() <= 1000))
                    .count()
            };
            let median = |v: &[Option<i64>]| {
                quantile(
                    &mut v.iter().copied().map(penalised_error).collect::<Vec<_>>(),
                    0.5,
                )
                .unwrap_or(f64::INFINITY)
            };
            (hits(b) * a.len())
                .cmp(&(hits(a) * b.len()))
                .then_with(|| median(a).total_cmp(&median(b)))
                .then_with(|| pa.abs_diff(default).cmp(&pb.abs_diff(default)))
        })
        .map_or(default, |(p, _)| *p)
}

/// Walk with fact anchoring removed and archived takes enabled, mirroring verify.
fn walk(k: &Kept, period: Option<i64>) -> LineWalk {
    let mut exit = ExitParams {
        take_from_archive: true,
        ..k.exit.clone()
    };
    if let Some(period) = period {
        exit.model.ticker_period_ms = period;
    }
    ExitModel::new(&exit).walk_held(
        &record::unanchored(&k.deal),
        &k.ticks,
        k.fill,
        k.deal.close_ms,
    )
}

/// A time exists only when the model's exit was a stop.
fn stop_time(walk: &LineWalk) -> Option<i64> {
    (walk.exit.kind == ExitKind::Stop).then_some(walk.exit.t_ms)
}

/// Count a false early stop only when it precedes the factual close by over a second.
fn is_early(time: Option<i64>, close: i64) -> bool {
    time.is_some_and(|time| time < close.saturating_sub(1000))
}

/// Match payload without losing route information used by the coverage table.
fn matched_value<'a, T>(matched: &Match<'a, T>) -> Option<&'a T> {
    match matched {
        Match::Task(v) | Match::CoinTime(v) => Some(v),
        _ => None,
    }
}

/// Per-route coverage; ambiguity counts deals separately from candidate lines.
#[derive(Default)]
struct Routes {
    task: usize,
    coin: usize,
    ambiguous: usize,
    candidates: usize,
    none: usize,
}
impl Routes {
    /// Accumulate one attachment result.
    fn add<T>(&mut self, m: &Match<'_, T>) {
        match m {
            Match::Task(_) => self.task += 1,
            Match::CoinTime(_) => self.coin += 1,
            Match::Ambiguous(n) => {
                self.ambiguous += 1;
                self.candidates += n;
            }
            Match::None => self.none += 1,
        }
    }
}

/// Timing distributions retain misses in the denominators of agreement shares.
#[derive(Default)]
struct Timing {
    n: usize,
    dt: Vec<f64>,
    abs: Vec<f64>,
    book: usize,
    second: usize,
    errors: Vec<Option<i64>>,
    triggers: BTreeMap<&'static str, usize>,
}
impl Timing {
    /// Add a matched fire, retaining a non-stop walk as an explicit model miss.
    fn add(&mut self, time: Option<i64>, fire: i64, allowance: i64) {
        self.n += 1;
        self.errors.push(time.map(|time| time - fire));
        if let Some(time) = time {
            let dt = (time - fire) as f64;
            self.dt.push(dt);
            self.abs.push(dt.abs());
            self.book += usize::from(dt.abs() <= allowance as f64);
            self.second += usize::from(dt.abs() <= 1000.0);
        }
    }
    /// Render counts, signed/absolute quantiles and both agreement shares.
    fn render(&self) -> String {
        format!(
            "n {} stopped {}/{}; dt {}; abs {}; book {}; 1s {}; penalised med|dt| {}; trigger mix {:?}",
            self.n,
            self.dt.len(),
            self.n,
            triplet(&self.dt),
            triplet(&self.abs),
            share(self.book, self.n),
            share(self.second, self.n),
            number(quantile(
                &mut self
                    .errors
                    .iter()
                    .copied()
                    .map(penalised_error)
                    .collect::<Vec<_>>(),
                0.5
            )),
            self.triggers
        )
    }
}

/// Population denominators distinguish missing log evidence from factual non-stops.
#[derive(Default)]
struct Population {
    non_stops: usize,
    false_early: usize,
    fact_stops: usize,
    no_log: usize,
    ambiguous: usize,
    lost: usize,
}

/// Per-branch take agreement, with no inferred branch supplied by the model.
#[derive(Default)]
struct TakeScore {
    n: usize,
    agree: usize,
}

/// Reading crossings and signed tape-to-log errors, grouped by actual settings.
#[derive(Default)]
struct Readings {
    counts: [usize; 6],
    proxy: Vec<f64>,
    last: Vec<f64>,
    ema_gap: Vec<f64>,
}
impl Readings {
    /// Compare the independent tape candidates with the logged fire readings.
    fn add(&mut self, k: &Kept, fire: &StopFire) {
        if !fire.trailing {
            if let Some(fixed) = fire.fixed {
                let by = fired_by(
                    k.deal.is_long(),
                    fixed,
                    fire.bid,
                    fire.bid_ema,
                    Some(fire.last),
                    Some(fire.ask),
                );
                let index = match by {
                    FiredBy::OnlyBid => 0,
                    FiredBy::OnlyEma => 1,
                    FiredBy::OnlyLast => 2,
                    FiredBy::OnlyAsk => 3,
                    FiredBy::Several => 4,
                    FiredBy::None => 5,
                };
                self.counts[index] += 1;
                let side = if k.deal.is_long() {
                    Side::Sell
                } else {
                    Side::Buy
                };
                let ticker = k
                    .ticks
                    .iter()
                    .rfind(|t| t.time_ms as i64 <= fire.t_ms && t.side == side)
                    .map(|t| f64::from(t.price));
                let logged = if k.deal.is_long() {
                    fire.bid
                } else {
                    Some(fire.ask)
                };
                if let (Some(tape), Some(logged)) = (ticker, logged) {
                    if let Some(err) = error_pct(tape, logged) {
                        self.proxy.push(err);
                    }
                }
                if let Some(tick) = k.ticks.iter().rfind(|t| t.time_ms as i64 <= fire.t_ms) {
                    if let Some(err) = error_pct(f64::from(tick.price), fire.last) {
                        self.last.push(err);
                    }
                }
            }
        }
        if let (Some(ema), Some(bid)) = (fire.bid_ema, fire.bid) {
            if let Some(gap) = error_pct(ema, bid) {
                self.ema_gap.push(gap);
            }
        }
    }
    /// Render disjoint crossing counts and independent candidate errors.
    fn render(&self) -> String {
        format!(
            "n {} {:?}; proxy {}; last {}",
            self.counts.iter().sum::<usize>(),
            self.counts,
            error_summary(&self.proxy),
            error_summary(&self.last)
        )
    }
}

/// Error relative to a valid logged price; unavailable references stay missing.
fn error_pct(tape: f64, logged: f64) -> Option<f64> {
    (logged.is_finite() && logged > 0.0 && tape.is_finite())
        .then(|| (tape - logged) / logged * 100.0)
}

/// Setting buckets retain the nonstandard EMA configurations rather than discarding them.
fn ema_bucket(n: f64) -> &'static str {
    if n.abs() < 1e-9 {
        "0"
    } else if [3.0, 5.0, 10.0].iter().any(|v| (n - v).abs() < 1e-9) {
        "3|5|10"
    } else {
        "other"
    }
}

/// Model trigger labels include the unexpected fact anchor as an explicit diagnostic.
fn trigger_name(t: Option<StopTrigger>) -> &'static str {
    match t {
        Some(StopTrigger::Ticker) => "Ticker",
        Some(StopTrigger::Series) => "Series",
        Some(StopTrigger::FastTick) => "FastTick",
        Some(StopTrigger::Trailing) => "Trailing",
        Some(StopTrigger::Fact) => "Fact",
        None => "none",
    }
}

/// Stable log branch labels for the take table.
fn branch_name(b: TakeBranch) -> &'static str {
    match b {
        TakeBranch::SellPrice => "SellPrice",
        TakeBranch::AskAdjusted => "AskAdjusted",
        TakeBranch::Adjusted => "Adjusted",
        TakeBranch::Unknown => "Unknown",
    }
}

/// Use the bench's sorted-index convention without rounding intermediate statistics.
pub(super) fn quantile(v: &mut [f64], q: f64) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    v.get(((v.len() as f64 * q) as usize).min(v.len().saturating_sub(1)))
        .copied()
}

/// Quantiles in a fixed order without changing the accumulated samples.
fn triplet(v: &[f64]) -> String {
    format!(
        "{}/{}/{}",
        number(quantile(&mut v.to_vec(), 0.1)),
        number(quantile(&mut v.to_vec(), 0.5)),
        number(quantile(&mut v.to_vec(), 0.9))
    )
}

/// Signed median and absolute p90 have separate meanings and sample counts.
fn error_summary(v: &[f64]) -> String {
    format!(
        "n {} {},{}",
        v.len(),
        number(quantile(&mut v.to_vec(), 0.5)),
        number(quantile(
            &mut v.iter().map(|v| v.abs()).collect::<Vec<_>>(),
            0.9
        ))
    )
}

/// Format missing or non-finite metrics without implying a measured zero.
fn number(v: Option<f64>) -> String {
    v.filter(|v| v.is_finite())
        .map_or_else(|| "NA".into(), |v| format!("{v:.3}"))
}

/// A share always carries its numerator and denominator, including empty groups.
fn share(n: usize, d: usize) -> String {
    format!("{n}/{d}={}", number((d > 0).then(|| n as f64 / d as f64)))
}

/// Keep all diagnostic output ASCII even for user-authored strategy kind labels.
fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii() && !c.is_control() {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// Print every observed row with explicit fit and held columns, including empty halves.
fn print_table(title: &str, rows: impl IntoIterator<Item = ((String, Half), String)>) {
    let mut table: BTreeMap<String, [String; 2]> = BTreeMap::new();
    for ((key, half), value) in rows {
        table
            .entry(key)
            .or_insert_with(|| ["NA".into(), "NA".into()])[usize::from(half == Half::Held)] = value;
    }
    eprintln!("{title}\nrow | Fit | Held");
    for (key, [fit, held]) in table {
        eprintln!("{key} | {fit} | {held}");
    }
}

/// Score every eligible deal of one half; no model stop remains a miss.
fn cadence_score(
    kept: &[Kept],
    matched: &[Option<&StopFire>],
    eligible: &[usize],
    half: Half,
    period: i64,
) -> Timing {
    let mut score = Timing::default();
    for trigger in ["Ticker", "Series", "FastTick", "Trailing", "Fact", "none"] {
        score.triggers.insert(trigger, 0);
    }
    for &i in eligible {
        let k = &kept[i];
        if day_half(k.deal.buy_ms) == half {
            if let Some(fire) = matched[i] {
                let walked = walk(k, Some(period));
                let time = stop_time(&walked);
                *score
                    .triggers
                    .entry(trigger_name(if time.is_some() {
                        walked.stop_trigger
                    } else {
                        None
                    }))
                    .or_default() += 1;
                score.add(time, fire.t_ms, k.exit.model.book_stop_time_ms);
            }
        }
    }
    score
}

/// Paired day-block bootstrap of default minus candidate median absolute timing error.
/// Returns NaN bounds when there are no paired days or no requested resamples.
pub(super) fn bootstrap_diff(days: &[(i64, f64, f64)], resamples: usize, seed: u64) -> (f64, f64) {
    let mut blocks: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
    for &(day, a, b) in days {
        if a.is_finite() && b.is_finite() {
            blocks.entry(day).or_default().push((a, b));
        }
    }
    let blocks: Vec<_> = blocks.into_values().collect();
    if blocks.is_empty() || resamples == 0 {
        return (f64::NAN, f64::NAN);
    }
    let mut state = if seed == 0 { 0x9E3779B97F4A7C15 } else { seed };
    let mut differences = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut a = Vec::new();
        let mut b = Vec::new();
        for _ in 0..blocks.len() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            for &(left, right) in &blocks[(state % blocks.len() as u64) as usize] {
                a.push(left);
                b.push(right);
            }
        }
        if let (Some(a), Some(b)) = (quantile(&mut a, 0.5), quantile(&mut b, 0.5)) {
            differences.push(a - b);
        }
    }
    (
        quantile(&mut differences, 0.05).unwrap_or(f64::NAN),
        quantile(&mut differences, 0.95).unwrap_or(f64::NAN),
    )
}
