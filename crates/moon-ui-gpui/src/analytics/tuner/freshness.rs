//! Material-change policy for completed tuner snapshots, independent of report generations.

use moon_core::db::tuner::VarStats;

/// A completed run's scoped baseline, established from its sample or its first fresh KPI reply.
#[derive(Clone, Copy, Default)]
pub(super) struct SampleSnapshot {
    sample: Option<(i64, i64)>,
    pending_end: Option<i64>,
}

impl SampleSnapshot {
    /// Capture the Fact column, which includes all closed trades in the selected scope.
    pub(super) fn capture(stats: Option<&Vec<VarStats>>, end: i64) -> Self {
        Self {
            sample: stats.and_then(|s| s.first()).map(|s| (s.n, end)),
            pending_end: None,
        }
    }

    /// Use a pick's already returned sample count without reading the database again.
    pub(super) fn from_count(n: i64, end: i64) -> Self {
        Self {
            sample: Some((n, end)),
            pending_end: None,
        }
    }

    /// Ignore pre-fit cached counts until the existing post-fit KPI refresh publishes.
    pub(super) fn after_run(end: i64) -> Self {
        Self {
            sample: None,
            pending_end: Some(end),
        }
    }

    /// Establish the baseline once, using only a successful new KPI response, never retained data.
    pub(super) fn observe(&mut self, stats: Option<&Vec<VarStats>>) {
        if let Some(end) = self.pending_end
            && stats.is_some_and(|stats| !stats.is_empty())
        {
            *self = Self::capture(stats, end);
        }
    }

    /// Warn after at least five new scoped trades and five percent growth, or a new period day.
    /// Without a completed run's baseline there is no evidence of a material change.
    pub(super) fn materially_changed(self, stats: Option<&Vec<VarStats>>, end: i64) -> bool {
        if self.pending_end.is_some() {
            return false;
        }
        let Some((sample, previous_end)) = self.sample else {
            return false;
        };
        let threshold = 5.max(sample.saturating_add(19) / 20);
        end.div_euclid(86_400) > previous_end.div_euclid(86_400)
            || stats
                .and_then(|s| s.first())
                .is_some_and(|s| s.n.saturating_sub(sample) >= threshold)
    }
}

#[cfg(test)]
mod tests;
