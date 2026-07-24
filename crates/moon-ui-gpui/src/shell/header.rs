//! Shell-owned state for interactive controls rendered in the terminal header.

use gpui::Context;

use super::Shell;

impl Shell {
    /// Open or close the active-core selector without coupling dismissal to every content click.
    ///
    /// Args:
    ///     open: Whether the selector popover should remain visible.
    ///     cx: Shell context used to request a repaint after state changes.
    ///
    /// Returns:
    ///     Nothing; the controlled popover state is updated in place.
    pub(crate) fn set_header_core_selector_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.header_core_selector_open == open {
            return;
        }
        self.header_core_selector_open = open;
        cx.notify();
    }
}
