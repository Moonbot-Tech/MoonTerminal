//! The adapter every consumer of [`super::core_combo`] implements, and the wiring it gets for free.
//!
//! Six views host the shared core picker. What genuinely differs between them is one thing — the
//! work each owes after its selection changes (a requery, a reload, a cache rebuild). Everything
//! around that is identical, so the assembly of the picker's optional affordances lives here once.
//!
//! The capture rules are the reason this is a shared builder rather than six call sites: the
//! quick-action handler is stored in a menu row, which lives only as long as the frame that built
//! it, so a strong view handle is safe there. Anything MoonUI RETAINS across frames must capture
//! weakly instead, or it closes a `view -> popup state -> closure -> view` cycle and the view never
//! drops.

use gpui::{Context, Entity};

use super::core_combo::CoreComboExtras;

/// A view that hosts the shared core picker.
pub(crate) trait CoreComboHost: Sized + 'static {
    /// Select every core, then do this view's own post-selection work.
    ///
    /// The id list is the one the menu actually RENDERED, so the count previewed on the row and the
    /// selection produced by the click can never be computed against different lists.
    ///
    /// Args:
    ///     selectable: Every core the menu offered.
    ///     cx: The view's context.
    ///
    /// Returns:
    ///     Nothing; an inert action must reload nothing.
    fn select_all_cores(&mut self, selectable: Vec<u64>, cx: &mut Context<Self>);
}

/// Assemble the picker's optional affordances for one consumer.
///
/// `enabled` is false wherever an Auto workspace pins the selector: the control is disabled there,
/// so building a row nobody can click would be waste, and the branch belongs here rather than
/// repeated at every call site.
///
/// Args:
///     enabled: Whether this consumer's selector accepts input at all.
///     view: The consuming view.
///
/// Returns:
///     Extras to hand to [`super::core_combo`], or `None` for a pinned selector.
pub(crate) fn core_combo_extras<T: CoreComboHost>(
    enabled: bool,
    view: &Entity<T>,
) -> Option<CoreComboExtras> {
    if !enabled {
        return None;
    }
    let host = view.clone();
    Some(CoreComboExtras::new(move |selectable, _window, app| {
        host.update(app, |this, cx| this.select_all_cores(selectable, cx));
    }))
}
