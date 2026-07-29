//! Tests for the field-set composition.
//!
//! The acceptance rules are pure functions over fold-score vectors, so they are tested directly:
//! no sample, no search, no database, and an oracle that is a hand-written vector rather than
//! anything the code under test produced.
//!
//! The fold-isolation test is the one that needs a real search behind it, because what it pins is
//! a property of how a fold is BUILT rather than of how a set is chosen.

use std::sync::Arc;

use super::*;
use crate::db::tuner::threshold_search::search::Columns;
use crate::db::tuner::{FieldClass, FIELDS};

/// Deterministic value generator, independent of the search's own PRNG so a fixture does not
/// shift when that stream changes.
struct Lcg(u64);

impl Lcg {
    /// Advance and return the next 64-bit state.
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    /// Uniform value in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Per field, whether it occupies a Delta2/Delta3 slot.
fn slot_flags() -> Vec<bool> {
    FIELDS
        .iter()
        .map(|s| s.class == FieldClass::DeltaSlot)
        .collect()
}

/// A sample of `n` trades whose profit genuinely depends on the first two columns, so a search
/// over it has something to find rather than only noise to fit.
fn sample(n: usize, seed: u64) -> (Vec<f64>, Vec<Vec<f64>>, Vec<i64>) {
    let mut rng = Lcg(seed | 1);
    let nf = FIELDS.len();
    let mut profits = Vec::with_capacity(n);
    let mut vals = vec![Vec::with_capacity(n); nf];
    let mut closes = Vec::with_capacity(n);
    for t in 0..n {
        let a = rng.unit() * 100.0;
        let b = rng.unit() * 100.0;
        // Trades in the upper band of the first column pay; everything else bleeds.
        let edge = if a > 60.0 { 4.0 } else { -1.5 };
        profits.push(edge + (rng.unit() - 0.5) * 3.0);
        for (fi, col) in vals.iter_mut().enumerate() {
            col.push(match fi {
                0 => a,
                1 => b,
                _ => rng.unit() * 50.0,
            });
        }
        // Distinct, increasing timestamps so every cut has a boundary to snap to.
        closes.push(1_700_000_000 + t as i64);
    }
    (profits, vals, closes)
}

// ─────────────────────────────── the acceptance rules ───────────────────────────────

/// A candidate must earn its place in MOST folds, not just on average.
///
/// The oracle is a hand-written pair of vectors: one fold improves enormously while two get
/// worse, so the mean rises and the majority does not. Nothing here is derived from the code
/// under test.
///
/// Breakage this pins: `compose.rs:accepts` dropping its majority clause and deciding on the
/// mean alone. The forward pass would then admit any field that caught one lucky stretch, which
/// is precisely the coincidence out-of-sample scoring exists to reject — and the composed set
/// would grow with fields that help in no other period.
#[test]
fn a_field_only_joins_when_it_helps_in_most_folds() {
    let incumbent = [100.0, 100.0, 100.0];
    // Mean rises from 100 to 140, yet two folds out of three got worse.
    let lucky_one_fold = [420.0, 99.0, 99.0];
    assert!(
        mean(&lucky_one_fold) > mean(&incumbent),
        "the fixture must be a candidate whose MEAN improves, or it tests nothing"
    );
    assert!(
        !accepts(&incumbent, &lucky_one_fold),
        "a candidate winning one fold and losing two must be refused"
    );
    // The same total improvement spread across the folds is a real pattern, and is taken.
    assert!(
        accepts(&incumbent, &[110.0, 108.0, 112.0]),
        "a candidate that helps in every fold must be accepted"
    );
    // Better in a majority but worse on average is still not worth taking.
    assert!(
        !accepts(&incumbent, &[101.0, 101.0, 40.0]),
        "a candidate that wrecks one fold must not ride in on a bare majority"
    );
}

/// A field that costs nothing to remove is removed — where "nothing" means less than the
/// precision the fold sums carry.
///
/// The discriminating case is the boundary PAIR around that tolerance, since a cost of exactly
/// zero is decided the same way with or without it: at this fixture's scale the floor is 7e-8, so
/// a cost of 1e-8 must still be free while 1e-6 must not. Both numbers are derived from
/// `improvement_margin`'s stated definition, not read back from `drops`.
///
/// Breakage this pins: `compose.rs:drops` losing its `noise_floor` term and comparing the means
/// directly. A field whose removal costs a float rounding error would then be kept forever — and
/// since every backward pass re-scores, the set would stop shrinking at whatever it first reached
/// and every strategy the tuner writes would carry filters that only narrow it.
///
/// (The `>=` in `drops` is deliberate but NOT what this pins: the two forms differ only when the
/// two sides are bit-for-bit equal, which no fold sum reaches.)
#[test]
fn a_field_that_costs_nothing_is_given_back() {
    let incumbent = [50.0, 60.0, 70.0];
    assert!(
        drops(&incumbent, &incumbent),
        "removing a field that changes nothing must be free, so the smaller set wins"
    );
    assert!(
        drops(&incumbent, &[51.0, 61.0, 71.0]),
        "removing a field that IMPROVES the score must obviously be taken"
    );
    assert!(
        drops(&incumbent, &[50.0, 60.0, 70.0 - 1e-8]),
        "a cost below the precision the sums carry is not a cost"
    );
    assert!(
        !drops(&incumbent, &[50.0, 60.0, 70.0 - 1e-6]),
        "a cost above that floor is real and the field must be kept"
    );
    assert!(
        !drops(&incumbent, &[50.0, 60.0, 40.0]),
        "a field the score actually depends on must be kept"
    );
}

/// Composition exists exactly on machines at or above the bar, and never asks for a budget of
/// nothing when it does.
///
/// The oracle is a boundary PAIR either side of the documented core threshold, taken from the
/// bar's own constant rather than a number restated here: one core below it there must be no
/// budget at all, and at it there must be a usable one.
///
/// Breakage this pins two ways. `compose.rs:budget_for` returning a budget below the bar — the
/// feature would run on exactly the machines it was gated off, since a caller decides whether to
/// compose by whether a budget came back. And `ranking_restarts` dividing without the
/// `RESTARTS_MIN` floor: with a low restart setting the ranking runs would request zero restarts,
/// find nothing, and the feature would report "this period has no answer" rather than a set.
#[test]
fn composition_has_a_budget_exactly_above_the_core_bar() {
    let bar = super::super::search::HEAVY_SEARCH_MIN_CORES;
    for cores in 0..bar {
        assert!(
            budget_for(cores).is_none(),
            "cores {cores}: below the bar composition must not be offered a budget at all"
        );
    }
    for cores in [bar, bar + 1, 16, 64] {
        let b = budget_for(cores)
            .unwrap_or_else(|| panic!("cores {cores}: at or above the bar there is a budget"));
        assert!(
            b.folds >= 2,
            "cores {cores}: {} folds is not a split",
            b.folds
        );
        for restarts in [1usize, 2, 7, 100, 20_000] {
            assert!(
                b.ranking_restarts(restarts) >= super::super::RESTARTS_MIN,
                "cores {cores}, restarts {restarts}: ranking budget fell to {}",
                b.ranking_restarts(restarts)
            );
        }
    }
    // Width is set by what makes the answer trustworthy, not by the hardware, so a bigger machine
    // buys speed rather than a wider search.
    let at_bar = budget_for(bar).expect("a budget at the bar");
    let far_above = budget_for(64).expect("a budget well above the bar");
    assert_eq!(
        (
            at_bar.folds,
            at_bar.max_fields,
            at_bar.ranking_restarts(800)
        ),
        (
            far_above.folds,
            far_above.max_fields,
            far_above.ranking_restarts(800)
        ),
        "past the bar the budget must stop growing"
    );
}

// ─────────────────────────────── real-snapshot diagnostic ───────────────────────────────

/// One tuner scope out of a real snapshot: the trades of ONE strategy on ONE core, in
/// chronological order.
///
/// Scoped, never pooled, because that is the only way the tuner is ever used: thresholds are
/// tuned for a strategy on the servers it runs on. A sample stirred together from every strategy
/// and every core is not a harder version of the same problem, it is a different one — the
/// strategies react to different fields and trade different situations, so no field can relate to
/// profit across the pool and any search over it is measuring the mixture rather than the tuning.
struct Scope {
    label: String,
    profits: Vec<f64>,
    vals: Vec<Vec<f64>>,
    closes: Vec<i64>,
}

/// The largest strategy+core scopes in the snapshot, biggest first.
///
/// Args:
///     want: How many scopes to return.
///     min_trades: Smallest scope worth reporting on.
///
/// Returns:
///     One [`Scope`] per strategy+core pair, or an empty vector when no snapshot was named.
fn snapshot_scopes(want: usize, min_trades: usize) -> Vec<Scope> {
    let Ok(path) = std::env::var("MOON_TUNER_BENCH_DB") else {
        return Vec::new();
    };
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let mut pick = conn
        .prepare(
            "SELECT strategyid, core_uid, COUNT(*) n FROM orders_rep WHERE closedate > 0 \
             GROUP BY strategyid, core_uid HAVING n >= ?1 ORDER BY n DESC LIMIT ?2",
        )
        .expect("the snapshot must carry orders_rep");
    let picked: Vec<(i64, i64, usize)> = pick
        .query_map(rusqlite::params![min_trades as i64, want as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as usize))
        })
        .expect("scope query")
        .filter_map(Result::ok)
        .collect();

    let nf = FIELDS.len();
    let cols = FIELDS
        .iter()
        .map(|s| format!("\"{}\"", s.col))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {cols}, COALESCE(profitbtc,0), COALESCE(closedate,0) FROM orders_rep \
         WHERE closedate > 0 AND strategyid = ?1 AND core_uid = ?2"
    );
    let mut stmt = conn.prepare(&sql).expect("scope rows");
    picked
        .into_iter()
        .map(|(sid, core, n)| {
            let mut rows = stmt
                .query(rusqlite::params![sid, core])
                .expect("scope rows");
            let (mut profits, mut closes) = (Vec::new(), Vec::new());
            let mut vals = vec![Vec::new(); nf];
            while let Some(r) = rows.next().expect("scope row") {
                profits.push(r.get::<_, f64>(nf).expect("profit"));
                closes.push(r.get::<_, i64>(nf + 1).expect("closedate"));
                for (fi, col) in vals.iter_mut().enumerate() {
                    let v = r.get::<_, Option<f64>>(fi).expect("field");
                    col.push(v.filter(|v| v.is_finite()).unwrap_or(0.0));
                }
            }
            let order = super::super::chronological_order(&closes, &profits, &vals);
            let gather = |c: &[f64]| order.iter().map(|t| c[*t]).collect::<Vec<f64>>();
            Scope {
                label: format!("strategy {sid} core {core} ({n} trades)"),
                profits: gather(&profits),
                vals: vals.iter().map(|c| gather(c)).collect(),
                closes: order.iter().map(|t| closes[*t]).collect(),
            }
        })
        .collect()
}

