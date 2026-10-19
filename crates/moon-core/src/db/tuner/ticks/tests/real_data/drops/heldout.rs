//! Fit-only rule selection, immutable coverage cohorts and anonymous held-out evidence.

use super::fit::{FitPoint, PointError, fit_detect_in_cohort};
use super::logs::{HALF_NAMES, Join, LogDetect, OUTCOME_NAMES, Outcome, Parameters};
use super::*;
use crate::db::tuner::ticks::drops::units::{Current, Units};
use std::collections::BTreeMap;

/// Report every coverage trade-off even though only the frozen envelope feeds the gate.
const ENVELOPES: [i64; 6] = [60, 120, 180, 300, 420, 600];
const FAMILY_NAMES: [&str; 3] = ["R0_universal", "R1_affine", "R2_lookup"];
const REFUSAL_NAMES: [&str; 11] = [
    "core_unknown",
    "venue_ambiguous",
    "strategy_missing",
    "strategy_ambiguous",
    "short_side",
    "params_missing",
    "param_fallback",
    "price_is_low_on",
    "degenerate_params",
    "no_tape",
    "partial_tape",
];

/// Every parsed detect survives as either a qualified row or one explicitly named refusal.
pub(super) struct Row {
    pub(super) detect: LogDetect,
    pub(super) params: Option<DropsParams>,
    ticks: Vec<Tick>,
    pub(super) covered: Coverage,
    refusal: Option<usize>,
    dump_only: bool,
    strategy_id: Option<i64>,
    production_spans: Vec<Coverage>,
    production_covered: Option<Coverage>,
    deal_joined: bool,
    deal_ambiguous: bool,
    buy_ms_fallback: bool,
    pub(super) from_dump: Option<[bool; 9]>,
}

/// UTC dates, including refused rows, define the split before any model is measured.
struct Split {
    fit: Vec<usize>,
    test: Vec<usize>,
    fit_dates: usize,
    test_dates: usize,
}

/// Never distribute two detects from one UTC day across different halves.
fn split_by_date(rows: &[Row]) -> Split {
    let mut dates: Vec<_> = rows
        .iter()
        .map(|r| r.detect.detect_ms.div_euclid(86_400_000))
        .collect();
    dates.sort_unstable();
    dates.dedup();
    let fit_dates = dates.len().div_ceil(2);
    let boundary = dates.get(fit_dates).copied();
    let (fit, test) = (0..rows.len()).partition(|&i| {
        boundary.is_none_or(|b| rows[i].detect.detect_ms.div_euclid(86_400_000) < b)
    });
    Split {
        fit,
        test,
        fit_dates,
        test_dates: dates.len() - fit_dates,
    }
}

/// The universal family can produce the full grid on every fit row, up to 420 seconds.
fn choose_envelope(points: &[FitPoint]) -> i64 {
    let needed = points
        .iter()
        .filter(|p| p.valid())
        .map(|p| p.history_s())
        .max()
        .unwrap_or(60);
    ENVELOPES.into_iter().find(|&s| s >= needed).unwrap_or(600)
}

/// Coverage admission is independent of candidate success, tick count and feasibility.
pub(super) fn refusal(row: &Row, env: i64) -> Option<usize> {
    row.refusal.or_else(|| {
        let span = Coverage::one((
            row.detect.detect_ms - env * 1000,
            row.detect.detect_ms + 2000,
        ));
        match fit::coverage_class(&row.covered, &span) {
            0 => None,
            1 => Some(10),
            _ => Some(9),
        }
    })
}

/// Keep just the sufficient anonymous source tuple for the lookup family.
fn key(p: &DropsParams) -> (u32, u32, u32) {
    (p.max_time_s, p.price_ma_s, p.last_price_ma)
}

/// R1 has fixed global measurement switches and three explicitly enumerated affine choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Affine {
    a2: i32,
    b: i32,
    c2: usize,
    j_mode: usize,
    template: FitPoint,
}

impl Affine {
    /// Positive half-integers round upward; signed window failures remain signed misses.
    fn point(self, p: &DropsParams) -> FitPoint {
        FitPoint {
            w_s: i32::try_from(
                (i64::from(self.a2) * i64::from(p.max_time_s) + 1).div_euclid(2)
                    + i64::from(self.b),
            )
            .unwrap_or(i32::MAX),
            k: (self.c2 as u64 * u64::from(p.price_ma_s) + 1).div_euclid(2) as usize,
            j: match self.j_mode {
                0 => p.last_price_ma as usize,
                1 => (p.last_price_ma as usize).div_ceil(2),
                _ => 0,
            },
            ..self.template
        }
    }
}

