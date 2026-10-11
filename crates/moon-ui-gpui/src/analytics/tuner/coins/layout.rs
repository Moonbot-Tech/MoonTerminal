//! Layout hooks of the By coin mode: the debug selectors its layout probe measures.
//!
//! MoonUI buttons render as opaque elements that take no selector, so a control whose text
//! must never be clipped is wrapped in a [`tagged`] box the probe can find by name.

pub(in crate::analytics::tuner) use super::super::shared::tagged;

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
