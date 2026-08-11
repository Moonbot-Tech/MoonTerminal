//! The adapter every consumer of [`super::core_combo`] implements, and the wiring it gets for free.
//!
//! Six views host the shared core picker. Its saved-group actions need exactly three
//! answers from each host: where the retained selection lives, whether an Auto workspace has
//! pinned it, and the work the view owes after it changes. This adapter applies those actions
//! against the three answers; the ordinary All, core, and exchange-row callbacks remain with each
//! consumer.
//!
//! View captures assembled here are always weak. Menu-row `on_click` handlers are stored in a
//! refcounted menu level, so a strong `Entity<T>` capture closes a
//! `view -> window state -> closure -> view` cycle and the view never drops. `Entity<Backend>` is
//! safe to retain because it is not the view; reading it at CLICK time also keeps a group row from
//! acting on a list edited since the menu opened.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{App, Context, Entity, WeakEntity};

use super::core_combo::CoreComboExtras;
use super::core_groups::{GroupClick, apply_core_group, configured_uids};
use crate::Backend;

/// A view that hosts the shared core picker.
pub(crate) trait CoreComboHost: Sized + 'static {
    /// Whether an Auto workspace owns this picker's scope, making every edit inert.
    ///
    /// The selector is also rendered disabled in that state; this is the second, load-bearing
    /// half — a disabled control must not be the only thing standing between a stray handler and
    /// the retained Classic selection it would silently rewrite. Call sites read the pin THROUGH
    /// this method rather than recomputing the predicate, so it has exactly one definition.
    fn core_selection_pinned(&self, cx: &App) -> bool;

    /// The retained selection this picker edits, for the pure decisions to rewrite in place;
    /// empty means all cores.
    fn core_selection_mut(&mut self) -> &mut HashSet<u64>;

    /// This view's mandatory work once the selection actually changed.
    ///
    /// A requery, a cache rebuild, a refresh — whatever this view owes. Not named
    /// `core_selection_changed`: Analytics already has an inherent method by that name, and an
    /// inherent method silently wins over a trait method of the same name at every call site,
    /// including inside the impl that meant to delegate to it.
    fn after_core_selection_change(&mut self, cx: &mut Context<Self>);
}

/// Apply one edit to a host's selection, honouring the pin and its post-change work.
///
/// Every picker action passes through this funnel, so no action can skip the pin guard or forget
/// the host's required reload.
///
/// Args:
///     view: Weak handle to the hosting view.
///     app: Application context.
///     edit: The pure decision, reporting whether it changed anything.
fn edit_selection<T: CoreComboHost>(
    view: &WeakEntity<T>,
    app: &mut App,
    edit: impl FnOnce(&mut HashSet<u64>) -> bool,
) {
    let _ = view.update(app, |this, cx| {
        if this.core_selection_pinned(cx) {
            return;
        }
        if edit(this.core_selection_mut()) {
            this.after_core_selection_change(cx);
        }
    });
}

/// Apply one click on a saved-group row.
///
/// The group is resolved at CLICK time and BY NAME, never by the position the menu was built with.
/// The management modal — possibly in another window, since the saved list is application state
/// while a dialog is window-scoped — can have reordered or deleted a group since then, and a
/// retained index would silently apply a different one. Names are unique case-insensitively
/// (`sanitize_core_groups`), so they are the only stable identity a group has; a group renamed or
/// deleted under an open menu simply no longer resolves, and the click is inert.
///
/// Args:
///     view: Weak handle to the hosting view.
///     backend: Shared terminal state holding the saved groups.
///     name: The clicked group's name.
///     secondary: Whether the click carried the platform's additive modifier.
///     selectable: Every core the menu offered.
///     app: Application context.
fn apply_group_click<T: CoreComboHost>(
    view: &WeakEntity<T>,
    backend: &Entity<Backend>,
    name: &str,
    secondary: bool,
    selectable: &[u64],
    app: &mut App,
) {
    let Some(members) = backend
        .read(app)
        .config
        .core_groups
        .iter()
        .find(|group| group.name == name)
        .map(|group| group.cores.clone())
    else {
        return;
    };
    let intent = GroupClick::from_secondary(secondary);
    edit_selection(view, app, |selected| {
        apply_core_group(selected, &members, selectable, intent)
    });
}

/// Assemble the picker's affordances for one consumer.
///
/// `enabled` is false wherever an Auto workspace pins the selector: the control is disabled there,
/// so building rows nobody can click would be waste, and the branch belongs here rather than
/// repeated at every call site.
///
/// The returned value BORROWS the saved groups out of `backend` for the rest of the render. Every
/// call site hands it straight to [`super::core_combo`] inside the same expression, so the borrow
/// never outlives the frame — and the picker stops cloning the whole group list per repaint.
///
/// Args:
///     enabled: Whether this consumer's selector accepts input at all.
///     view: The consuming view.
///     backend: Shared terminal state, holding the saved groups and the configured cores.
///     cx: Read context for the backend.
///
/// Returns:
///     Extras to hand to [`super::core_combo`], or `None` for a pinned selector.
pub(crate) fn core_combo_extras<'a, T: CoreComboHost>(
    enabled: bool,
    view: &Entity<T>,
    backend: &'a Entity<Backend>,
    cx: &'a App,
) -> Option<CoreComboExtras<'a>> {
    if !enabled {
        return None;
    }
    let config = &backend.read(cx).config;
    let group_view = view.downgrade();
    let group_backend = backend.clone();
    let save_backend = backend.clone();
    let manage_backend = backend.clone();

    Some(CoreComboExtras::new(
        &config.core_groups,
        configured_uids(config),
        Rc::new(move |name, secondary, selectable, _window, app| {
            apply_group_click(
                &group_view,
                &group_backend,
                &name,
                secondary,
                &selectable,
                app,
            );
        }),
        Rc::new(move |cores, window, app| {
            super::core_group_dialogs::open_save_dialog(save_backend.clone(), cores, window, app);
        }),
        Rc::new(move |window, app| {
            super::core_group_dialogs::open_manage_dialog(manage_backend.clone(), window, app);
        }),
    ))
}
