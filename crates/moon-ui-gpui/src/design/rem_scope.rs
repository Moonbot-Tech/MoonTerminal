//! A subtree drawn at its own rem size.
//!
//! MoonUI's stock widgets (the sidebar, its menu items and group headings, the small input) size
//! their text, padding, icons and row height in rems, and the window's rem is the theme's base font
//! size. [`RemScope`] hands one subtree a different rem, so such a widget scales as a whole —
//! text, icon and height in proportion — without touching MoonUI.

use gpui::*;

/// `child` laid out, prepainted and painted with the rem size `rem`.
///
/// Rem lengths are resolved while an element requests its layout (styles and text measures alike),
/// so overriding the rem around all three phases is enough for the whole subtree.
pub struct RemScope {
    rem: Pixels,
    child: AnyElement,
}

/// Wrap `child` so every rem length inside it resolves against `rem`.
///
/// Args:
///     rem: The rem size the subtree draws at.
///     child: The subtree.
///
/// Returns:
///     The wrapping element.
pub fn rem_scope(rem: Pixels, child: impl IntoElement) -> RemScope {
    RemScope {
        rem,
        child: child.into_any_element(),
    }
}

impl IntoElement for RemScope {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RemScope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child = &mut self.child;
        let id = window.with_rem_size(Some(self.rem), |window| child.request_layout(window, cx));
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_rem_size(Some(self.rem), |window| {
            child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_rem_size(Some(self.rem), |window| child.paint(window, cx));
    }
}
