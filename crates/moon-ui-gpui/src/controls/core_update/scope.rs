//! Which cores one click stands for -- the pure half of every bulk update this control performs.
//!
//! GPUI-free on purpose. This is the single most dangerous decision in the update path: the queue
//! it feeds reaches LIVE cores that trade real money, so "which cores did the user actually point
//! at" has to be a function that can be read, tested and mutated in isolation rather than a rule
//! spread across three click handlers.

use std::rc::Rc;

use moon_core::session::CoreId;

use crate::controls::row_selection::RowSelection;

/// Resolve the cores a context-menu click commands.
///
/// Windows Explorer semantics, and they are the whole point: a right-click on a row INSIDE the
/// current selection acts on the whole selection, and a right-click anywhere else acts on that row
/// alone. The click never MOVES the selection -- opening a menu must not change what it is about to
/// act on -- so this is the only place the two readings are reconciled.
///
/// The result is filtered through `order` rather than taken from the selection set directly, for
/// two reasons that both matter here: the set has no order of its own (it is a hash set), and it
/// can outlive a row leaving the view for one frame. Anything the user cannot currently see is not
/// something a menu may enqueue.
///
/// Args:
///     clicked: The core whose row was right-clicked.
///     order: Every selectable core currently rendered, in visual order.
///     selection: The panel's controlled row selection.
///
/// Returns:
///     The cores to command, in visual order. Never empty: a click outside the selection -- and a
///     click made with nothing selected -- yields the clicked core alone.
pub(crate) fn resolve_menu_scope(
    clicked: CoreId,
    order: &[CoreId],
    selection: &RowSelection<CoreId>,
) -> Rc<[CoreId]> {
    if !selection.contains(Some(clicked)) {
        return Rc::from(vec![clicked]);
    }
    let scope: Vec<CoreId> = order
        .iter()
        .copied()
        .filter(|core| selection.contains(Some(*core)))
        .collect();
    // The membership test above already proved `clicked` is selected, but it proves nothing about
    // `order`: a caller can hand a stale order that no longer draws it. Falling back to the clicked
    // core keeps the invariant this function is trusted for -- the result is never empty, so a menu
    // entry can never enqueue "everything" by resolving to nothing.
    if scope.is_empty() {
        return Rc::from(vec![clicked]);
    }
    Rc::from(scope)
}

#[cfg(test)]
mod tests;
