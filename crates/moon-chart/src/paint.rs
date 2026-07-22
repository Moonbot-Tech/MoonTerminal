//! Shared time utilities for the UI shell. The wgpu pane renderer (`render_panes`) and
//! visible-pane signature (`panes_visible_sig`) were removed with the egui engine —
//! the own-pass implementation (chartdx) has its own renderer and `data_signature`.

/// Current Unix time in milliseconds (the same scale as tick time_ms). Re-exports the canonical
/// source from `moon_core::util`; it historically lived here, and the path is retained for the
/// many `moon_chart::paint::now_unix_ms` references in the UI shell.
pub use moon_core::util::now_unix_ms;
