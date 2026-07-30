//! Tests for the field-set composition.
//!
//! The acceptance rules are pure functions over fold-score vectors, so they are tested directly:
//! no sample, no search, no database, and an oracle that is a hand-written vector rather than
//! anything the code under test produced.
//!
//! The fold-isolation test is the one that needs a real search behind it, because what it pins is
//! a property of how a fold is BUILT rather than of how a set is chosen.

use std::sync::Arc;
use std::time::Instant;

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
/// mean alone. The inner winner or outer gate could then accept a path that caught one lucky
/// stretch, which is precisely the coincidence out-of-sample scoring exists to reject.
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

/// A temporarily weak singleton must remain reachable long enough to form a strong pair.
///
/// The oracle is a hand-written score table, independent of the threshold search: every singleton
/// loses money while fields 0+1 together score 30 on every fold. Width two is
/// enough to retain those two best losing branches and discover their synergy.
///
/// Breakage this pins: `compose.rs:beam_candidates` filtering a layer through `accepts` before
/// retaining its frontier. That one-line shortcut would discard every singleton here, so the
/// profitable pair could never be generated and the user-visible composition would stay empty.
#[test]
fn a_weak_singleton_can_survive_to_form_a_strong_pair() {
    let candidate = [true, true, true];
    let is_slot = [false, false, false];
    let scores = |mask: &[bool]| match mask {
        [true, false, false] => vec![-1.0, -1.0, -1.0],
        [false, true, false] => vec![-2.0, -2.0, -2.0],
        [false, false, true] => vec![-3.0, -3.0, -3.0],
        [true, true, false] => vec![30.0, 30.0, 30.0],
        [true, false, true] => vec![11.0, 11.0, 11.0],
        [false, true, true] => vec![12.0, 12.0, 12.0],
        other => panic!("unexpected mask {other:?}"),
    };
    let masks = beam_candidates(&candidate, &is_slot, 0, 2, 2, |mask, _, _, _| {
        Some(scores(mask))
    })
    .expect("the deterministic scorer never cancels");
    let empty = [false, false, false];
    let empty_scores = [0.0, 0.0, 0.0];
    assert!(
        masks
            .iter()
            .filter(|mask| mask.iter().filter(|chosen| **chosen).count() == 1)
            .all(|mask| !accepts(&empty_scores, &scores(mask))),
        "the fixture must make every retained singleton lose on its own"
    );
    let ranked = masks
        .into_iter()
        .map(|mask| ScoredMask {
            scores: scores(&mask),
            mask,
        })
        .collect();
    assert_eq!(
        inner_winner(&empty_scores, ranked, &empty),
        [true, true, false],
        "the beam must carry the losing branches into their jointly profitable pair"
    );
}

