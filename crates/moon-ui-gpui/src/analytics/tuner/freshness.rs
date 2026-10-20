//! Material-change policy for completed tuner snapshots, independent of report generations.

use moon_core::db::tuner::VarStats;

/// The scoped closed-trade count and period end visible when a run started.
#[derive(Clone, Copy, Default)]
pub(super) struct SampleSnapshot(Option<(i64, i64)>);

impl SampleSnapshot {
    /// Capture the Fact column, which includes all closed trades in the selected scope.
    pub(super) fn capture(stats: Option<&Vec<VarStats>>, end: i64) -> Self {
        Self(stats.and_then(|s| s.first()).map(|s| (s.n, end)))
    }

    /// Warn after at least five new scoped trades and five percent growth, or a new period day.
    /// Missing baselines retain the caller's conservative report-generation warning.
    pub(super) fn materially_changed(self, stats: Option<&Vec<VarStats>>, end: i64) -> bool {
        let Some((sample, previous_end)) = self.0 else {
            return true;
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
