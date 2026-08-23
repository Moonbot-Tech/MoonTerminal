//! Register a WINDOW-level mouse listener from inside a `Render` implementation.
//!
//! `Window::on_mouse_event` is a paint-phase API — it pushes into the frame currently being
//! painted, and the fork asserts as much (`window.rs`, `Invalidator::debug_assert_paint`). A
//! `Render::render` body runs during request-layout, one phase earlier, so calling it there is a
//! misuse. It has never LOOKED like one, because this workspace builds with `debug-assertions =
//! false` and the assertion is compiled out; the listener still lands in the same frame's list and
//! everything works. The first build that turned assertions on for `moon-gpui` — the UI-atlas
//! capture profile — died on the first frame, in four places at once.
//!
//! The fix is not to stop using window-level listeners. They are load-bearing: the chart consumes
//! its own mouse events and calls `stop_propagation`, so a listener on the root element never sees
//! movement over the chart, and only a window-level CAPTURE-phase listener does. What changes is
//! WHEN the registration happens — a zero-sized `canvas` child whose paint closure runs in the
//! right phase, which is the idiomatic GPUI way to reach a paint-phase API from a view.

use gpui::{App, DispatchPhase, IntoElement, MouseEvent, Styled as _, Window, canvas};

/// A zero-sized element that installs one window-level mouse listener when it is painted.
///
/// Add it as a child anywhere in the tree; it draws nothing and occupies nothing. The listener is
/// registered once per frame, exactly as a call from `render` was, because `render` rebuilds this
/// element every frame too.
///
/// Args:
///     listener: The handler, receiving both dispatch phases like any window listener.
///
/// Returns:
///     An element to add as a child of the view that wants the listener.
pub(crate) fn window_mouse_hook<E: MouseEvent>(
    listener: impl FnMut(&E, DispatchPhase, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    canvas(
        |_bounds, _window, _cx| (),
        // FnOnce, so the listener can simply be moved through: a fresh canvas is built for every
        // frame, and each one hands its listener to the frame it is painted into.
        move |_bounds, (), window, _cx| window.on_mouse_event::<E>(listener),
    )
    // Absolute and zero-sized: the hook must not take part in the layout of whatever it is added
    // to. A canvas with no size set would still claim a flex slot.
    .absolute()
    .size_0()
}