/// The outer gate must report the exact path whose mask it selected.
///
/// Every score is hand-written independent evidence. The cases cover a strict subset, all
/// admitted fields, no additional filters, a beam mask equal to all fields, and the zero-admitted
/// boundary where all-fields and no-filters are the same mask.
///
/// Breakage this pins: `compose.rs:gate_choice` labelling an all-fields mask as `ReducedSet` after
/// removing the strict-subset check, or restoring the asymmetric sequence that defaulted to no
/// filters whenever neither the subset nor all-fields beat both alternatives. The thresholds
/// would remain numerically plausible while the new user-facing result line described either a
/// different path or a route another candidate beat on every gate fold.
#[test]
fn the_outer_gate_reports_each_distinct_decision() {
    let no_filters = [false, false, false];
    let all_fields = [true, true, true];
    let subset = [true, false, true];

    let chosen_subset = gate_choice(
        (&no_filters, &[100.0, 100.0]),
        (&all_fields, &[110.0, 110.0]),
        (&subset, &[130.0, 125.0]),
    );
    assert_eq!(
        chosen_subset,
        GateChoice {
            decision: ComposeDecision::ReducedSet,
            mask: subset.to_vec(),
        },
        "a strict subset that beats both complete alternatives is its own decision"
    );

    let chosen_all = gate_choice(
        (&no_filters, &[100.0, 100.0]),
        (&all_fields, &[140.0, 135.0]),
        (&subset, &[130.0, 120.0]),
    );
    assert_eq!(
        chosen_all,
        GateChoice {
            decision: ComposeDecision::AllAllowedFields,
            mask: all_fields.to_vec(),
        },
        "all admitted fields must be named when their search wins"
    );

    let chosen_none = gate_choice(
        (&no_filters, &[100.0, 100.0]),
        (&all_fields, &[90.0, 90.0]),
        (&subset, &[95.0, 95.0]),
    );
    assert_eq!(
        chosen_none,
        GateChoice {
            decision: ComposeDecision::NoAdditionalFilters,
            mask: no_filters.to_vec(),
        },
        "no additional filters is a normal winning path"
    );

    let chosen_non_transitive = gate_choice(
        (&no_filters, &[100.0, 100.0]),
        (&all_fields, &[200.0, 0.0]),
        (&subset, &[120.0, 120.0]),
    );
    assert_eq!(
        chosen_non_transitive,
        GateChoice {
            decision: ComposeDecision::ReducedSet,
            mask: subset.to_vec(),
        },
        "one inconclusive alternative must not make the gate default to a route the subset beat"
    );

    let beam_reached_all = gate_choice(
        (&no_filters, &[100.0, 100.0]),
        (&all_fields, &[130.0, 130.0]),
        (&all_fields, &[130.0, 130.0]),
    );
    assert_eq!(
        beam_reached_all.decision,
        ComposeDecision::AllAllowedFields,
        "a beam path equal to all fields is not a reduced set"
    );

    let no_admitted = gate_choice(
        (&no_filters, &[100.0, 100.0]),
        (&no_filters, &[100.0, 100.0]),
        (&no_filters, &[100.0, 100.0]),
    );
    assert_eq!(
        no_admitted.decision,
        ComposeDecision::NoAdditionalFilters,
        "with no admitted fields the two identical masks have one unambiguous meaning"
    );
}

/// Composition exists exactly on machines at or above the bar, and never asks for a budget of
/// nothing when it does.
///
/// The oracle is a boundary PAIR either side of the documented core threshold, taken from the
/// bar's own constant rather than a number restated here: one core below it there must be no
/// budget at all, and at it there must be a usable one.
///
/// Breakage this pins five ways. `compose.rs:budget_for` returning a budget below the bar — the
/// feature would run on exactly the machines it was gated off, since a caller decides whether to
/// compose by whether a budget came back. Reducing `folds` below four would make every production
/// call skip composition because two inner and two gate folds are mandatory. Restoring a fixed
/// `max_fields: 6` would silently make larger admitted sets unreachable again. Restoring
/// `beam_width: 4` would discard half the competing interactions the widened search promises.
/// And `ranking_restarts` dividing without the `RESTARTS_MIN` floor: with a low restart setting
/// the ranking runs would request zero restarts, find nothing, and the feature would report "this
/// period has no answer" rather than a set.
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
            b.folds >= 4,
            "cores {cores}: {} folds cannot supply two inner and two gate folds",
            b.folds
        );
        assert_eq!(
            b.max_fields,
            FIELDS.len(),
            "cores {cores}: every admitted field count must be reachable"
        );
        assert_eq!(
            b.beam_width, 8,
            "cores {cores}: the widened search must retain eight branches per depth"
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
            at_bar.beam_width,
            at_bar.ranking_restarts(800)
        ),
        (
            far_above.folds,
            far_above.max_fields,
            far_above.beam_width,
            far_above.ranking_restarts(800)
        ),
        "past the bar the budget must stop growing"
    );
}

// ─────────────────────────────── real-snapshot diagnostic ───────────────────────────────

/// One exact strategy+core scope out of a real snapshot, in chronological order.
struct Scope {
    label: String,
    profits: Vec<f64>,
    vals: Vec<Vec<f64>>,
    closes: Vec<i64>,
}

/// Positive integer override for the opt-in real-snapshot diagnostic.
///
/// Invalid or absent values use `default`, so an accidental shell value cannot turn a local
/// benchmark into an unbounded run.
///
/// Args:
///     name: Environment variable to read.
///     default: Value used when the variable is absent or invalid.
///
/// Returns:
///     Positive configured value or `default`.
fn bench_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
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

