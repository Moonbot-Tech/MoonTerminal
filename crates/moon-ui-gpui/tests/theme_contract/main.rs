//! Static UI contract for `moon-ui-gpui`.
//!
//! The crate is a BINARY with no `[lib]`, so an integration test cannot import anything from
//! it and can only execute the built exe. These invariants are therefore checked by reading
//! the sources as text — a workaround for that limitation, not a style choice.
//!
//! ONE test target — `theme_contract`, the name every reference to these invariants uses —
//! split by SUBJECT across the modules below. The directory form is what makes that possible:
//! cargo takes `tests/<dir>/main.rs` as a single integration test, while a loose `tests/*.rs`
//! would become a target of its own and lose the shared helpers.
//!
//! Add a new invariant to the module that owns its surface, and reach for [`support`] rather
//! than re-reading a source file by hand.

mod support;

mod analytics;
mod chart;
mod shell;
mod strategies;
mod theme;
mod tuner;
mod windowing;