/// Measure composition against the plain joint search on a REAL report, on the only figure that
/// matters: what each earns on the period neither of them was allowed to fit.
///
/// Opt-in, because it needs a snapshot and takes minutes:
/// `MOON_TUNER_BENCH_DB=<snapshot> cargo test -p moon-core compose -- --ignored --nocapture`
///
/// This is a DIAGNOSTIC, not an assertion, and deliberately so. The fold count and the cut points
/// are a judgement call; whether they are the right one is an empirical question about a
/// particular user's trading, and freezing today's answer into a threshold would turn a tuning
/// decision into a test that fails when the data changes. It prints, and a human reads it.
#[test]
#[ignore]
fn composition_against_the_plain_search_on_a_real_snapshot() {
    let scopes = snapshot_scopes(4, 600);
    assert!(
        !scopes.is_empty(),
        "MOON_TUNER_BENCH_DB must name a report snapshot holding a scope worth tuning"
    );
    for scope in scopes {
        report_one_scope(scope);
    }
}

/// Run the baseline, the plain search and the composition over one tuner scope and print all
/// three. Split out so the loop above reads as "every scope, the same three numbers".
fn report_one_scope(scope: Scope) {
    let Scope {
        label,
        profits,
        vals,
        closes,
    } = scope;
    let total = profits.len();
    let cols = Arc::new(Columns { profits, vals });
    let locked = vec![None; FIELDS.len()];
    let (ne, restarts) = (32usize, 2_000usize);
    // The window's own defaults: fit on the leading 90%, measure on the rest.
    let train_n = super::super::train_split(&closes, 0.9);
    let min_n = (train_n / 10).max(1);
    let handle = SearchHandle::new();
    let full = Search::new(cols.clone(), &locked, slot_flags(), min_n, ne, train_n)
        .expect("the snapshot is larger than its min_n");

    let report = |what: &str, mask: &[bool]| {
        let out = full
            .run_masked(mask, restarts, 0x5EED, &handle)
            .expect("an uncancelled search answers");
        let applied = full.applied_ranges(&out.sel, true);
        let (tr, ho) = (
            full.tally(&applied, 0..train_n),
            full.tally(&applied, train_n..total),
        );
        let names: Vec<&str> = applied.iter().map(|(fi, _, _)| FIELDS[*fi].col).collect();
        println!(
            "[{what}] fields {} {:?}\n         in sample {:+.4} over {}\n         \
             OUT OF SAMPLE {:+.4} over {} (pf {:.2}, dd {:.4})",
            names.len(),
            names,
            tr.profit,
            tr.n,
            ho.profit,
            ho.n,
            ho.profit_factor(),
            ho.max_dd
        );
        ho.profit
    };

    println!(
        "\n[scope] {label} — train {train_n}, holdout {}",
        total - train_n
    );
    // The base rate: what the period pays with no filter at all. Without it the two searches
    // cannot be read — "out of sample +33" means one thing against a baseline of -200 and the
    // opposite against a baseline of +400.
    let base = report("no filter", &vec![false; FIELDS.len()]);
    let plain = report("plain", full.free_mask());

    let budget = budget_for(super::super::search::logical_cores()).expect(
        "the composition diagnostic needs a machine the feature actually runs on — see \
         HEAVY_SEARCH_MIN_CORES",
    );
    println!(
        "[budget] cores {} -> ranking restarts {}, folds {}, max fields {}",
        budget.cores,
        budget.ranking_restarts(restarts),
        budget.folds,
        budget.max_fields
    );
    // Through the production path, not a local rebuild of it: a diagnostic that cuts its own
    // folds can report on a geometry the product no longer builds.
    let folds = super::super::build_folds(
        &cols,
        &locked,
        &slot_flags(),
        &closes,
        min_n,
        ne,
        train_n,
        budget.folds,
    );
    println!(
        "[folds] {:?}",
        folds
            .iter()
            .map(|f| (f.search.train_n(), f.validate.end))
            .collect::<Vec<_>>()
    );
    let p = ComposeParams {
        restarts: budget.ranking_restarts(restarts),
        seed: 0x5EED,
        round: true,
        max_fields: budget.max_fields,
    };
    let out = compose(&folds, &p, &handle).expect("an uncancelled composition answers");
    let support: Vec<(&str, u8)> = out
        .chosen
        .iter()
        .enumerate()
        .filter(|(_, c)| **c)
        .map(|(fi, _)| (FIELDS[fi].col, out.support[fi]))
        .collect();
    println!("[composed] support out of {} folds: {support:?}", out.folds);
    let composed = report("composed", &out.chosen);
    println!(
        "[verdict] out of sample vs the no-filter baseline {base:+.4}: \
         plain {:+.4}, composed {:+.4}",
        plain - base,
        composed - base
    );
}