/// Measure composition against all-fields joint search on raw-USDT rows from a REAL report.
///
/// Opt-in, because it needs a snapshot and takes minutes:
/// `MOON_TUNER_BENCH_DB=<snapshot> cargo test -p moon-core compose -- --ignored --nocapture`
///
/// This controlled diagnostic evaluates every exact strategy+core pair separately. It
/// intentionally does not reconstruct the Analytics window's period, side, emulator, deleted,
/// liquidation, or Percent-metric filters. Its output measures the selector on the snapshot, not
/// the exact row set of an arbitrary saved UI view.
///
/// The selected profits remain a diagnostic: the fold count and cut points are a judgement call,
/// and freezing today's answer into a threshold would make an evolving snapshot fail spuriously.
/// The harness still asserts that the requested number of separate scopes exists, so a grouping
/// regression cannot silently replace the requested comparison with a shorter printed report.
#[test]
#[ignore]
fn composition_against_all_fields_on_a_real_snapshot() {
    let want = bench_usize("MOON_TUNER_BENCH_SCOPES", 4).max(1);
    let min_trades = bench_usize("MOON_TUNER_BENCH_MIN_TRADES", 600);
    let scopes = snapshot_scopes(want, min_trades);
    assert_eq!(
        scopes.len(),
        want,
        "MOON_TUNER_BENCH_DB must hold {want} separate strategy+core scopes worth tuning"
    );
    for scope in scopes {
        report_one_scope(scope);
    }
}

/// Run no-filters, all-fields, and composition over one exact tuner scope.
///
/// Args:
///     scope: Chronological rows for one exact strategy+core pair.
fn report_one_scope(scope: Scope) {
    let started = Instant::now();
    let Scope {
        label,
        profits,
        vals,
        closes,
    } = scope;
    let total = profits.len();
    let cols = Arc::new(Columns { profits, vals });
    // Match the UI's "Search all" boundary: fields with nowhere to save a threshold remain
    // excluded unless a user explicitly enables them one by one.
    let locked: Vec<Option<(Option<f64>, Option<f64>)>> = FIELDS
        .iter()
        .map(|field| {
            if field.mapped() {
                None
            } else {
                Some((None, None))
            }
        })
        .collect();
    let ne = bench_usize("MOON_TUNER_BENCH_EDGES", 32)
        .clamp(super::super::EDGES_MIN, super::super::edges_max());
    let restarts = bench_usize("MOON_TUNER_BENCH_RESTARTS", 2_000)
        .clamp(super::super::RESTARTS_MIN, super::super::restarts_max());
    let train_pct = bench_usize("MOON_TUNER_BENCH_TRAIN_PCT", 90).clamp(1, 100);
    let train_n = super::super::train_split(&closes, train_pct as f64 / 100.0);
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
        "\n[scope] {label} - train {train_n}, holdout {}",
        total - train_n
    );
    // The base rate: what the period pays with no filter at all. Without it the two searches
    // cannot be read — "out of sample +33" means one thing against a baseline of -200 and the
    // opposite against a baseline of +400.
    let base = report("no filter", &vec![false; FIELDS.len()]);
    let all_fields = report("all fields", full.free_mask());

    let budget = budget_for(super::super::search::logical_cores()).expect(
        "the composition diagnostic needs a machine the feature actually runs on - see \
         HEAVY_SEARCH_MIN_CORES",
    );
    let beam_width =
        bench_usize("MOON_TUNER_BENCH_BEAM_WIDTH", budget.beam_width).clamp(1, FIELDS.len());
    println!(
        "[budget] cores {} -> ranking restarts {}, folds {}, max fields {}, beam {}",
        budget.cores,
        budget.ranking_restarts(restarts),
        budget.folds,
        budget.max_fields,
        beam_width
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
        ranking_restarts: budget.ranking_restarts(restarts),
        gate_restarts: restarts,
        seed: 0x5EED,
        round: true,
        max_fields: budget.max_fields,
        beam_width,
    };
    let out = compose(&folds, &p, &handle).expect("an uncancelled composition answers");
    let support: Vec<(&str, u8)> = out
        .chosen
        .iter()
        .enumerate()
        .filter(|(_, c)| **c)
        .map(|(fi, _)| (FIELDS[fi].col, out.support[fi]))
        .collect();
    println!(
        "[composition] decision {:?}; support out of {} folds: {support:?}",
        out.decision, out.folds
    );
    let composed = report("composed", &out.chosen);
    println!(
        "[verdict] out of sample vs the no-filter baseline {base:+.4}: \
         all fields {:+.4}, composed {:+.4}; elapsed {:.2?}",
        all_fields - base,
        composed - base,
        started.elapsed()
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
