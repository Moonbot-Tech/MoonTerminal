//! One diagnostic sample per second, and the aggregation the verdict is built from.
//!
//! FireTest never compares a storm against a quiet idle chart — a live BTC feed bakes layers on
//! its own. It compares a stage against the samples of another stage, so every number the verdict
//! uses is "this phase's value" or "this phase's value above that phase's ceiling". [`PhaseStats`]
//! is that per-phase view: it borrows the samples of one phase and answers avg/max questions
//! about them, so the verdict reads as thresholds rather than as fold expressions.

use moon_core::metrics::MetricsSnapshot;

use crate::diag;

use super::plan::Phase;

/// One second of counters plus process metrics, tagged with the phase it was taken in.
///
/// Recorded once and only ever borrowed afterwards, so it is deliberately not `Clone`.
pub(super) struct Sample {
    pub(super) phase: Phase,
    pub(super) rates: Vec<diag::DiagRate>,
    pub(super) metrics: MetricsSnapshot,
    pub(super) gpu_frame_ms: f64,
}

/// The rate of one diag counter in this sample, or `0.0` when the counter never fired.
fn rate(sample: &Sample, label: &str) -> f64 {
    sample
        .rates
        .iter()
        .find(|r| r.label == label)
        .map(|r| r.hz)
        .unwrap_or(0.0)
}

/// The samples of one phase (or of several phases treated as one), as a set of aggregate queries.
///
/// Every `avg_*` returns `0.0` for an empty set rather than `NaN`: the baseline phase can
/// legitimately be empty, and a `NaN` would silently pass every threshold comparison.
pub(super) struct PhaseStats<'a> {
    samples: Vec<&'a Sample>,
}

impl<'a> PhaseStats<'a> {
    /// Collect the samples recorded in exactly one phase.
    pub(super) fn of(samples: &'a [Sample], phase: Phase) -> Self {
        Self {
            samples: samples.iter().filter(|s| s.phase == phase).collect(),
        }
    }

    /// Treat the samples of two phases as one set — the storm verdict spans both mouse storms.
    pub(super) fn joined(a: &Self, b: &Self) -> Self {
        Self {
            samples: a
                .samples
                .iter()
                .copied()
                .chain(b.samples.iter().copied())
                .collect(),
        }
    }

    /// Whether this phase produced no samples at all — a run with no storm samples has nothing to
    /// score and must fail rather than pass on zeroes.
    pub(super) fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Mean over the set, or `0.0` when it is empty.
    fn avg(&self, f: impl Fn(&Sample) -> f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples.iter().map(|s| f(s)).sum::<f64>() / self.samples.len() as f64
    }

    /// Maximum over the set, or `0.0` when it is empty.
    fn max(&self, f: impl Fn(&Sample) -> f64) -> f64 {
        self.samples.iter().map(|s| f(s)).fold(0.0_f64, f64::max)
    }

    /// Mean rate of one diag counter over the phase — what a `*_delta` threshold measures.
    pub(super) fn avg_rate(&self, label: &str) -> f64 {
        self.avg(|s| rate(s, label))
    }

    /// Peak rate of one diag counter over the phase — what an absolute ceiling measures, and what a
    /// baseline contributes to a delta, so one hot second cannot be averaged away.
    pub(super) fn max_rate(&self, label: &str) -> f64 {
        self.max(|s| rate(s, label))
    }

    /// Mean process CPU percentage over the phase.
    pub(super) fn avg_cpu(&self) -> f64 {
        self.avg(|s| s.metrics.cpu_process as f64)
    }

    /// Peak process CPU percentage over the phase.
    pub(super) fn max_cpu(&self) -> f64 {
        self.max(|s| s.metrics.cpu_process as f64)
    }

    /// Mean process GPU percentage over the phase. Windows-only in practice: elsewhere the metric
    /// stays zero and the verdict skips its checks rather than assert on a fabricated number.
    pub(super) fn avg_gpu_process(&self) -> f64 {
        self.avg(|s| s.metrics.gpu_process as f64)
    }

    /// Peak process GPU percentage over the phase.
    pub(super) fn max_gpu_process(&self) -> f64 {
        self.max(|s| s.metrics.gpu_process as f64)
    }

    /// Mean measured GPU frame time over the phase, from the backend's own completed-frame timing.
    pub(super) fn avg_gpu_frame_ms(&self) -> f64 {
        self.avg(|s| s.gpu_frame_ms)
    }

    /// Peak measured GPU frame time over the phase.
    pub(super) fn max_gpu_frame_ms(&self) -> f64 {
        self.max(|s| s.gpu_frame_ms)
    }

    /// Peak-to-trough resident memory across the set.
    ///
    /// Samples at or below 1 MB are dropped as "metrics not available yet" rather than counted as
    /// a huge growth from zero, and fewer than two usable samples report no growth at all.
    pub(super) fn mem_growth(&self) -> f64 {
        let values: Vec<f64> = self
            .samples
            .iter()
            .map(|s| s.metrics.mem_mb as f64)
            .filter(|m| *m > 1.0)
            .collect();
        if values.len() < 2 {
            return 0.0;
        }
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(0.0_f64, f64::max);
        (max - min).max(0.0)
    }
}
