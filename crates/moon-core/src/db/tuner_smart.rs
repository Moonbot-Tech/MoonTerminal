//! Smart automatic threshold tuning: coordinate descent with RANDOM RESTARTS
//! (random-restart hill climbing) across all fields at once.
//!
//! A naive suggestion optimizes each field independently, so the combined result is not
//! optimal because fields interact. Here, the state assigns every field a range or no filter;
//! each step reoptimizes one field by exhaustively testing pairs of QUANTILE edges (quantiles
//! are actual data values; the edge count is configurable and defaults to 64) while holding
//! the others fixed. Passes continue until convergence or the 16-pass cap. The descent is
//! greedy and can become trapped in local optima, so it uses multiple RESTARTS: the first starts
//! empty, while the others use random initialization and shuffled field order; the best result
//! wins.
//!
//! Semantics match tuner variants: NULL fields equal 0 (COALESCE), and bounds are inclusive.
//! The database is scanned once and all later work stays in memory, so call this ONLY from a
//! background executor.

use super::analytics::Query;
use super::read_fail::read_fail;
use super::tuner::{FieldClass, FIELDS};
use super::ReadResult;

/// Sentinel bin for a value below the first quantile edge. Quantile edges currently include
/// the observed minimum, making this a defensive, unreachable case for derived bins. The edge
/// count is at most 128, so bin indices cannot collide with the sentinel.
const BELOW: u8 = u8::MAX;

/// Minimum accepted restart count. Shared with the UI so displayed and executed counts agree.
pub const RESTARTS_MIN: usize = 1;
/// Maximum accepted restart count: the bound on a run nobody can stop.
///
/// There is no deadline and no cancel path here, and the Analytics window sits under a blocking
/// overlay until the call returns, so this ceiling is what a user can be asked to wait through.
/// It is set from measured cost. Work is strictly linear in the count — one restart is
/// `FIELDS.len()` fields × up to 16 passes × (`n` trades + ~`ne²/2` edge pairs) — and the
/// workspace crates compile unoptimized in the dev profile that ships. Measured there at the
/// maximum depth: ~17 ms per restart over a few hundred trades, ~33 ms over five thousand. The
/// ceiling therefore costs well under a minute on a small report and a few minutes on a large
/// one. Going higher needs a cancel path first, not a bigger number.
pub const RESTARTS_MAX: usize = 2000;

/// Minimum quantile-edge count accepted by the search and exposed to the UI.
pub const EDGES_MIN: usize = 4;
/// Maximum quantile-edge count; keeps bin indices clear of the `BELOW` sentinel.
pub const EDGES_MAX: usize = 128;

/// Final range for one field.
#[derive(Clone, Debug)]
pub struct SmartField {
    pub field: &'static str,
    pub from: f64,
    pub to: f64,
}

/// Smart-suggestion result: ranges plus the combination's expected profit and trade count.
#[derive(Clone, Debug)]
pub struct SmartResult {
    pub fields: Vec<SmartField>,
    pub profit: f64,
    pub n: i64,
    /// Number of completed restarts.
    pub rounds: usize,
}

