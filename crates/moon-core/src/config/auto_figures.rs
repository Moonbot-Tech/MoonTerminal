//! Auto figures: levels, sloped lines and squeezes the chart finds by itself and draws with the
//! ordinary figure layer.
//!
//! Configured like the chart captions (`chart_labels`): a table of rows, one row per figure "by a
//! characteristic" — which detector, its parameters, the timeframe, how many lines to keep, and how
//! they look. Rows are not figures: nothing here is drawn, saved to `figures.json` or sent to a
//! core. The detectors themselves live in `moon_chart::auto_figures`; this module is the model they
//! read and the file it is stored in. Specification: `docs-internal/AUTO_FIGURES.md`.

mod params;

pub use params::*;
