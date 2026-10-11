//! Layout hooks of the By coin mode: the debug selectors its layout probe measures.
//!
//! MoonUI buttons render as opaque elements that take no selector, so a control whose text
//! must never be clipped is wrapped in a [`tagged`] box the probe can find by name.

use gpui::*;

/// Wrap `el` in a content-sized box the layout probe finds as `sel`.
///
/// Args:
///     sel: Debug selector the probe looks up.
///     el: The control to wrap.
///
/// Returns:
///     A non-shrinking box holding `el`.
pub(in crate::analytics::tuner) fn tagged(sel: impl Into<String>, el: impl IntoElement) -> Div {
    let sel = sel.into();
    div().flex_none().debug_selector(move || sel).child(el)
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
