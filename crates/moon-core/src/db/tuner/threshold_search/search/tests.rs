//! Tests for the threshold-search core.
//!
//! The optimizer was rewritten for speed under an explicit promise that its ANSWER does not
//! change. A test that merely checks the fast path is self-consistent cannot see a regression
//! there, so the oracle here is a REFERENCE implementation of the original algorithm — the
//! exhaustive O(ne²) pair scan over every trade, run SEQUENTIALLY — kept in the tree permanently
//! and compared against the fast path on randomized samples. Being a child module, it reads
//! `Search`'s private fields directly, so the production code carries no test-only branch.
//!
//! Since the restarts fan out across a worker pool, that comparison carries a second promise:
//! the parallel search must return, bit for bit, what a sequential run of the same restarts
//! returns. The reference deliberately shares `restart_seed` — the seeding scheme IS the contract
//! that makes the fan-out reproducible, and it is three inspectable lines — while everything the
//! oracle exists to check, the pair search and the buffer reuse, stays independent.

use super::*;
use crate::db::tuner::threshold_search::SearchHandle;
use crate::db::tuner::{FieldClass, FIELDS};

/// A handle no test cancels, for the searches that are meant to run to completion.
fn uncancelled() -> SearchHandle {
    SearchHandle::new()
}

/// Deterministic value generator, so a fixture is reproducible and comparable across runs.
///
/// Deliberately NOT the search's own PRNG: fixtures must not shift when that stream changes.
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

    /// Uniform integer in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

// ─────────────────────────── reference implementation (the oracle) ───────────────────────────

/// Exhaustive pair scan: the search's original candidate loop, kept as the oracle for
/// [`best_pair`].
///
/// Enumerates `i` then `j` ascending and keeps the first strict maximum, which is exactly the
/// tie-breaking rule the fast path has to reproduce.
fn best_pair_reference(
    pre_profit: &[f64],
    pre_count: &[usize],
    min_n: usize,
    tot_c: usize,
    floor: f64,
) -> Option<(usize, usize)> {
    let ne = pre_profit.len() - 1;
    let mut best: Option<(usize, usize)> = None;
    let mut best_p = floor;
    for i in 0..ne {
        for j in (i + 1)..=ne {
            let c = pre_count[j] - pre_count[i];
            if c < min_n {
                continue;
            }
            if c == tot_c {
                continue;
            }
            let p = pre_profit[j] - pre_profit[i];
            if p > best_p {
                best_p = p;
                best = Some((i, j));
            }
        }
    }
    best
}

/// The whole original restart loop: full-trade scans and the exhaustive pair search, one restart
/// after another on this thread.
///
/// Mirrors the fast path's use of the PRNG exactly — the same draws in the same order within a
/// restart, from the same per-restart seed — so a difference in the result is a difference in the
/// ALGORITHM or in the merge, not in the random stream. Keeping the best only on a STRICT
/// improvement is the sequential tie-break the parallel merge has to reproduce.
fn run_reference(s: &Search, allow: &[bool], restarts: usize, seed: u64) -> Option<Outcome> {
    let mut best_global: Option<Outcome> = None;
    for restart in 0..restarts {
        let mut rng = Rng(restart_seed(seed, restart));
        let outcome = one_restart_reference(s, allow, restart, &mut rng);
        if best_global
            .as_ref()
            .is_none_or(|best| outcome.profit > best.profit)
        {
            best_global = Some(outcome);
        }
    }
    best_global
}

