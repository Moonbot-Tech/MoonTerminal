//! Per-pane chart data preparation in `ChartDataState`: the main non-drawing preparation reads
//! history, order books, and orders; uploads GPU buffers; applies automatic Y scaling; and tracks
//! change signatures. The implementation was extracted from `mod.rs`, where `ChartDataState`
//! remains declared.

use super::*;

// Responsibilities are split without logic changes: state handles lifecycle, signatures, and
// frames; orders synchronizes orders and line labels; market synchronizes history, order books,
// and automatic Y scaling.
mod market;
pub(crate) mod orders;
mod state;
