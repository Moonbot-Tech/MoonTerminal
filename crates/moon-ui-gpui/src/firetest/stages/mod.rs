//! One module per stage of the run.
//!
//! Each module owns its stage's logic — the request that drives the app, the verification that
//! reads the result back, and its log lines — as `impl Runtime` blocks, so the dispatcher in the
//! parent stays a readable list of phases. Every timing stays in the parent: the dispatcher is what
//! paces the run, and a duration split across modules is a duration nobody can weigh against its
//! neighbours.
//!
//! The modules are private: an `impl Runtime` block applies wherever it is written, so nothing
//! outside needs to name the module. `order_cancel` is the one exception — the run holds its
//! in-flight state as a `Runtime` field, so its type has to be nameable in the parent.

mod chart;
mod command_error;
mod locale;
pub(super) mod order_cancel;
mod perf;
mod price_scale;
mod root_overlay;
mod tool_windows;
