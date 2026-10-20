use super::best_span;

/// Enumerate attainable circular bounds independently of the prefix/deque implementation.
fn brute(rows: &[(usize, f64)], period: usize, min_n: usize, round: bool) -> f64 {
    let mut best = rows.iter().map(|r| r.1).sum::<f64>();
    let coords: Vec<_> = (0..period)
        .filter(|v| round || rows.iter().any(|r| r.0 == *v))
        .collect();
    for &from in &coords {
        if round && !from.is_multiple_of(5) {
            continue;
        }
        for &to in &coords {
            if round && !to.is_multiple_of(5) && to != period - 1 {
                continue;
            }
            let kept: Vec<_> = rows
                .iter()
                .filter(|r| {
                    if from <= to {
                        from <= r.0 && r.0 <= to
                    } else {
                        r.0 >= from || r.0 <= to
                    }
                })
                .collect();
            if kept.len() >= min_n {
                best = best.max(kept.iter().map(|r| r.1).sum());
            }
        }
    }
    best
}

/// Quantile capping, linear-only search, and relaxing min_n all disagree with enumeration.
#[test]
fn exact_circular_matches_brute_beyond_512_rows() {
    let mut seed = 991u64;
    for period in [60, 1440, 10080] {
        for trial in 0..12 {
            let rows: Vec<_> = (0..650)
                .map(|i| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    ((i * 17) % period, ((seed >> 32) % 21) as f64 - 10.0)
                })
                .collect();
            let min_n = 1 + trial * 49;
            let span = best_span(rows.iter().copied(), period, min_n, false);
            let got: f64 = rows
                .iter()
                .filter(|r| {
                    span.is_none_or(|(f, t)| {
                        if f <= t {
                            f as usize <= r.0 && r.0 <= t as usize
                        } else {
                            r.0 >= f as usize || r.0 <= t as usize
                        }
                    })
                })
                .map(|r| r.1)
                .sum();
            assert_eq!(
                got,
                enumerate_exact(&rows, period, min_n),
                "period={period} min={min_n}"
            );
        }
    }
}

/// Quadratic enumeration accumulates every possible circular interval without pruning.
fn enumerate_exact(rows: &[(usize, f64)], period: usize, min_n: usize) -> f64 {
    let mut buckets = vec![(0usize, 0.0); period];
    for &(minute, profit) in rows {
        buckets[minute].0 += 1;
        buckets[minute].1 += profit;
    }
    let groups: Vec<_> = buckets.into_iter().filter(|r| r.0 > 0).collect();
    let mut best = rows.iter().map(|r| r.1).sum::<f64>();
    for start in 0..groups.len() {
        let (mut n, mut profit) = (0, 0.0);
        for len in 0..groups.len() {
            let group = groups[(start + len) % groups.len()];
            n += group.0;
            profit += group.1;
            if n >= min_n {
                best = best.max(profit);
            }
        }
    }
    best
}

/// Circular windows on every time axis must keep both sides of the boundary.
#[test]
fn wrap_and_minimum_on_all_axes() {
    for period in [60, 1440, 10080] {
        let rows = [(0, 5.0), (period / 2, -20.0), (period - 1, 6.0)];
        assert_eq!(
            best_span(rows.into_iter(), period, 2, false),
            Some(((period - 1) as u16, 0))
        );
        assert_eq!(best_span(rows.into_iter(), period, 3, false), None);
    }
}

/// Five-minute candidates must be rescored, preserving final-minute and wrap boundaries.
#[test]
fn rounded_search_scores_saved_bounds() {
    for period in [60, 1440, 10080] {
        let rows = [(0, 7.0), (period - 3, 9.0), (period - 6, -30.0), (5, -20.0)];
        let span = best_span(rows.into_iter(), period, 2, true).unwrap();
        assert_eq!(span, ((period - 5) as u16, 0));
        let edge = [(period - 3, 9.0), (0, -10.0)];
        assert_eq!(
            best_span(edge.into_iter(), period, 1, true),
            Some(((period - 5) as u16, (period - 1) as u16))
        );
    }
    let rows = [(1, 20.0), (4, -30.0), (12, 10.0), (16, -50.0)];
    let span = best_span(rows.into_iter(), 60, 1, true).unwrap();
    let pnl: f64 = rows
        .iter()
        .filter(|r| {
            if span.0 <= span.1 {
                span.0 as usize <= r.0 && r.0 <= span.1 as usize
            } else {
                r.0 >= span.0 as usize || r.0 <= span.1 as usize
            }
        })
        .map(|r| r.1)
        .sum();
    assert_eq!(pnl, brute(&rows, 60, 1, true));
}