// ─────────────────────────────── fold isolation ───────────────────────────────

/// A fold's thresholds must be fitted without any knowledge of the rows it is then measured on.
///
/// Rewriting the fold's VALIDATION rows onto a scale its fitting prefix never held moves every
/// quantile edge that stretch would contribute. If the fold saw it, its chosen ranges move with
/// it. The oracle is that rewrite — a controlled mutation of the fixture, owing nothing to the
/// code under test — and the assertion is that the fitted side does not budge while the measured
/// side does.
///
/// The folds come from `mod.rs:build_folds`, the production path, and NOT from a fixture helper
/// of this file's own. That is the whole point: what is under test is the `fit_end` that function
/// hands each fold's `Search`, so a test that assembles its own folds would prove only that
/// `Search` honours the window it was given — which was never in doubt, and which leaves the
/// argument that decides the leak untested.
///
/// Breakage this pins: `mod.rs:build_folds` constructing a fold's `Search` with the whole fitting
/// region as its `train_n` instead of that fold's own `fit_end` — the obvious "why derive the
/// edges K times" optimization. Nothing would crash and every number would still look plausible;
/// the fold scores would simply get better, the composed set would be chosen on the stretches
/// meant to judge it, and the figure the window prints as out-of-sample would be a fitted one
/// wearing an out-of-sample label.
#[test]
fn a_folds_thresholds_are_fitted_without_its_validation_rows() {
    let (profits, vals, closes) = sample(600, 20260729);
    // By hand from the anchored geometry — `600 * (2 + j) / 4` is 300 then 450, and the final
    // endpoint is the region itself: folds (300, 450) and (450, 600).
    let (train_n, k) = (600usize, 2usize);
    let (fit_end, validate_end) = (300usize, 450usize);
    let mut rewritten = vals.clone();
    for col in rewritten.iter_mut() {
        for v in col[fit_end..validate_end].iter_mut() {
            *v = *v * 3.0 + 1000.0;
        }
    }
    let build = |vals: Vec<Vec<f64>>| {
        let cols = Arc::new(Columns {
            profits: profits.clone(),
            vals,
        });
        let locked = vec![None; FIELDS.len()];
        super::super::build_folds(&cols, &locked, &slot_flags(), &closes, 10, 16, train_n, k)
    };
    let built = build(vals);
    let altered_folds = build(rewritten);
    assert_eq!(
        built.len(),
        2,
        "the fixture must produce the two folds above"
    );
    let (plain, altered) = (&built[0], &altered_folds[0]);
    // The window the fold was actually built over, read back from the production path rather
    // than from the fixture: under the named mutation this is the whole region.
    assert_eq!(
        (plain.search.train_n(), plain.validate.clone()),
        (fit_end, fit_end..validate_end),
        "the first fold must fit on its own prefix and be measured on the stretch after it"
    );

    let mask = vec![true; FIELDS.len()];
    let handle = SearchHandle::new();
    let run = |f: &Fold| {
        let out = f
            .search
            .run_masked(&mask, 4, 0xF01D, &handle)
            .expect("an uncancelled search over a sufficient fixture always answers");
        let applied = f.search.applied_ranges(&out.sel, false);
        let profit = f.search.tally(&applied, f.validate.clone()).profit;
        (applied, profit)
    };
    let (plain_ranges, plain_profit) = run(plain);
    let (altered_ranges, altered_profit) = run(altered);

    assert_eq!(
        plain_ranges, altered_ranges,
        "the fitted ranges moved, so the fold read the rows it is measured on"
    );
    assert_ne!(
        plain_profit.to_bits(),
        altered_profit.to_bits(),
        "the fixture must actually change what the validation rows are worth, or this test \
         cannot distinguish a fold that peeked from one that did not"
    );
}
