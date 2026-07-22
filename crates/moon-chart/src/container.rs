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
    AddToChart { born_ms: f64, ttl_ms: f64 },
}