/// Simple PRNG (xorshift64*) that avoids a rand dependency. The seed comes from invocation time,
/// so every "Suggest all" click tries its OWN random starts. This is intentional: repeated
/// searches keep looking for new local optima instead of replaying one sequence.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Maximize combined profit with random-restart coordinate descent while
/// retaining at least `min_n` trades.
///
/// `restarts` is clamped to `RESTARTS_MIN..=RESTARTS_MAX`, and each attempt runs at most 16
/// passes.
/// `edges_want` controls quantile resolution per field. At most two Delta2/3
/// slot fields may carry ranges because the strategy format cannot store more.
/// `locked[fi]` is `None` for a searched field, a fixed range for an active
/// locked field, or `(None, None)` for an excluded field.
/// `Ok(None)` means the sample is too small; `NotReady` means the replica or required schema is
/// absent, while `Failed` means opening or scanning the replica failed.
pub fn smart_suggest(
    q: &Query,
    restarts: usize,
    min_n: i64,
    locked: &[Option<(Option<f64>, Option<f64>)>],
    edges_want: usize,
    round: bool,
) -> ReadResult<Option<SmartResult>> {
    const CTX: &str = "tuner: smart_suggest";
    let ne = edges_want.clamp(EDGES_MIN, EDGES_MAX);
    let (conn, q, src) = super::tuner::open_tuner_source(q)?;
    let nf = FIELDS.len();
    let cols = FIELDS
        .iter()
        .map(|s| format!("o.\"{}\"", s.col))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT {cols}, COALESCE(o.pnl,0) FROM {src}");

    // Scan profit and effective values (COALESCE 0) into column-oriented memory.
    let mut profits: Vec<f64> = Vec::new();
    let mut vals: Vec<Vec<f64>> = vec![Vec::new(); nf];
    {
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let mut rows = stmt
            .query(rusqlite::params![q.from, q.to])
            .map_err(|e| read_fail(CTX, e))?;
        // Every value feeds the optimiser, so an unreadable cell aborts the
        // calculation. NULL deliberately maps to 0.0; a read error does not.
        while let Some(r) = rows.next().map_err(|e| read_fail(CTX, e))? {
            profits.push(r.get(nf).map_err(|e| read_fail(CTX, e))?);
            for (fi, col) in vals.iter_mut().enumerate() {
                let v = r.get::<_, Option<f64>>(fi).map_err(|e| read_fail(CTX, e))?;
                col.push(v.filter(|v| v.is_finite()).unwrap_or(0.0));
            }
        }
    }
    let n = profits.len();
    let min_n = min_n.max(1) as usize;
    if n < min_n {
        // Not enough sample — a legitimate "no suggestion", not a failure.
        return Ok(None);
    }

    // Quantile edges and each trade's bin for every field.
    let mut edges: Vec<Vec<f64>> = Vec::with_capacity(nf);
    let mut bins: Vec<Vec<u8>> = Vec::with_capacity(nf);
    for col in &vals {
        let mut sorted = col.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let e: Vec<f64> = (0..=ne).map(|k| sorted[k * (n - 1) / ne]).collect();
        let b = col
            .iter()
            .map(|v| {
                if *v < e[0] {
                    BELOW
                } else {
                    // The last edge is inclusive, so clamp it into the top bin.
                    let mut lo = 0usize;
                    let mut hi = ne;
                    while lo + 1 < hi {
                        let m = (lo + hi) / 2;
                        if *v >= e[m] {
                            lo = m
                        } else {
                            hi = m
                        }
                    }
                    lo.min(ne - 1) as u8
                }
            })
            .collect();
        edges.push(e);
        bins.push(b);
    }

    // Multi-start: restart 0 is greedy from an empty state; subsequent restarts use random
    // initialization and shuffled field order.
    let restarts = restarts.clamp(RESTARTS_MIN, RESTARTS_MAX);
    let is_slot: Vec<bool> = FIELDS
        .iter()
        .map(|s| s.class == FieldClass::DeltaSlot)
        .collect();
    // Searchable fields; fixed masks remain embedded in the baseline failure counts.
    let free: Vec<bool> = (0..nf)
        .map(|fi| locked.get(fi).is_none_or(|l| l.is_none()))
        .collect();
    let mut base_fail: Vec<u16> = vec![0; n];
    for fi in 0..nf {
        let Some(Some((lo, hi))) = locked.get(fi) else {
            continue;
        };
        if lo.is_none() && hi.is_none() {
            continue;
        }
        for t in 0..n {
            let v = vals[fi][t];
            let ok = lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h);
            if !ok {
                base_fail[t] += 1;
            }
        }
    }
    // Slots occupied by fixed fields reduce the available Delta2/Delta3 pair limit.
    let locked_slots: usize = (0..nf)
        .filter(|fi| {
            is_slot[*fi]
                && matches!(locked.get(*fi), Some(Some((lo, hi))) if lo.is_some() || hi.is_some())
        })
        .count();
    // Seed from invocation time so each search gets its own random starts. Restart 0 remains
    // deterministic because it is greedy from empty. Xorshift forbids a zero seed, so mix in
    // a constant.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1);
    let mut best_global: Option<(f64, i64, Vec<Option<(usize, usize)>>)> = None;
    for restart in 0..restarts {
        let mut sel: Vec<Option<(usize, usize)>> = vec![None; nf];
        if restart > 0 {
            // About 30% of fields start with a random range, with at most two slots.
            let mut slots_used = 0usize;
            for (fi, s) in sel.iter_mut().enumerate() {
                if !free[fi] {
                    continue;
                }
                if rng.below(100) < 30 {
                    if is_slot[fi] {
                        if slots_used >= 2 {
                            continue;
                        }
                        slots_used += 1;
                    }
                    let i = rng.below(ne);
                    let j = i + 1 + rng.below(ne - i);
                    *s = Some((i, j));
                }
            }
        }
        // Masks from initialization, layered over fixed filters.
        let mut pass: Vec<Vec<bool>> = vec![vec![true; n]; nf];
        let mut fail: Vec<u16> = base_fail.clone();
        for fi in 0..nf {
            if let Some((i, j)) = sel[fi] {
                for t in 0..n {
                    let b = bins[fi][t];
                    let ok = b != BELOW && (i..j).contains(&(b as usize));
                    if !ok {
                        fail[t] += 1;
                    }
                    pass[fi][t] = ok;
                }
            }
        }
        let mut order: Vec<usize> = (0..nf).filter(|fi| free[*fi]).collect();
        for _pass_no in 0..16 {
            if restart > 0 {
                // Fisher-Yates gives each pass its own field order.
                for k in (1..order.len()).rev() {
                    order.swap(k, rng.below(k + 1));
                }
            }
            let mut changed = false;
            for &fi in &order {
                // Sum bins among trades that pass every other field.
                let selfpass = &pass[fi];
                let mut bp = vec![0.0f64; ne + 1]; // Bins followed by BELOW.
                let mut bc = vec![0usize; ne + 1];
                let (mut tot_p, mut tot_c) = (0.0f64, 0usize);
                for t in 0..n {
                    let others_ok = fail[t] == u16::from(!selfpass[t]);
                    if !others_ok {
                        continue;
                    }
                    tot_p += profits[t];
                    tot_c += 1;
                    let b = bins[fi][t];
                    let idx = if b == BELOW { ne } else { b as usize };
                    bp[idx] += profits[t];
                    bc[idx] += 1;
                }
                // Candidates are no filter and every edge pair `(i, j)`.
                let mut pre_p = vec![0.0f64; ne + 1];
                let mut pre_c = vec![0usize; ne + 1];
                for k in 0..ne {
                    pre_p[k + 1] = pre_p[k] + bp[k];
                    pre_c[k + 1] = pre_c[k] + bc[k];
                }
                let mut best: Option<(usize, usize)> = None;
                // The no-filter candidate is the baseline. A range must beat it
                // beyond floating-point noise because bin sums and `tot_p`
                // accumulate in different orders; otherwise an equivalent trade
                // set can win by about 1e-12 and leave a no-op filter.
                let mut best_p = tot_p + super::metrics::improvement_margin(tot_p);
                // Only two Delta2/Delta3 slots exist. If two other slots already
                // have ranges, this field may only choose no filter.
                let slot_full = is_slot[fi]
                    && locked_slots
                        + sel
                            .iter()
                            .enumerate()
                            .filter(|(o, s)| *o != fi && is_slot[*o] && s.is_some())
                            .count()
                        >= 2;
                if !slot_full {
                    for i in 0..ne {
                        for j in (i + 1)..=ne {
                            let c = pre_c[j] - pre_c[i];
                            if c < min_n {
                                continue;
                            }
                            // A candidate that passes every trade in the current
                            // combination is functionally no filter, including a
                            // full-span range, so reject it by exact integer count.
                            if c == tot_c {
                                continue;
                            }
                            let p = pre_p[j] - pre_p[i];
                            if p > best_p {
                                best_p = p;
                                best = Some((i, j));
                            }
                        }
                    }
                }
                if best != sel[fi] {
                    // Rebuild this field's pass mask and failure counts.
                    sel[fi] = best;
                    changed = true;
                    for t in 0..n {
                        let ok = match best {
                            None => true,
                            Some((i, j)) => {
                                let b = bins[fi][t];
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
        // Final cleanup of jointly redundant ranges: descent may hit the pass limit before
        // removing a field whose rejected trades are already rejected by OTHER filters.
        // A field uniquely rejects row t only when !pass && fail==1. Such a range is a no-op:
        // removing it changes neither profit nor count. Repeat until stable.
        loop {
            let mut removed = false;
            for fi in 0..nf {
                if !free[fi] || sel[fi].is_none() {
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
        // Compare this restart's result with the global best.
        let (mut profit, mut cnt) = (0.0f64, 0i64);
        for t in 0..n {
            if fail[t] == 0 {
                profit += profits[t];
                cnt += 1;
            }
        }
        if best_global.as_ref().is_none_or(|(bp, _, _)| profit > *bp) {
            best_global = Some((profit, cnt, sel.clone()));
        }
    }

    // Defensive fallback when no restart completed; clamping normally makes this unreachable.
    let Some((profit, cnt, sel)) = best_global else {
        return Ok(None);
    };
    let fields = sel
        .iter()
        .enumerate()
        .filter_map(|(fi, s)| {
            // A pair spanning both data edges filters nothing, so omit it.
            let (i, j) = (*s).filter(|(i, j)| !(*i == 0 && *j == ne))?;
            let (mut from, mut to) = (edges[fi][i], edges[fi][j]);
            if round {
                (from, to) =
                    super::tuner::round_pair_outward(from, to, edges[fi][0], edges[fi][ne]);
            }
            Some(SmartField {
                field: FIELDS[fi].col,
                from,
                to,
            })
        })
        .collect();
    Ok(Some(SmartResult {
        fields,
        profit,
        n: cnt,
        rounds: restarts,
    }))
}