/// Grid enumeration order is the final deterministic tie-break after family complexity.
fn point_index(p: FitPoint) -> Option<usize> {
    if !p.valid() {
        return None;
    }
    let current = p.current.index();
    let base = ((p.w_s as usize - 2) * 30 + p.k - 1) * 7 + p.j;
    let offset = if p.step_ms == 1000 {
        usize::from(p.extend) * 4 + current
    } else {
        8 + usize::from(p.extend) * 8 + current * 2 + usize::from(p.wall_slots)
    };
    Some(base * 24 + offset)
}

/// Compact agreement columns avoid retaining millions of floating errors per detect.
pub(super) fn flags(error: PointError) -> u8 {
    u8::from(error.drop05)
        | (u8::from(error.ask005) << 1)
        | (u8::from(error.ask01) << 2)
        | (u8::from(error.fire2s) << 3)
        | (u8::from(error.truncation) << 4)
        | (u8::from(error.logged_fire2s) << 5)
}

/// The gate uses only the rounded-drop, tight-Ask and causal-fire bits.
fn joint(bits: u8) -> bool {
    bits & 11 == 11
}

/// The fitting interface receives only qualified fit rows and their measurements.
struct Scored {
    params: DropsParams,
    passing: Vec<u64>,
}

/// No held-out rows or held-out scores can enter this frozen model bundle.
struct Frozen {
    r0: FitPoint,
    r1: Affine,
    lookup: BTreeMap<(u32, u32, u32), FitPoint>,
    winner: usize,
}

impl Frozen {
    /// Lookup support is frozen on fit rows; every other tuple uses the same fitted R1.
    fn point(&self, family: usize, p: &DropsParams) -> FitPoint {
        match family {
            0 => self.r0,
            1 => self.r1.point(p),
            _ => self
                .lookup
                .get(&key(p))
                .copied()
                .unwrap_or_else(|| self.r1.point(p)),
        }
    }
}

/// Missing candidate readings are misses against a fixed cohort, never new refusals.
fn score_point(row: &Scored, point: FitPoint) -> usize {
    usize::from(point_index(point).is_some_and(|i| {
        row.passing
            .get(i / 64)
            .is_some_and(|bits| bits & (1 << (i % 64)) != 0)
    }))
}

/// First-in-grid ties stay stable, including a zero-hit or empty fit cohort.
fn best_point(rows: &[&Scored], points: &[FitPoint]) -> FitPoint {
    let mut best = (0, points[0]);
    for &point in points {
        let hits = rows.iter().map(|r| score_point(r, point)).sum();
        if hits > best.0 {
            best = (hits, point);
        }
    }
    best.1
}

