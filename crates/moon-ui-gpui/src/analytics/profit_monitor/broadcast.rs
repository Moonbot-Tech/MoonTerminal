//! The monitor's half of the terminal-wide core filter: publishing a row click, and repainting
//! from the value it published.
//!
//! Only the transport lives here. What one click makes of the current selection is
//! [`crate::controls::next_core_filter`], and what each panel makes of the result is
//! `crate::controls::apply_core_broadcast` — both pure, both tested without a window.

use gpui::*;
use moon_core::session::CoreId;

use super::ProfitMonitorView;
use crate::Backend;

/// Subscribe the monitor to the filter it publishes.
///
/// The monitor is the only publisher, but it reads the value back from the Backend rather than
/// keeping a copy, so the selected rows repaint from the one authority the panels also read. The
/// body is a cached sibling view: notifying the monitor alone would leave the old highlight on
/// screen.
///
/// Args:
///     backend: Shared terminal state owning the filter and its revision entity.
///     cx: Monitor context the subscription is attached to.
///
/// Returns:
///     Nothing; the subscription lives as long as the view.
pub(super) fn observe_core_filter(backend: &Entity<Backend>, cx: &mut Context<ProfitMonitorView>) {
    let revision = backend.read(cx).core_filter_revision();
    cx.observe(&revision, |this, _revision, cx| {
        this.invalidate_content(cx);
        cx.notify();
    })
    .detach();
}

impl ProfitMonitorView {
    /// Apply one row click to the terminal-wide core filter.
    ///
    /// The monitor holds no copy of the selection: it reads the current value from the Backend,
    /// resolves the click against it, and publishes the result. Everything visible afterwards — the
    /// selected rows here and the narrowed panels in the main window — comes from that one value
    /// through [`crate::CoreFilterRevision`], so the two cannot disagree.
    ///
    /// Args:
    ///     cores: Every core the clicked row stands for.
    ///     additive: Whether Ctrl (⌘ on macOS) asked to extend the selection instead of replacing.
    ///     cx: View context used to publish the new filter.
    ///
    /// Returns:
    ///     Nothing; the preference being off makes this a no-op, as does a click that resolves to
    ///     the selection already on air.
    pub(super) fn filter_to_cores(
        &mut self,
        cores: &[CoreId],
        additive: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.prefs.core_filter {
            return;
        }
        let next = {
            let backend = self.backend.read(cx);
            crate::controls::next_core_filter(backend.core_filter(), cores, additive)
        };
        self.backend.update(cx, |backend, backend_cx| {
            backend.set_core_filter(next, backend_cx)
        });
    }
}
