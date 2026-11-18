//! Wall-clock timings in milliseconds for the ignored UI scale tests.

use std::hint::black_box;
use std::time::{Duration, Instant};

/// Nearest-rank percentiles and the slowest measured iteration, in milliseconds.
pub(crate) struct BenchStats {
    pub(crate) p50_ms: f64,
    pub(crate) p95_ms: f64,
    pub(crate) max_ms: f64,
}

impl BenchStats {
    /// Format a stable row for comparing benchmark runs.
    fn line(&self, name: &str, iterations: u32) -> String {
        let Self {
            p50_ms,
            p95_ms,
            max_ms,
        } = self;
        format!(
            "BENCH {name} n={iterations} p50_ms={p50_ms:.2} p95_ms={p95_ms:.2} max_ms={max_ms:.2}"
        )
    }
}

/// Select the nearest-rank percentile of nonempty samples sorted by duration.
fn nearest_rank(sorted: &[Duration], percent: u64) -> Duration {
    let rank = (sorted.len() as u64 * percent).div_ceil(100);
    sorted[rank as usize - 1]
}

/// Warm up twice, then time each invocation and print one summary.
/// Callers supply an ASCII name and at least one iteration.
pub(crate) fn measure<T>(name: &str, iterations: u32, mut work: impl FnMut() -> T) -> BenchStats {
    assert!(iterations > 0, "a benchmark needs at least one iteration");
    for _ in 0..2 {
        black_box(work());
    }

    let mut elapsed = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let start = Instant::now();
        let result = black_box(work());
        elapsed.push(start.elapsed());
        drop(result);
    }
    elapsed.sort_unstable();

    let percentile_ms = |percent| nearest_rank(&elapsed, percent).as_secs_f64() * 1_000.0;
    let stats = BenchStats {
        p50_ms: percentile_ms(50),
        p95_ms: percentile_ms(95),
        max_ms: percentile_ms(100),
    };
    println!("{}", stats.line(name, iterations));
    stats
}

#[cfg(test)]
mod tests;