/// Select all parameters and exactly one family using the fit half only.
fn fit_rules(rows: &[Scored], points: &[FitPoint], forced_family: Option<usize>) -> Frozen {
    let refs: Vec<_> = rows.iter().collect();
    let r0 = best_point(&refs, points);
    let mut r1 = Affine {
        a2: 1,
        b: -10,
        c2: 1,
        j_mode: 0,
        template: points[0],
    };
    let mut r1_hits = 0;
    // Global switches are enumerated once, rather than once per window/high/current dimension.
    for &template in points.iter().filter(|p| p.w_s == 2 && p.k == 1 && p.j == 0) {
        for a2 in [1, 2, 3, 4] {
            for b in -10..=20 {
                for c2 in [1, 2, 4] {
                    for j_mode in 0..3 {
                        let rule = Affine {
                            a2,
                            b,
                            c2,
                            j_mode,
                            template,
                        };
                        let hits = rows
                            .iter()
                            .map(|r| score_point(r, rule.point(&r.params)))
                            .sum();
                        if hits > r1_hits {
                            r1 = rule;
                            r1_hits = hits;
                        }
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<_, Vec<&Scored>> = BTreeMap::new();
    for row in rows {
        groups.entry(key(&row.params)).or_default().push(row);
    }
    let lookup = groups
        .into_iter()
        .filter(|(_, group)| group.len() >= 5)
        .map(|(key, group)| (key, best_point(&group, points)))
        .collect();
    let mut frozen = Frozen {
        r0,
        r1,
        lookup,
        winner: 0,
    };
    let mut best = rows.iter().map(|r| score_point(r, r0)).sum::<usize>();
    for family in 1..3 {
        let hits = rows
            .iter()
            .map(|r| score_point(r, frozen.point(family, &r.params)))
            .sum();
        if hits > best {
            best = hits;
            frozen.winner = family;
        }
    }
    if let Some(family) = forced_family {
        frozen.winner = family;
    }
    frozen
}

/// Time both real family searches on synthetic measurements without loading logs or tapes.
pub(super) fn synthetic_rule_search(
    rows: Vec<(DropsParams, Vec<u64>)>,
    points: &[FitPoint],
) -> [usize; 2] {
    let rows: Vec<_> = rows
        .into_iter()
        .map(|(params, passing)| Scored { params, passing })
        .collect();
    let mid = rows.len() / 2;
    let forward = fit_rules(&rows[..mid], points, None);
    let reverse = fit_rules(&rows[mid..], points, Some(forward.winner));
    [forward.winner, reverse.winner]
}

/// All table denominators are explicit and retain candidate misses.
#[derive(Clone, Default)]
struct Counts {
    parsed: usize,
    scorable: usize,
    drop05: usize,
    ask005: usize,
    ask01: usize,
    fire: usize,
    joint: usize,
    joint01: usize,
    trunc_joint: usize,
    logged_joint: usize,
    infeasible: usize,
    prodwin: usize,
    prodwin_joint: usize,
}

impl Counts {
    /// Diagnostic feasibility never changes the causal joint result.
    fn add(&mut self, bits: u8, infeasible: bool, prodwin: bool) {
        self.scorable += 1;
        self.drop05 += usize::from(bits & 1 != 0);
        self.ask005 += usize::from(bits & 2 != 0);
        self.ask01 += usize::from(bits & 4 != 0);
        self.fire += usize::from(bits & 8 != 0);
        self.joint += usize::from(joint(bits));
        self.joint01 += usize::from(bits & 13 == 13);
        self.trunc_joint += usize::from(bits & 26 == 26);
        self.logged_joint += usize::from(bits & 35 == 35);
        self.infeasible += usize::from(infeasible);
        self.prodwin += usize::from(prodwin);
        self.prodwin_joint += usize::from(prodwin && joint(bits));
    }
}

/// Missing denominators yield zero agreement and therefore cannot pass a gate.
fn percent(hits: usize, n: usize) -> f64 {
    if n == 0 {
        0.0
    } else {
        hits as f64 * 100.0 / n as f64
    }
}

/// Two-sided Wilson 95 percent lower bound is evidence, not an extra gate threshold.
fn wilson(hits: usize, n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let n = n as f64;
    let p = hits as f64 / n;
    let z2 = 1.96_f64.powi(2);
    100.0 * (p + z2 / (2.0 * n) - 1.96 * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt())
        / (1.0 + z2 / n)
}

/// Enforce the predeclared forward, reverse, production-window and R1-support floors.
fn gate(family: usize, forward: &Counts, reverse: &Counts, r1: &Counts) -> &'static str {
    if forward.scorable < 150 {
        return "FAIL (insufficient held-out cohort)";
    }
    if percent(forward.joint, forward.scorable) < 80.0 {
        return "FAIL (forward joint below 80 percent)";
    }
    if reverse.scorable == 0 || percent(reverse.joint, reverse.scorable) < 75.0 {
        return "FAIL (reverse joint below 75 percent)";
    }
    if forward.prodwin == 0 || percent(forward.prodwin_joint, forward.prodwin) < 80.0 {
        return "FAIL (prodwin joint below 80 percent)";
    }
    if family == 2 && (r1.scorable == 0 || percent(r1.joint, r1.scorable) < 80.0) {
        return "FAIL (R2 lacks held-out R1 support)";
    }
    "PROVISIONAL-PASS (production parity not run)"
}

/// Only fixed labels and numeric aggregates enter stdout.
fn table_line(name: &str, half: &str, c: &Counts) -> String {
    format!(
        "HO RULE name={name} half={half} parsed={} scorable={} drop05={} ask005={} ask01={} fire2s={} joint={} joint_pct={:.3} wilson95lo={:.3} strict_pct={:.3} joint01={} trunc_joint={} logged_joint_diag={} infeasible_diag={} prodwin={} prodwin_joint={} prodwin_pct={:.3}",
        c.parsed,
        c.scorable,
        c.drop05,
        c.ask005,
        c.ask01,
        c.fire,
        c.joint,
        percent(c.joint, c.scorable),
        wilson(c.joint, c.scorable),
        percent(c.joint, c.parsed),
        c.joint01,
        c.trunc_joint,
        c.logged_joint,
        c.infeasible,
        c.prodwin,
        c.prodwin_joint,
        percent(c.prodwin_joint, c.prodwin)
    )
}

/// Stable outcome indices are shared by funnel, coverage and agreement tables.
fn outcome(d: &LogDetect) -> usize {
    match d.outcome {
        Outcome::TaskStarted => 0,
        Outcome::AutoStartFalse(_) => 1,
        Outcome::FilterCheckRefused => 2,
        Outcome::NoFollowUp => 3,
        Outcome::Ambiguous => 4,
    }
}

/// Prepare a row without consulting any candidate score or another detect's outcome.
pub(super) fn load_row(
    mut detect: LogDetect,
    conn: &Connection,
    labels: &HashMap<String, u64>,
    venues: &HashMap<u64, Option<String>>,
    pairs: &[(String, String)],
    spans: Option<&Coverage>,
) -> rusqlite::Result<Row> {
    detect.core_uid = labels.get(&detect.core_label).copied();
    let mut row = Row {
        detect,
        params: None,
        ticks: Vec::new(),
        covered: Coverage::none(),
        refusal: None,
        dump_only: false,
        strategy_id: None,
        production_spans: Vec::new(),
        production_covered: None,
        deal_joined: false,
        deal_ambiguous: false,
        buy_ms_fallback: false,
        from_dump: None,
    };
    let Some(uid) = row.detect.core_uid else {
        row.refusal = Some(0);
        return Ok(row);
    };
    let Some(Some(venue)) = venues.get(&uid) else {
        row.refusal = Some(1);
        return Ok(row);
    };
    let span = Coverage::one((row.detect.detect_ms - 600_000, row.detect.detect_ms + 2000));
    if let Some((ticks, covered)) =
        logs::tape_for_coin(venue, &row.detect.coin, pairs, spans.unwrap_or(&span))
    {
        row.ticks = ticks;
        row.covered = covered;
    }
    let id = match logs::join_strategy(conn, uid, &row.detect.strategy)? {
        Join::Exact(id) | Join::PrefixStripped(id) => id,
        Join::Missing => {
            row.refusal = Some(2);
            return Ok(row);
        }
        Join::Ambiguous => {
            row.refusal = Some(3);
            return Ok(row);
        }
        Join::ParamFallback => {
            row.refusal = Some(6);
            return Ok(row);
        }
    };
    row.strategy_id = Some(id);
    match logs::parameters(conn, id, uid, row.detect.detect_ms) {
        Some(Parameters::Long { params, from_dump }) => {
            row.from_dump = Some(from_dump);
            row.dump_only = from_dump.into_iter().all(|v| v);
            if params.max_time_s < 2
                || params.price_ma_s > params.max_time_s
                || !params.price_delta_pct.is_finite()
                || params.price_delta_pct <= 0.0
                || params.last_price_ma > 6
            {
                row.refusal = Some(8);
            }
            row.params = Some(params);
        }
        Some(Parameters::ShortSide) => row.refusal = Some(4),
        Some(Parameters::ParamFallback) => row.refusal = Some(6),
        Some(Parameters::PriceIsLowOn) => row.refusal = Some(7),
        None => row.refusal = Some(5),
    }
    Ok(row)
}

/// Prior parsed detects constrain only the explicitly labelled reset diagnostic.
fn previous<'a>(rows: &'a [Row], row: &Row) -> Option<&'a LogDetect> {
    rows.iter().rev().map(|r| &r.detect).find(|d| {
        d.detect_ms < row.detect.detect_ms
            && d.core_label == row.detect.core_label
            && d.strategy == row.detect.strategy
            && logs::coin_key(&d.coin) == logs::coin_key(&row.detect.coin)
    })
}

/// Select the nearest unused factual order; tied nearest stamps remain explicitly ambiguous.
fn join_deal(
    row: &Row,
    deals: &[Deal],
    consumed: &mut std::collections::HashSet<usize>,
) -> Result<Option<usize>, ()> {
    if !matches!(row.detect.outcome, Outcome::TaskStarted) {
        return Ok(None);
    }
    let mut candidates: Vec<_> = deals
        .iter()
        .enumerate()
        .filter(|(i, deal)| {
            let stamp = deal.buy_set_ms.unwrap_or(deal.buy_ms);
            !consumed.contains(i)
                && deal.kind == KIND_DROPS
                && Some(deal.core_uid) == row.detect.core_uid
                && Some(deal.strategy_id) == row.strategy_id
                && logs::coin_key(&deal.coin) == logs::coin_key(&row.detect.coin)
                && (row.detect.detect_ms..=row.detect.detect_ms + 5000).contains(&stamp)
        })
        .map(|(i, d)| (d.buy_set_ms.unwrap_or(d.buy_ms) - row.detect.detect_ms, i))
        .collect();
    candidates.sort_unstable();
    if candidates.len() > 1 && candidates[0].0 == candidates[1].0 {
        return Err(());
    }
    Ok(candidates.first().map(|&(_, i)| {
        consumed.insert(i);
        i
    }))
}

/// Require the same held admission as C2 before retaining a joined deal's UI window.
fn production_spans(row: &Row, deals: &[Deal], margin: i64, tail_ms: i64) -> Vec<Coverage> {
    if !matches!(row.detect.outcome, Outcome::TaskStarted) {
        return Vec::new();
    }
    deals
        .iter()
        .filter(|deal| {
            let stamp = deal.buy_set_ms.unwrap_or(deal.buy_ms);
            deal.kind == KIND_DROPS
                && Some(deal.core_uid) == row.detect.core_uid
                && Some(deal.strategy_id) == row.strategy_id
                && logs::coin_key(&deal.coin) == logs::coin_key(&row.detect.coin)
                && (row.detect.detect_ms..=row.detect.detect_ms + 5000).contains(&stamp)
        })
        .filter_map(|deal| {
            let w = model_window(deal, margin, long_position_ms())?;
            row.production_covered
                .as_ref()
                .unwrap_or(&row.covered)
                .covers(&required_spans(&w))
                .then(|| tail::focus_with_tail(&w, tail_ms))
        })
        .collect()
}

/// Membership uses the fixed cohort envelope even when a frozen rule is invalid on this row.
fn in_production_window(row: &Row, env: i64) -> bool {
    matches!(row.detect.outcome, Outcome::TaskStarted)
        && row
            .production_spans
            .iter()
            .any(|spans| fit::coverage_qualified(spans, row.detect.detect_ms, env))
}

/// Refused rows with known parameters still count in their parsed source bucket.
fn bucket(params: Option<&DropsParams>) -> usize {
    match params.map(|p| p.max_time_s) {
        Some(0..=10) => 0,
        Some(11..=30) => 1,
        Some(31..=59) => 2,
        Some(60..=90) => 3,
        _ => 4,
    }
}

/// Cache only fit-half measurements; reverse fitting is invoked after the forward freeze.
fn measure_fit(rows: &[Row], indices: &[usize], points: &[FitPoint], env: i64) -> Vec<Scored> {
    indices
        .iter()
        .filter_map(|&i| {
            let row = &rows[i];
            if refusal(row, env).is_some() {
                return None;
            }
            let p = row.params.clone()?;
            let measured = fit_detect_in_cohort(
                &row.detect,
                &p,
                &row.ticks,
                &row.covered,
                previous(rows, row),
                points,
                env,
            );
            Some(Scored {
                params: p,
                passing: measured.passing,
            })
        })
        .collect()
}

/// Baselines use their original readers, independent of the fitted candidate engine.
fn baseline(row: &Row, c2: bool, env: i64) -> u8 {
    let Some(p) = row.params.as_ref() else {
        return 0;
    };
    // C2: probe::BEST includes the current slot; the named units hypothesis uses 1s high endpoints.
    let units = Units {
        high: 2,
        extend: true,
        ..Default::default()
    };
    let history = i64::from(p.max_time_s) + if c2 { i64::from(p.price_ma_s) } else { 0 };
    if history.max(i64::from(p.last_price_ma) * 2) + 60 > env {
        return 0;
    }
    let read = |at| {
        if c2 {
            units.reading(p, &row.ticks, at)
        } else {
            p.drop_at_variant(&row.ticks, at, DropsVariant::default())
        }
    };
    let Some(r) = read(row.detect.detect_ms) else {
        return 0;
    };
    let drop_error = (r.drop_pct - row.detect.x_price_delta).abs();
    let ask_error = row
        .detect
        .ask
        .filter(|a| *a > 0.0)
        .map_or(f64::INFINITY, |a| (r.last_price / a - 1.0).abs() * 100.0);
    let fire = (row.detect.detect_ms - 60_000..=row.detect.detect_ms + 2000)
        .step_by(1000)
        .find(|&t| read(t).is_some_and(|r| r.drop_pct >= p.price_delta_pct));
    flags(PointError {
        drop_error,
        ask_error_pct: ask_error,
        drop05: drop_error <= 0.05 + 1e-9,
        truncation: r.drop_pct >= row.detect.x_price_delta
            && r.drop_pct < row.detect.x_price_delta + 0.1,
        ask005: ask_error <= 0.05 + 1e-9,
        ask01: ask_error <= 0.1 + 1e-9,
        fire2s: fire.is_some_and(|t| (t - row.detect.detect_ms).abs() <= 2000),
        logged_fire2s: false,
    })
}

/// Score the already frozen families once and retain outcome/bucket diagnostics separately.
fn evaluate(
    rows: &[Row],
    indices: &[usize],
    model: &Frozen,
    env: i64,
    half: &str,
    print: bool,
) -> [Counts; 5] {
    let mut totals: [Counts; 5] = std::array::from_fn(|_| Counts {
        parsed: indices.len(),
        ..Default::default()
    });
    let mut by_outcome: [Counts; 5] = Default::default();
    let mut by_bucket: [Counts; 5] = Default::default();
    let mut prod_funnel = [0; 5];
    let mut dump = Counts::default();
    let mut refused = [0; 11];
    for &i in indices {
        let row = &rows[i];
        by_outcome[outcome(&row.detect)].parsed += 1;
        let bucket = bucket(row.params.as_ref());
        by_bucket[bucket].parsed += 1;
        prod_funnel[0] += usize::from(matches!(row.detect.outcome, Outcome::TaskStarted));
        prod_funnel[1] += usize::from(row.deal_joined);
        let frozen_prodwin = in_production_window(row, env);
        prod_funnel[3] += usize::from(row.deal_ambiguous);
        prod_funnel[4] += usize::from(row.buy_ms_fallback);
        prod_funnel[2] += usize::from(frozen_prodwin);
        if let Some(reason) = refusal(row, env) {
            refused[reason] += 1;
            continue;
        }
        let Some(p) = row.params.as_ref() else {
            continue;
        };
        let points = [model.point(0, p), model.point(1, p), model.point(2, p)];
        let fit = fit_detect_in_cohort(
            &row.detect,
            p,
            &row.ticks,
            &row.covered,
            previous(rows, row),
            &points,
            env,
        );
        let values = [
            baseline(row, false, env),
            baseline(row, true, env),
            flags(fit.errors[0]),
            flags(fit.errors[1]),
            flags(fit.errors[2]),
        ];
        // The gate's subset is fixed by coverage, shared across diagnostic columns.
        let prodwin = frozen_prodwin;
        for (total, &bits) in totals.iter_mut().zip(&values) {
            total.add(bits, fit.infeasible, prodwin);
        }
        let bits = values[model.winner + 2];
        by_outcome[outcome(&row.detect)].add(bits, fit.infeasible, prodwin);
        by_bucket[bucket].add(bits, fit.infeasible, prodwin);
        if row.dump_only {
            dump.parsed += 1;
            dump.add(bits, fit.infeasible, prodwin);
        }
    }
    if print {
        eprintln!(
            "HO PRODWIN half={half} task_started={} deal_joined={} window_ok={} ambiguous={} buy_ms_fallback={}",
            prod_funnel[0], prod_funnel[1], prod_funnel[2], prod_funnel[3], prod_funnel[4]
        );
        let funnel = REFUSAL_NAMES
            .iter()
            .zip(refused)
            .map(|(name, n)| format!(" {name}={n}"))
            .collect::<String>();
        eprintln!(
            "HO FUNNEL half={half} parsed={}{} scorable={} infeasible_diag={}",
            indices.len(),
            funnel,
            totals[0].scorable,
            totals[0].infeasible
        );
        for (name, c) in [
            "prod_default",
            "units_w0_h2_l0_e1",
            "R0_universal",
            "R1_affine",
            "R2_lookup",
        ]
        .into_iter()
        .zip(&totals)
        {
            eprintln!("{}", table_line(name, half, c));
        }
        for (name, c) in OUTCOME_NAMES.into_iter().zip(by_outcome) {
            eprintln!(
                "HO BYOUTCOME outcome={name} {}",
                table_line(FAMILY_NAMES[model.winner], half, &c)
            );
        }
        for (name, c) in [
            "maxtime_le10",
            "maxtime_11_30",
            "maxtime_31_59",
            "maxtime_60_90",
            "other",
        ]
        .into_iter()
        .zip(by_bucket)
        {
            eprintln!(
                "HO BYBUCKET bucket={name} {}",
                table_line(FAMILY_NAMES[model.winner], half, &c)
            );
        }
        eprintln!(
            "HO DUMP_ONLY {}",
            table_line(FAMILY_NAMES[model.winner], half, &dump)
        );
    }
    totals
}

/// Print numeric rule parameters; lookup keys are source knobs, never strategy identifiers.
fn print_params(model: &Frozen, half: &str, env: i64) {
    eprintln!(
        "HO FREEZE half={half} winner={} envelope_s={env} rounding_half_up=1",
        model.winner
    );
    let a = model.r1;
    eprintln!(
        "HO RULEPARAMS family=1 a={:.1} b={} c={:.1} j_mode={}",
        f64::from(a.a2) / 2.0,
        a.b,
        a.c2 as f64 / 2.0,
        a.j_mode
    );
    for (family, point) in [(0, model.r0), (1, a.template)] {
        eprintln!(
            "HO RULEPARAMS family={family} w={} k={} j={} step={} extend={} current={} wall={}",
            point.w_s,
            point.k,
            point.j,
            point.step_ms,
            usize::from(point.extend),
            point.current.index(),
            usize::from(point.wall_slots)
        );
    }
    for ((w, k, j), p) in &model.lookup {
        eprintln!(
            "HO RULEPARAMS family=2 source_w={w} source_k={k} source_j={j} w={} k={} j={} step={} extend={} current={} wall={}",
            p.w_s,
            p.k,
            p.j,
            p.step_ms,
            usize::from(p.extend),
            p.current.index(),
            usize::from(p.wall_slots)
        );
    }
}

/// Run only under the owner's ignored-bench command on an isolated root and exclusive lease.
pub(super) fn bench() {
    if let Err(reason) = run() {
        eprintln!("GATE: FAIL ({reason})");
    }
    eprintln!("HO DONE");
}

/// IO failures fail closed; the parent removes the snapshot after this reader process exits.
fn run() -> Result<(), &'static str> {
    let _permit = crate::db::report_recovery::prepare().ok_or("lease unavailable")?;
    let held = crate::market::trade_replay::trade_cache::handle()
        .and_then(|c| c.inventory())
        .and_then(Result::ok)
        .ok_or("inventory unavailable")?;
    let (records, stats, app, failed) = logs::snapshot_logs().map_err(|_| "logs unavailable")?;
    eprintln!(
        "HO PARSE parsed={} malformed={} other_lines={} unpaired={} files_unreadable={failed}",
        stats.detects, stats.unparsed, stats.other_lines, stats.signals_unpaired
    );
    if failed > 0 {
        return Err("unreadable log files");
    }
    let conn = Connection::open_with_flags(
        paths::strategies_db_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "strategies unavailable")?;
    let labels = logs::core_labels(&app);
    let venues = logs::core_venues_unique(&app);
    let mut rows = records
        .into_iter()
        .map(|d| load_row(d, &conn, &labels, &venues, &held.keys, None))
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "strategy query failed")?;
    let scope = Query {
        from: -1,
        to: crate::db::analytics::ANALYTICS_HORIZON_SECS,
        metric: crate::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let deals = read_deals(&scope).map_err(|_| "deals unavailable")?;
    let layout = crate::config::layout::WindowLayout::load(crate::config::ProfileAge::Established);
    let min_tail_s = layout
        .analytics_ticks
        .as_ref()
        .and_then(|t| t.min_tail_s)
        .unwrap_or(tail::DEFAULT_MIN_TAIL_S);
    let margin = crate::market::trade_replay::margin_ms();
    let tail_ms = tail::record_tail_ms(min_tail_s, margin);
    let mut consumed = std::collections::HashSet::new();
    for row in &mut rows {
        match join_deal(row, &deals.deals, &mut consumed) {
            Ok(Some(i)) => {
                let deal = &deals.deals[i];
                row.deal_joined = true;
                row.buy_ms_fallback = deal.buy_set_ms.is_none();
                if let (Some(w), Some(Some(venue))) = (
                    model_window(deal, margin, long_position_ms()),
                    row.detect.core_uid.and_then(|uid| venues.get(&uid)),
                ) {
                    // Admission coverage never widens the frozen cohort or merges market tapes.
                    row.production_covered = Some(
                        tape_for(deal, venue, &held.keys, &required_spans(&w))
                            .map_or_else(Coverage::none, |(_, covered)| covered),
                    );
                }
                row.production_spans =
                    production_spans(row, std::slice::from_ref(deal), margin, tail_ms);
            }
            Err(()) => row.deal_ambiguous = true,
            Ok(None) => {}
        }
    }
    let split = split_by_date(&rows);
    let (kline_pairs, klines) = logs::kline_sources();
    let mut kline = [0; 4];
    for row in &rows {
        if let (Some(p), Some(Some(venue))) = (
            row.params.as_ref(),
            row.detect.core_uid.and_then(|uid| venues.get(&uid)),
        ) {
            logs::kline_sanity(
                klines.as_ref(),
                kline_pairs.as_deref().unwrap_or_default(),
                venue,
                &row.detect,
                p,
                &mut kline,
            );
        }
    }
    eprintln!(
        "HO KLINE ask_in_minute={} ask_checked={} implied_high_le_minute_high={} high_checked={} cache_missing={}",
        kline[0],
        kline[1],
        kline[2],
        kline[3],
        usize::from(klines.is_none())
    );
    eprintln!(
        "HO SPLIT fit_dates={} test_dates={} fit_detects={} test_detects={}",
        split.fit_dates,
        split.test_dates,
        split.fit.len(),
        split.test.len()
    );
    let points: Vec<_> = FitPoint::grid().collect();
    let env = choose_envelope(&points);
    for (half, indices) in [(&HALF_NAMES[0], &split.fit), (&HALF_NAMES[1], &split.test)] {
        for envelope in ENVELOPES {
            let full = indices
                .iter()
                .filter(|&&i| {
                    rows[i].covered.contains((
                        rows[i].detect.detect_ms - envelope * 1000,
                        rows[i].detect.detect_ms + 2000,
                    ))
                })
                .count();
            eprintln!("HO COVER half={half} env={envelope} full={full}");
            for (outcome_id, name) in OUTCOME_NAMES.into_iter().enumerate() {
                let mut counts = [0; 3];
                for &i in indices {
                    let row = &rows[i];
                    if outcome(&row.detect) != outcome_id {
                        continue;
                    }
                    let span = Coverage::one((
                        row.detect.detect_ms - envelope * 1000,
                        row.detect.detect_ms + 2000,
                    ));
                    let class = fit::coverage_class(&row.covered, &span);
                    counts[class] += 1;
                }
                eprintln!(
                    "HO COVER_OUTCOME half={half} env={envelope} outcome={name} full={} partial={} none={}",
                    counts[0], counts[1], counts[2]
                );
            }
        }
    }
    let fit = measure_fit(&rows, &split.fit, &points, env);
    let frozen = fit_rules(&fit, &points, None);
    drop(fit);
    print_params(&frozen, "fit", env);
    evaluate(&rows, &split.fit, &frozen, env, "fit", true);
    let test = evaluate(&rows, &split.test, &frozen, env, "test", true);
    let reverse_env = env;
    let reverse_fit = measure_fit(&rows, &split.test, &points, reverse_env);
    let reverse = fit_rules(&reverse_fit, &points, Some(frozen.winner));
    drop(reverse_fit);
    for &i in &split.test {
        rows[i].ticks = Vec::new();
    }
    print_params(&reverse, "reverse_fit", reverse_env);
    let reverse_test = evaluate(
        &rows,
        &split.fit,
        &reverse,
        reverse_env,
        "reverse_test",
        false,
    );
    for &i in &split.fit {
        rows[i].ticks = Vec::new();
    }
    eprintln!(
        "HO REVERSE {}",
        table_line(
            FAMILY_NAMES[frozen.winner],
            "reverse_test",
            &reverse_test[frozen.winner + 2]
        )
    );
    eprintln!(
        "GATE: {}",
        gate(
            frozen.winner,
            &test[frozen.winner + 2],
            &reverse_test[frozen.winner + 2],
            &test[3]
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests;