/// One restart, computed the original way, over the fields `allow` admits.
///
/// The draws are taken for every SEARCHABLE field — both at initialization and in the per-pass
/// shuffle — and only the assignment and the visit are withheld, mirroring the production rule
/// that keeps the random stream a property of the search rather than of the mask.
fn one_restart_reference(s: &Search, allow: &[bool], restart: usize, rng: &mut Rng) -> Outcome {
    let nf = s.bins.len();
    let (n, ne) = (s.n, s.ne);
    let mut sel: Vec<Option<(usize, usize)>> = vec![None; nf];
    if restart > 0 {
        let mut slots_used = 0usize;
        for (fi, sl) in sel.iter_mut().enumerate() {
            if !s.free[fi] {
                continue;
            }
            if rng.below(100) < 30 {
                if s.is_slot[fi] {
                    if slots_used >= 2 {
                        continue;
                    }
                    slots_used += 1;
                }
                let i = rng.below(ne);
                let j = i + 1 + rng.below(ne - i);
                if allow[fi] {
                    *sl = Some((i, j));
                }
            }
        }
    }
    let mut pass: Vec<Vec<bool>> = vec![vec![true; n]; nf];
    // `base_fail` spans the whole sample; a restart only ever addresses the train window.
    let mut fail: Vec<u16> = s.base_fail[..n].to_vec();
    for fi in 0..nf {
        if let Some((i, j)) = sel[fi] {
            for t in 0..n {
                let b = s.bins[fi][t];
                let ok = b != BELOW && (i..j).contains(&(b as usize));
                if !ok {
                    fail[t] += 1;
                }
                pass[fi][t] = ok;
            }
        }
    }
    // Over every SEARCHABLE field, like the initialization above and for the same reason: the
    // shuffle spends one draw per element, so ordering only the allowed ones would make the
    // random stream a property of the mask.
    let mut order: Vec<usize> = (0..nf).filter(|fi| s.free[*fi]).collect();
    for _pass_no in 0..MAX_PASSES {
        if restart > 0 {
            for k in (1..order.len()).rev() {
                order.swap(k, rng.below(k + 1));
            }
        }
        let mut changed = false;
        for &fi in &order {
            if !allow[fi] {
                continue;
            }
            let best = best_for_field_reference(s, fi, &sel, &pass[fi], &fail);
            if best != sel[fi] {
                sel[fi] = best;
                changed = true;
                for t in 0..n {
                    let ok = match best {
                        None => true,
                        Some((i, j)) => {
                            let b = s.bins[fi][t];
                            b != BELOW && (i..j).contains(&(b as usize))
                        }
                    };
                    if ok != pass[fi][t] {
                        if ok {
                            fail[t] -= 1;
                        } else {
                            fail[t] += 1;
                        }
                        pass[fi][t] = ok;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    loop {
        let mut removed = false;
        for fi in 0..nf {
            if !allow[fi] || sel[fi].is_none() {
                continue;
            }
            let redundant = (0..n).all(|t| pass[fi][t] || fail[t] > 1);
            if redundant {
                sel[fi] = None;
                for t in 0..n {
                    if !pass[fi][t] {
                        fail[t] -= 1;
                        pass[fi][t] = true;
                    }
                }
                removed = true;
            }
        }
        if !removed {
            break;
        }
    }
    let mut profit = 0.0f64;
    for t in 0..n {
        if fail[t] == 0 {
            profit += s.cols.profits[t];
        }
    }
    Outcome { sel, profit }
}

/// One field's best range, computed by scanning every trade and every pair.
fn best_for_field_reference(
    s: &Search,
    fi: usize,
    sel: &[Option<(usize, usize)>],
    selfpass: &[bool],
    fail: &[u16],
) -> Option<(usize, usize)> {
    let (n, ne) = (s.n, s.ne);
    let mut bp = vec![0.0f64; ne + 1];
    let mut bc = vec![0usize; ne + 1];
    let (mut tot_p, mut tot_c) = (0.0f64, 0usize);
    for t in 0..n {
        let others_ok = fail[t] == u16::from(!selfpass[t]);
        if !others_ok {
            continue;
        }
        tot_p += s.cols.profits[t];
        tot_c += 1;
        let b = s.bins[fi][t];
        let idx = if b == BELOW { ne } else { b as usize };
        bp[idx] += s.cols.profits[t];
        bc[idx] += 1;
    }
    let mut pre_p = vec![0.0f64; ne + 1];
    let mut pre_c = vec![0usize; ne + 1];
    for k in 0..ne {
        pre_p[k + 1] = pre_p[k] + bp[k];
        pre_c[k + 1] = pre_c[k] + bc[k];
    }
    let slot_full = s.is_slot[fi]
        && s.locked_slots
            + sel
                .iter()
                .enumerate()
                .filter(|(o, x)| *o != fi && s.is_slot[*o] && x.is_some())
                .count()
            >= 2;
    if slot_full {
        return None;
    }
    best_pair_reference(
        &pre_p,
        &pre_c,
        s.min_n,
        tot_c,
        tot_p + improvement_margin(tot_p),
    )
}

// ─────────────────────────────────── fixtures ───────────────────────────────────

/// Which strategy slot class each `FIELDS` entry belongs to, as the search expects it.
fn slot_flags() -> Vec<bool> {
    FIELDS
        .iter()
        .map(|s| s.class == FieldClass::DeltaSlot)
        .collect()
}

/// Nothing fixed: every field is searched.
fn all_free() -> Vec<Option<(Option<f64>, Option<f64>)>> {
    vec![None; FIELDS.len()]
}

/// Build a synthetic sample of `n` trades shaped like real report data.
///
/// Two properties matter and are reproduced deliberately: several columns are mostly ONE repeated
/// value, which collapses quantile edges into duplicates and is what makes the full-coverage
/// rejection fire on ranges other than the full span; and profit is heavy-tailed around zero.
fn synthetic(n: usize, seed: u64) -> (Vec<f64>, Vec<Vec<f64>>) {
    let mut rng = Lcg(seed | 1);
    let nf = FIELDS.len();
    let mut profits = Vec::with_capacity(n);
    let mut vals = vec![Vec::with_capacity(n); nf];
    for _ in 0..n {
        let u = rng.unit();
        profits.push((u - 0.52) * 40.0 / (0.05 + rng.unit()));
        for (fi, col) in vals.iter_mut().enumerate() {
            let v = if fi % 3 == 0 && rng.unit() < 0.8 {
                0.0
            } else {
                (rng.unit() - 0.5) * 100.0
            };
            col.push(v);
        }
    }
    (profits, vals)
}

/// A sample with heavy ties in profit, so equal-profit candidate pairs are common.
///
/// The tie-break is the part of the fast path that a "does it find a good answer" test cannot
/// see: any of several pairs scores the same, and only one of them matches the original.
fn tie_heavy(n: usize, seed: u64) -> (Vec<f64>, Vec<Vec<f64>>) {
    let mut rng = Lcg(seed | 1);
    let nf = FIELDS.len();
    let mut profits = Vec::with_capacity(n);
    let mut vals = vec![Vec::with_capacity(n); nf];
    for _ in 0..n {
        // Only three distinct profits, so many disjoint ranges sum to exactly the same number.
        profits.push([-1.0, 0.0, 1.0][rng.below(3)]);
        for col in vals.iter_mut() {
            // Few distinct values per column: duplicate quantile edges everywhere.
            col.push(rng.below(4) as f64);
        }
    }
    (profits, vals)
}

/// Load a real report snapshot named by `MOON_TUNER_BENCH_DB`, or `None` when unset.
///
/// Ordered through the production rule, so a fixture taken from a real report is shaped exactly
/// as the scan would hand it over — including for the train/holdout split, which is meaningless
/// on an arbitrarily ordered sample.
fn from_snapshot() -> Option<(Vec<f64>, Vec<Vec<f64>>)> {
    let path = std::env::var("MOON_TUNER_BENCH_DB").ok()?;
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let nf = FIELDS.len();
    let cols = FIELDS
        .iter()
        .map(|s| format!("\"{}\"", s.col))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {cols}, COALESCE(profitbtc,0), COALESCE(closedate,0) \
         FROM orders_rep WHERE closedate > 0"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let mut rows = stmt.query([]).ok()?;
    let mut profits = Vec::new();
    let mut closes = Vec::new();
    let mut vals = vec![Vec::new(); nf];
    while let Some(r) = rows.next().ok()? {
        profits.push(r.get::<_, f64>(nf).ok()?);
        closes.push(r.get::<_, i64>(nf + 1).ok()?);
        for (fi, col) in vals.iter_mut().enumerate() {
            let v = r.get::<_, Option<f64>>(fi).ok()?;
            col.push(v.filter(|v| v.is_finite()).unwrap_or(0.0));
        }
    }
    let order = super::super::chronological_order(&closes, &profits, &vals);
    let sorted = vals
        .iter()
        .map(|col| order.iter().map(|t| col[*t]).collect())
        .collect();
    Some((order.iter().map(|t| profits[*t]).collect(), sorted))
}

/// Build a search over the WHOLE fixture, or panic with a message naming the fixture.
fn build(
    profits: Vec<f64>,
    vals: &[Vec<f64>],
    locked: &[Option<(Option<f64>, Option<f64>)>],
    min_n: usize,
    ne: usize,
) -> Search {
    let train_n = profits.len();
    build_split(profits, vals, locked, min_n, ne, train_n)
}

/// Build a search that may fit on only the first `train_n` trades of the fixture.
fn build_split(
    profits: Vec<f64>,
    vals: &[Vec<f64>],
    locked: &[Option<(Option<f64>, Option<f64>)>],
    min_n: usize,
    ne: usize,
    train_n: usize,
) -> Search {
    try_build(profits, vals, locked, min_n, ne, train_n).expect("fixture is larger than its min_n")
}

/// Build a search that may legitimately be REFUSED, for the cases where refusal is the assertion.
fn try_build(
    profits: Vec<f64>,
    vals: &[Vec<f64>],
    locked: &[Option<(Option<f64>, Option<f64>)>],
    min_n: usize,
    ne: usize,
    train_n: usize,
) -> Option<Search> {
    Search::new(
        Arc::new(Columns {
            profits,
            vals: vals.to_vec(),
        }),
        locked,
        slot_flags(),
        min_n,
        ne,
        train_n,
    )
}

/// The bounds a result finally applies, through the production rule itself.
fn applied_bounds(s: &Search, out: &Outcome, _ne: usize) -> Vec<(usize, f64, f64)> {
    s.applied_ranges(&out.sel, false)
}

// ─────────────────────────────────── tests ───────────────────────────────────

/// The optimized search must match the sequential oracle across representative problem sizes.
#[test]
fn fast_search_matches_the_reference_on_random_samples() {
    for &(n, ne, restarts, seed) in &[
        (200usize, 8usize, 5usize, 1u64),
        (200, 16, 5, 2),
        (500, 32, 4, 3),
        (300, 64, 3, 4),
    ] {
        let (profits, vals) = synthetic(n, seed);
        let search = build(profits, &vals, &all_free(), n / 5, ne);
        let fast = search.run(restarts, seed, &uncancelled());
        let slow = run_reference(&search, search.free_mask(), restarts, seed);
        assert_outcomes_match(fast, slow, &format!("synthetic n={n} ne={ne}"));
    }
}

/// Depth 512 must keep its highest real bin distinct from the defensive `BELOW` sentinel.
///
/// Breakage this pins: `search.rs:Search::new` retaining an old 8-bit-era clamp such as
/// `lo.min(255)` while the public ceiling moves to 512. The upper half of the distribution would
/// collapse into bin 255, changing every range search without an error.
#[test]
fn depth_512_keeps_bin_511_distinct_from_below() {
    let n = 513usize;
    let vals = vec![(0..n).map(|value| value as f64).collect::<Vec<_>>()];
    let search = build(vec![0.0; n], &vals, &[None], 1, 512);

    assert_eq!(search.edges[0].len(), 513);
    assert_eq!(search.bins[0][0], 0);
    assert_eq!(search.bins[0][n - 1], 511);
    assert!(
        search.bins[0].iter().all(|bin| *bin != BELOW),
        "all fixture values are at or above the first edge"
    );
}

/// Equal-profit candidates must preserve the original search's deterministic tie-breaking.
#[test]
fn fast_search_matches_the_reference_when_profits_tie() {
    for &(n, ne, restarts, seed) in &[
        (150usize, 8usize, 6usize, 11u64),
        (400, 16, 4, 12),
        (250, 32, 4, 13),
    ] {
        let (profits, vals) = tie_heavy(n, seed);
        let search = build(profits, &vals, &all_free(), 3, ne);
        let fast = search.run(restarts, seed, &uncancelled());
        let slow = run_reference(&search, search.free_mask(), restarts, seed);
        assert_outcomes_match(fast, slow, &format!("tie-heavy n={n} ne={ne}"));
    }
}

/// Fixed, excluded, and slot-constrained fields must agree with the reference search.
#[test]
fn fast_search_matches_the_reference_with_fixed_and_excluded_fields() {
    let (profits, vals) = synthetic(400, 77);
    let mut locked = all_free();
    // A field held at a fixed range, one excluded entirely, and a fixed SLOT field, which is
    // what consumes a Delta2/Delta3 slot and changes what the searched slot fields may do.
    locked[0] = Some((Some(-10.0), Some(30.0)));
    locked[3] = Some((None, None));
    let slot_index = FIELDS
        .iter()
        .position(|s| s.class == FieldClass::DeltaSlot)
        .expect("FIELDS carries at least one slot field");
    locked[slot_index] = Some((Some(-5.0), None));
    let search = build(profits, &vals, &locked, 20, 16);
    let fast = search.run(6, 0xBEEF, &uncancelled());
    let slow = run_reference(&search, search.free_mask(), 6, 0xBEEF);
    assert_outcomes_match(fast, slow, "locked fields");
}

/// Candidate legality at the minimum-trade boundary must match the exhaustive scan.
#[test]
fn fast_search_matches_the_reference_at_the_min_trades_boundary() {
    // `min_n` at, just under, and just over the sample size decides whether ANY range is legal;
    // the admissible-prefix pointer is the part that has to agree at those edges.
    let (profits, vals) = synthetic(120, 5);
    for &min_n in &[1usize, 2, 60, 118, 119, 120] {
        let search = build(profits.clone(), &vals, &all_free(), min_n, 16);
        let fast = search.run(4, 0xABCD, &uncancelled());
        let slow = run_reference(&search, search.free_mask(), 4, 0xABCD);
        assert_outcomes_match(fast, slow, &format!("min_n={min_n}"));
    }
}

/// Collapsed quantile edges must not change the selected range or its tie-breaking.
#[test]
fn fast_search_matches_the_reference_on_a_constant_column() {
    // Every value identical collapses all quantile edges onto one number, so every trade lands
    // in the top bin and MANY index pairs cover the whole sample. This is exactly the shape that
    // makes "full coverage means the (0, ne) pair" false.
    let (profits, mut vals) = synthetic(200, 9);
    for col in vals.iter_mut().step_by(2) {
        col.iter_mut().for_each(|v| *v = 7.0);
    }
    let search = build(profits, &vals, &all_free(), 10, 16);
    let fast = search.run(5, 0x1234, &uncancelled());
    let slow = run_reference(&search, search.free_mask(), 5, 0x1234);
    assert_outcomes_match(fast, slow, "constant columns");
}

/// An opt-in real report snapshot must produce the same answer through both implementations.
#[test]
#[ignore]
fn fast_search_matches_the_reference_on_the_real_snapshot() {
    // The strongest available oracle, but it needs a report snapshot and takes minutes against
    // the reference implementation, so it stays opt-in:
    // `MOON_TUNER_BENCH_DB=<snapshot> cargo test -p moon-core threshold_search -- --ignored`
    let Some((profits, vals)) = from_snapshot() else {
        panic!("MOON_TUNER_BENCH_DB must name a report snapshot for this test");
    };
    let n = profits.len();
    let search = build(profits, &vals, &all_free(), n / 5, 64);
    let fast = search.run(3, 0x5EED, &uncancelled());
    let slow = run_reference(&search, search.free_mask(), 3, 0x5EED);
    assert_outcomes_match(fast, slow, "real snapshot");
}

/// A stopped search must abandon its restarts rather than quietly finish them.
///
/// The oracle is the handle's own counter against the requested count — two decoupled numbers,
/// not a literal restated here.
///
/// Breakage this pins: dropping the `handle.is_cancelled()` guard from the restart closure in
/// `search.rs:Search::run`, or moving `record_restart` ahead of it. The Stop button would then
/// return the window to idle while every core kept working, and the result would report restarts
/// nobody waited for.
#[test]
fn a_stopped_search_abandons_its_restarts() {
    let (profits, vals) = synthetic(300, 31);
    let search = build(profits, &vals, &all_free(), 60, 16);
    let handle = SearchHandle::new();
    handle.cancel();

    assert!(
        search.run(50, 0xC0FFEE, &handle).is_none(),
        "a search stopped before its first restart has nothing to report"
    );
    assert_eq!(
        handle.completed(),
        0,
        "no restart finished, so none may be counted"
    );
}

/// A search left alone must count every restart it was asked for.
///
/// Pairs with the stopped case above: together they pin that [`SearchHandle::completed`] follows
/// the work actually done. Breakage: dropping `record_restart` after a completed restart. Live
/// progress and the final completed count would then remain below the work the search performed.
#[test]
fn a_completed_search_counts_every_restart() {
    let (profits, vals) = synthetic(300, 32);
    let search = build(profits, &vals, &all_free(), 60, 16);
    let handle = SearchHandle::new();

    assert!(search.run(12, 0xC0FFEE, &handle).is_some());
    assert_eq!(handle.completed(), 12);
}

/// Seed groups must divide one work budget without repeating deterministic restart zero.
///
/// The oracle is the contiguous partition itself: lengths sum to the caller's budget, every
/// range starts where the previous one ended, and only that first range contains global index
/// zero. This covers budgets below the requested three groups as boundary cases.
///
/// Breakage this pins: resetting every group's `start` to zero in `search.rs:restart_groups`.
/// Composition would then count the same greedy restart as three independent seed votes, and at
/// a ranking budget of three every alleged seed would be seed-independent. Removing the empty
/// outcome guard from `run_masked_seed_groups` would also make a zero budget look like valid
/// seed evidence.
#[test]
fn seed_groups_share_one_budget_and_one_greedy_restart() {
    for restarts in 1..=7usize {
        let groups = restart_groups(restarts, 3, 0x5EED);
        assert_eq!(
            groups.len(),
            restarts.min(3),
            "active groups must be bounded by both the request and available restarts"
        );
        assert_eq!(
            groups.iter().map(|group| group.len).sum::<usize>(),
            restarts,
            "seed groups must neither multiply nor lose restart work"
        );
        let mut expected_start = 0usize;
        for (index, group) in groups.iter().enumerate() {
            assert!(group.len > 0, "an active seed group cannot be empty");
            assert_eq!(
                group.start, expected_start,
                "group {index} must continue the preceding global range"
            );
            assert_eq!(
                group.start == 0,
                index == 0,
                "only the first seed group may own deterministic restart zero"
            );
            expected_start += group.len;
        }
        let unique_seeds = groups
            .iter()
            .map(|group| group.seed)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            unique_seeds.len(),
            groups.len(),
            "every active group needs an independent base seed"
        );
    }

    let (profits, vals) = synthetic(300, 33);
    let search = build(profits, &vals, &all_free(), 60, 16);
    let handle = SearchHandle::new();
    let outcomes = search
        .run_masked_seed_groups(search.free_mask(), 2, 0x5EED, 3, &handle)
        .expect("two uncancelled restart groups both answer");
    assert_eq!(outcomes.len(), 2, "two restarts can support only two seeds");
    assert_eq!(
        handle.completed(),
        2,
        "multi-seed search must keep the exact caller-owned work budget"
    );
    assert!(
        search
            .run_masked_seed_groups(search.free_mask(), 0, 0x5EED, 3, &SearchHandle::new())
            .is_none(),
        "a zero restart budget cannot produce an empty but apparently valid seed vote"
    );
}

/// Seed consensus must reject a group set after any requested restart was abandoned.
///
/// The injected abandoned flag models a stop arriving during the final group after another
/// restart in that group completed. Ordinary masked search may return that completed work, but a
/// shortened group cannot carry the same vote as two complete groups.
///
/// Breakage this pins: removing the final `handle.abandoned()` guard from
/// `search.rs:run_masked_seed_groups`. Composition could then select a field set from unequal
/// seed budgets after the user pressed Stop.
#[test]
fn seed_groups_reject_abandoned_evidence() {
    let (profits, vals) = synthetic(300, 34);
    let search = build(profits, &vals, &all_free(), 60, 16);
    let handle = SearchHandle::new();
    handle.note_abandoned();

    assert!(
        search
            .run_masked_seed_groups(search.free_mask(), 3, 0x5EED, 3, &handle)
            .is_none(),
        "an abandoned restart invalidates the whole seed vote"
    );
}

/// Trades held back from the search must not influence a single threshold it picks.
///
/// The oracle is a CONTROLLED REWRITE, not a remembered number: the same fixture is searched
/// twice, the second time with every field value in the held-back tail replaced by one far
/// outside the train window's range. Nothing the search is allowed to look at changed, so the
/// selection, its profit and the train metrics must be identical bit for bit; the holdout
/// metrics, which describe exactly the rows that were rewritten, must not be.
///
/// Breakage this pins: `search.rs:Search::new` deriving the quantile edges from the whole column
/// (`col` rather than `col[..n]`), or the descent reading past the train window. The thresholds
/// would then be placed with knowledge of the very period the holdout exists to test, so the
/// out-of-sample figure would flatter every overfitted range instead of exposing it — the one
/// number in this feature that has to be earned rather than fitted.
#[test]
fn trades_held_back_cannot_reach_the_thresholds_chosen_on_train() {
    let (n, train_n, ne, restarts, seed) = (400usize, 300usize, 16usize, 6usize, 0x5A17u64);
    let (profits, mut vals) = synthetic(n, 4242);

    let before = build_split(profits.clone(), &vals, &all_free(), 40, ne, train_n);
    let out_before = before
        .run(restarts, seed, &uncancelled())
        .expect("the fixture is large enough to produce a result");
    let bounds_before = applied_bounds(&before, &out_before, ne);
    assert!(
        !bounds_before.is_empty(),
        "the fixture must make the search choose at least one range, or this proves nothing"
    );
    let train_before = before.tally(&bounds_before, 0..train_n);
    let hold_before = before.tally(&bounds_before, train_n..n);
    assert!(
        hold_before.n > 0,
        "the chosen ranges must keep some held-back trades, or the comparison below is vacuous"
    );

    // Rewrite the held-back rows onto a scale the train window never saw. `synthetic` values sit
    // within +-50, so every rewritten value lands above 800 and outside every candidate range.
    for col in vals.iter_mut() {
        for v in col[train_n..].iter_mut() {
            *v = *v * 3.0 + 1000.0;
        }
    }

    let after = build_split(profits, &vals, &all_free(), 40, ne, train_n);
    let out_after = after
        .run(restarts, seed, &uncancelled())
        .expect("the same fixture still produces a result");
    assert_eq!(
        out_after.sel, out_before.sel,
        "held-back values moved the selected ranges"
    );
    assert_eq!(
        out_after.profit.to_bits(),
        out_before.profit.to_bits(),
        "held-back values moved the fitted profit"
    );
    let bounds_after = applied_bounds(&after, &out_after, ne);
    assert_eq!(
        bounds_after, bounds_before,
        "held-back values moved the quantile edges the bounds are read from"
    );
    let train_after = after.tally(&bounds_after, 0..train_n);
    assert_eq!(train_after.n, train_before.n);
    assert_eq!(train_after.profit.to_bits(), train_before.profit.to_bits());

    let hold_after = after.tally(&bounds_after, train_n..n);
    assert_eq!(
        hold_after.n, 0,
        "every rewritten trade sits outside the ranges, so none may still pass"
    );
}

/// The range a suggestion returns must select exactly the trades the optimizer scored it on.
///
/// The optimizer works on HALF-OPEN bin pairs while everything downstream — the KPI's SQL, the
/// saved strategy, the reported tally — applies an INCLUSIVE value range. The oracle here is
/// that independence: the profit the descent computed from its bins, against the profit obtained
/// by applying the returned bounds to the same rows. They are reached by two different routes
/// and must agree.
///
/// Breakage this pins: emitting `(edges[i], edges[j])` from `threshold_search/mod.rs` instead of
/// `Search::bound_values`. Every trade sitting exactly on the upper edge — which the descent had
/// EXCLUDED — would then pass the returned filter, so the suggestion would be scored on one
/// trade set and applied as another, and with a loss parked on that edge it can come out worse
/// than no filter at all.
#[test]
fn the_returned_range_selects_the_trades_the_optimizer_scored() {
    // Five trades whose first field takes the values 0..4. Only the first makes money, and the
    // trade sitting exactly on the NEXT quantile edge is a heavy loss — the one arrangement
    // where the half-open and the inclusive reading of a pair disagree about real money.
    let profits = vec![10.0, -100.0, 0.0, 0.0, 0.0];
    let mut vals = vec![vec![7.0; 5]; FIELDS.len()];
    vals[0] = vec![0.0, 1.0, 2.0, 3.0, 4.0];
    let ne = 4;
    let search = build(profits, &vals, &all_free(), 1, ne);

    // By hand: with five sorted values and four edges, `edges` is [0, 1, 2, 3, 4] and each trade
    // lands in the bin of its own index (the last clamped into bin 3). So the pair 0..1 selects
    // the single profitable trade and nothing else.
    let applied = search.applied_ranges(&[Some((0, 1))], false);
    assert_eq!(applied.len(), 1, "the pair filters, so it must be reported");
    let scored = search.tally(&applied, 0..5);
    assert_eq!(
        scored.n, 1,
        "the returned range passed {} trades where the pair selects 1",
        scored.n
    );
    assert!((scored.profit - 10.0).abs() < 1e-12, "{}", scored.profit);

    // And the arrangement really is one where the naive reading differs: taking the pair's upper
    // EDGE as an inclusive bound would also pass the trade valued exactly 1.
    let naive = search.tally(&[(0, 0.0, 1.0)], 0..5);
    assert_eq!(naive.n, 2, "the fixture must expose the difference");
    assert!((naive.profit - -90.0).abs() < 1e-12, "{}", naive.profit);
}

/// Fixed filters that already starve the sample must not yield a confident suggestion.
///
/// `min_n` counts the trades a suggestion must RETAIN, so a scope whose caller-fixed bounds leave
/// fewer than that cannot answer at all. The oracle is a boundary PAIR computed from the fixture
/// itself: at exactly the surviving count the search is accepted, one more and it is refused.
///
/// Breakage this pins: moving the check in `search.rs:Search::new` back to the raw train length,
/// before `base_fail` is applied. The search would then run, find no legal range, and report the
/// unfiltered baseline over a handful of trades as a completed suggestion — a train/holdout verdict
/// computed on five rows, presented exactly like one computed on thousands.
#[test]
fn fixed_filters_that_starve_the_sample_yield_no_suggestion() {
    let (profits, vals) = synthetic(200, 55);
    let mut locked = all_free();
    // A narrow fixed window on field 0, whose survivors are counted from the fixture, not assumed.
    locked[0] = Some((Some(40.0), None));
    let kept = vals[0].iter().filter(|v| **v >= 40.0).count();
    assert!(
        kept > 0 && kept < 200,
        "the fixture must leave a partial sample, not all or nothing (kept {kept})"
    );

    assert!(
        try_build(profits.clone(), &vals, &locked, kept, 16, 200).is_some(),
        "asking for exactly what the fixed filters leave must be accepted"
    );
    assert!(
        try_build(profits, &vals, &locked, kept + 1, 16, 200).is_none(),
        "asking for one more than they leave must be refused"
    );
}

/// A masked run must agree with the reference over the SAME mask, and must leave every field
/// outside the mask unfiltered.
///
/// The reference is an independent implementation of the descent, so agreement across randomized
/// masks is an oracle the fast path cannot satisfy by being self-consistent.
///
/// Breakage this pins: `search.rs:Search::one_restart` reading `self.free` instead of `allow` for
/// its visiting order or its redundancy sweep, or `run_masked` dropping the intersection that
/// keeps a caller-fixed field out. An excluded field would then acquire a range of its own, and
/// the composition built on top — which asks "is this field worth having" precisely by excluding
/// it — would be answering about a set it never proposed.
#[test]
fn a_masked_run_ignores_every_field_outside_its_mask() {
    let (profits, vals) = synthetic(400, 4242);
    let search = build(profits, &vals, &all_free(), 40, 16);
    let mut lcg = Lcg(0xC0FFEE);
    for round in 0..6u64 {
        let allow: Vec<bool> = (0..FIELDS.len()).map(|_| lcg.below(2) == 0).collect();
        let seed = 0x51DE + round;
        let fast = search.run_masked(&allow, 5, seed, &uncancelled());
        if let Some(out) = fast.as_ref() {
            for (fi, chosen) in out.sel.iter().enumerate() {
                assert!(
                    allow[fi] || chosen.is_none(),
                    "round {round}: field {fi} is outside the mask and still got a range"
                );
            }
        }
        let slow = run_reference(&search, &allow, 5, seed);
        assert_outcomes_match(fast, slow, &format!("masked run round {round}"));
    }
}

/// Two masked runs of ONE seed must consume the same random stream, so their difference is the
/// field and not the search path.
///
/// The fixture adds a CONSTANT column: it discriminates nothing, so no range over it can beat the
/// unfiltered baseline, and admitting it cannot change which trades pass. Any difference between
/// the two runs is therefore the random stream itself — which is precisely the property
/// composition rests on, since it ranks candidates by scoring the incumbent against the incumbent
/// plus one field at the same seed.
///
/// Breakage this pins: `search.rs:Search::one_restart` building `w.order` from `allow` instead of
/// `self.free`. The Fisher-Yates shuffle below it consumes one draw per element, so a shorter
/// vector spends a mask-dependent number of draws and shifts every later value; the two runs then
/// visit the SHARED fields in different orders and descend to different local optima. Nothing
/// crashes, every number stays plausible, and composition silently starts reading descent noise as
/// a field's contribution.
#[test]
fn one_seed_gives_two_masks_the_same_random_stream() {
    let (profits, mut vals) = synthetic(400, 31337);
    // The last field carries no information at all, so it can never earn a range.
    let inert = FIELDS.len() - 1;
    vals[inert].iter_mut().for_each(|v| *v = 7.0);
    let search = build(profits, &vals, &all_free(), 40, 16);

    let mut without = vec![true; FIELDS.len()];
    without[inert] = false;
    let with = vec![true; FIELDS.len()];

    for seed in [0u64, 0xBEEF, 0x5EED_1234] {
        let a = search
            .run_masked(&without, 6, seed, &uncancelled())
            .expect("an uncancelled search over a sufficient fixture always answers");
        let b = search
            .run_masked(&with, 6, seed, &uncancelled())
            .expect("an uncancelled search over a sufficient fixture always answers");
        assert_eq!(
            b.sel[inert], None,
            "seed {seed}: the inert column must not have earned a range, or this proves nothing"
        );
        assert_eq!(
            a.sel, b.sel,
            "seed {seed}: admitting a field that cannot matter changed the chosen ranges"
        );
        assert_eq!(
            a.profit.to_bits(),
            b.profit.to_bits(),
            "seed {seed}: and changed the score with them"
        );
    }
}

/// What the excluded columns CONTAIN must not reach the answer.
///
/// The mask-versus-reference test above shares the descent's own structure with the code under
/// test; this one does not. Rewriting the excluded columns onto a scale the fixture never held
/// changes their quantile edges and every bin derived from them, so any path that still reads
/// them shifts the result. The oracle is that rewrite, which owes nothing to the search.
///
/// Breakage this pins: an optimization that skips rebuilding a field's mask, or reuses a cached
/// one, on the assumption that an excluded field is unread. (The visiting-order slip is the
/// sibling test's above — this one's fixture cannot see it, because both runs here share one
/// mask and therefore one random stream.)
#[test]
fn excluded_columns_cannot_influence_a_masked_run() {
    let (profits, vals) = synthetic(300, 8080);
    // Every other field admitted, so the excluded ones are interleaved with the searched ones
    // rather than sitting in a block the loops could skip wholesale.
    let allow: Vec<bool> = (0..FIELDS.len()).map(|fi| fi % 2 == 0).collect();
    let mut rewritten = vals.clone();
    for (fi, col) in rewritten.iter_mut().enumerate() {
        if !allow[fi] {
            col.iter_mut().for_each(|v| *v = *v * 3.0 + 1000.0);
        }
    }
    let plain = build(profits.clone(), &vals, &all_free(), 30, 16);
    let altered = build(profits, &rewritten, &all_free(), 30, 16);
    assert_outcomes_match(
        plain.run_masked(&allow, 5, 0xA11CE, &uncancelled()),
        altered.run_masked(&allow, 5, 0xA11CE, &uncancelled()),
        "rewritten excluded columns",
    );
}

/// Assert two outcomes are the same selection and the same profit, bit for bit.
///
/// The trade count is not compared because it is not independent: given the sample, `sel` alone
/// determines it, so a mismatch could only show up as a `sel` mismatch first.
fn assert_outcomes_match(fast: Option<Outcome>, slow: Option<Outcome>, what: &str) {
    match (fast, slow) {
        (None, None) => {}
        (Some(f), Some(s)) => {
            assert_eq!(f.sel, s.sel, "{what}: selected ranges differ");
            assert_eq!(
                f.profit.to_bits(),
                s.profit.to_bits(),
                "{what}: profit differs ({} vs {})",
                f.profit,
                s.profit
            );
        }
        (f, s) => panic!(
            "{what}: one side found a result and the other did not ({} vs {})",
            f.is_some(),
            s.is_some()
        ),
    }
}

// ─────────────────────────────────── benchmark ───────────────────────────────────

/// How many times each configuration is timed. The reported figure is the MINIMUM: this machine
/// runs the terminal and other work alongside the benchmark, so a single sample is dominated by
/// scheduling noise, and the minimum is the least contaminated estimate of the real cost.
const BENCH_REPEATS: usize = 3;

/// Time the fast path against the reference implementation on one configuration.
///
/// Both run in the SAME process, back to back, under the same load — comparing against numbers
/// remembered from an earlier process is how noise gets mistaken for a speedup. The ratio is the
/// whole distance travelled: the reference is the ORIGINAL algorithm run sequentially, so it
/// folds the linear pair search and the restart fan-out together rather than isolating either.
fn compare(label: &str, profits: &[f64], vals: &[Vec<f64>], ne: usize, restarts: usize) {
    let min_n = (profits.len() / 5).max(1);
    let search = build(profits.to_vec(), vals, &all_free(), min_n, ne);
    let seed = 0x9E37_79B9_7F4A_7C15;
    let mut fast_best = f64::INFINITY;
    let mut slow_best = f64::INFINITY;
    let mut outcome = None;
    for _ in 0..BENCH_REPEATS {
        let started = std::time::Instant::now();
        let got = search.run(restarts, seed, &uncancelled());
        fast_best = fast_best.min(started.elapsed().as_secs_f64() * 1000.0);
        outcome = got;

        let started = std::time::Instant::now();
        let reference = run_reference(&search, search.free_mask(), restarts, seed);
        slow_best = slow_best.min(started.elapsed().as_secs_f64() * 1000.0);
        // The benchmark doubles as an equivalence check: a "speedup" that changed the answer is
        // not a speedup.
        assert_eq!(
            outcome.as_ref().map(|o| &o.sel),
            reference.as_ref().map(|o| &o.sel),
            "{label}: fast and reference disagree at ne={ne}"
        );
    }
    let profit = outcome.as_ref().map(|o| o.profit).unwrap_or(0.0);
    let ranges = outcome
        .as_ref()
        .map(|o| o.sel.iter().filter(|s| s.is_some()).count())
        .unwrap_or(0);
    println!(
        "[bench] {label:<22} n={:<6} ne={ne:<4} r={restarts:<4} \
         old {slow_best:>8.1} ms -> new {fast_best:>8.1} ms  = {:>5.2}x   \
         profit={profit:+.2} ranges={ranges}",
        profits.len(),
        slow_best / fast_best,
    );
}

/// Print what a split search actually reports on a real report, at several train shares.
///
/// A DIAGNOSTIC, not a check: it asserts only that the split leaves both sides populated, and
/// exists so the gap between the fitted and the held-back figures can be read off real data
/// rather than guessed at. Run with:
/// `MOON_TUNER_BENCH_DB=<snapshot> cargo test -p moon-core threshold_search -- --ignored --nocapture`
#[test]
#[ignore]
fn report_holdout_on_the_real_snapshot() {
    let Some((profits, vals)) = from_snapshot() else {
        panic!("MOON_TUNER_BENCH_DB must name a report snapshot for this diagnostic");
    };
    let (n, ne, restarts) = (profits.len(), 64usize, 100usize);
    println!("[holdout] trades={n} ne={ne} restarts={restarts}");
    for pct in [100usize, 80, 70, 50] {
        let train_n = n * pct / 100;
        let search = build_split(profits.clone(), &vals, &all_free(), n / 5, ne, train_n);
        let out = search
            .run(restarts, 0x5EED, &uncancelled())
            .expect("the real snapshot is far larger than its min_n");
        let applied = applied_bounds(&search, &out, ne);
        let train = search.tally(&applied, 0..train_n);
        let hold = search.tally(&applied, train_n..n);
        assert!(train.n > 0, "train share {pct}% left nothing to fit on");
        // A held-back RANGE exists exactly when a split was asked for. How many trades in it
        // still pass the ranges is the very thing being reported, so it is printed, not asserted.
        assert_eq!(train_n < n, pct < 100, "train share {pct}%");
        println!(
            "[holdout] train {pct:>3}% | ranges {:<2} | in  {:>+10.2} over {:<6} WR {:>4.1}% \
             PF {:>5.2} DD {:>9.2} | out {:>+10.2} over {:<6} WR {:>4.1}% PF {:>5.2} DD {:>9.2}",
            applied.len(),
            train.profit,
            train.n,
            train.winrate(),
            train.profit_factor(),
            train.max_dd,
            hold.profit,
            hold.n,
            hold.winrate(),
            hold.profit_factor(),
            hold.max_dd,
        );
    }
}

/// Benchmark the optimizer against the reference across sample sizes and quantile depths.
///
/// Ignored by default so `cargo test --workspace` in CI never pays for it. Run with:
/// `cargo test -p moon-core threshold_search -- --ignored --nocapture`
/// Set `MOON_TUNER_BENCH_DB` to a report snapshot to add the real-data rows.
#[test]
#[ignore]
fn bench_threshold_search() {
    println!(
        "[bench] fields={} repeats={BENCH_REPEATS} (reporting the minimum)",
        FIELDS.len()
    );
    for &n in &[500usize, 5_000] {
        let (profits, vals) = synthetic(n, 12345);
        for &ne in &[16usize, 64, 128] {
            compare("synthetic", &profits, &vals, ne, 20);
        }
    }
    let (profits, vals) = synthetic(26_000, 12345);
    for &ne in &[64usize, 128] {
        compare("synthetic", &profits, &vals, ne, 20);
    }

    match from_snapshot() {
        Some((profits, vals)) => {
            // 512 is the ceiling, and the deepest slicing is where a bin index could collide with
            // the BELOW sentinel — `compare` asserts the fast path against the reference on every
            // repeat, so this row doubles as the equivalence check at that depth.
            for &ne in &[16usize, 64, 128, 256, 512] {
                compare("real snapshot", &profits, &vals, ne, 20);
            }
            compare("real snapshot", &profits, &vals, 64, 100);
        }
        None => println!("[bench] MOON_TUNER_BENCH_DB unset - real-data rows skipped"),
    }
}
