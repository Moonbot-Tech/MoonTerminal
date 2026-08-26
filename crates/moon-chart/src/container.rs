//! Chart tab and pane descriptor types shared with the UI shell. The container logic itself
//! (open/auto/prune/layout/mode) lives in the own-pass wrapper (`chartdx::pane` in
//! moon-ui-gpui), which re-exports these types. The wgpu pane engine (`Pane{chart:Chart}`)
//! was removed together with the egui binary.

use moon_core::config::ChartBucket;

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    /// Main tab: detection clicks, centered on fullscreen use.
    Main,
    /// AddToChart chart tab identified by `num`. `bucket` determines how charts from cores
    /// within the group are consolidated (per-core / shared tab / named bundle). See `ChartBucket`.
    Chart { num: u32, bucket: ChartBucket },
}

/// Pane origin, which affects its TTL and behavior.
#[derive(Clone, Copy)]
pub enum PaneSource {
    /// Opened manually by clicking a detection and remains until closed with the close button.
    Manual,
    /// Added automatically through AddToChart and remains for `ttl_ms` after the last detection.
    ///
    /// A `ttl_ms` of `f64::INFINITY` means "keep forever" — `KeepInChart = 0` on the Moonbot
    /// strategy, see `DetectRow::keep_in_chart_ttl_ms` in `moon-core`. Such a pane never expires.
    AddToChart { born_ms: f64, ttl_ms: f64 },
}

impl PaneSource {
    /// The moment this pane expires on its own, or `None` when it never will — opened by hand, or
    /// held indefinitely.
    ///
    /// The one place that turns an infinite TTL into an ABSENT deadline: consumers ask here rather
    /// than taking `ttl_ms` apart, so "forever" cannot come back as a number arithmetic then treats
    /// as a real instant. A cap asking which chart should give way wants
    /// [`Self::last_detect_ms`] instead — absence of a deadline is not absence of a rank.
    pub fn deadline_ms(&self) -> Option<f64> {
        match *self {
            PaneSource::Manual => None,
            // Lazy on purpose: the eager form still computes the infinity this keeps out.
            PaneSource::AddToChart { born_ms, ttl_ms } => {
                ttl_ms.is_finite().then(|| born_ms + ttl_ms)
            }
        }
    }

    /// When this pane last had a reason to exist, for choosing which pane gives way under a cap.
    ///
    /// `None` only for a pane a cap must never take: one the reader opened by hand. An auto-added
    /// pane ALWAYS answers, including one held forever — "never closes on its own" and "never gives
    /// way to a newer detect" are different questions, and answering the second with
    /// [`Self::deadline_ms`] is how a capped tab stops showing new coins altogether once every
    /// chart on it is permanent.
    ///
    /// Every repeat detect pushes `born_ms` forward, so the smallest value is the chart that has
    /// gone longest without one — the same victim the deadline used to pick, now reachable whether
    /// the TTL is finite or not.
    pub fn last_detect_ms(&self) -> Option<f64> {
        match *self {
            PaneSource::Manual => None,
            PaneSource::AddToChart { born_ms, .. } => Some(born_ms),
        }
    }
}
