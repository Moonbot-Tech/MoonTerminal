//! One module per stage of the run.
//!
//! Each module exposes one free `fn` per row it owns in [`super::plan`] — the stage's tick rule,
//! answering `Stay` / `Next` / `Fail`. That function is the whole of what the dispatcher knows
//! about the stage; everything it calls stays an `impl Runtime` method beside it.
//!
//! What a stage does NOT own is when it runs. Its dwell, deadline, present mode and sampling rule
//! are cells in the table, not code here, so they can be read against every other stage's at once.

pub(super) mod chart;
pub(super) mod command_error;
pub(super) mod idle_floor;
pub(super) mod locale;
pub(super) mod order_cancel;
pub(super) mod perf;
pub(super) mod price_scale;
pub(super) mod root_overlay;
pub(super) mod tool_windows;
