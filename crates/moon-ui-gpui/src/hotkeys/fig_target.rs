//! Which figure the `fig_delete` key and a plain left click are aimed at (#971).
//!
//! Pure rules, kept apart from GPUI so they can be pinned by unit tests. The key aims, in order, at
//! the figure under the cursor (the one the hover paints bright), then the entry order under the
//! cursor (the built-in cancel that shares the factory Delete), then the click-selected figure — so
//! a figure selected by an incidental click never steals the key from an order being aimed at.

/// The figure a `fig_delete` press deletes, tagged with how it was aimed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FigDeleteTarget<K> {
    /// The figure under the cursor on the hovered chart.
    Hovered(K),
    /// The click-selected figure (the one carrying knots).
    Selected(K),
}

/// Run a `fig_delete` press in its fixed priority order.
///
/// a. A figure under the cursor is deleted, and the entry cancel is never tried.
/// b. Else the entry cancel runs, exactly as the built-in Tab/Delete; if it took the press (an
///    order was under the cursor), that is the end.
/// c. Else the selected figure is deleted — only when `selected_in_reach`: `fig_selected` is
///    global, and a figure selected on another chart or window must not be deleted unseen.
/// d. Else nothing: the press is not consumed.
///
/// Args:
///     hovered: The figure under the cursor, if any.
///     selected: The click-selected figure, if any.
///     selected_in_reach: Whether the selection is on the hovered chart, or no chart is hovered.
///     cx: The context every branch acts through, handed to whichever one runs.
///     delete: Deletes the target figure.
///     cancel: The entry-order cancel; returns whether it consumed the press.
///
/// Returns:
///     Whether the press was consumed.
pub(crate) fn press_fig_delete<K, C: ?Sized>(
    hovered: Option<K>,
    selected: Option<K>,
    selected_in_reach: bool,
    cx: &mut C,
    delete: impl FnOnce(FigDeleteTarget<K>, &mut C),
    cancel: impl FnOnce(&mut C) -> bool,
) -> bool {
    if let Some(hovered) = hovered {
        delete(FigDeleteTarget::Hovered(hovered), cx);
        return true;
    }
    if cancel(cx) {
        return true;
    }
    match selected.filter(|_| selected_in_reach) {
        Some(selected) => {
            delete(FigDeleteTarget::Selected(selected), cx);
            true
        }
        None => false,
    }
}

/// The selection after a plain (unmodified) left click, or `None` when it stays as it is.
///
/// A click on a figure selects it, moving the knots off any older figure; a click on empty chart
/// space clears a selection made on this chart and leaves one made elsewhere alone.
///
/// Args:
///     hit: The figure under the click, if any.
///     selected: The current selection.
///     selected_here: Whether the current selection belongs to the clicked chart.
///
/// Returns:
///     `Some(new_selection)` when the selection changes, `None` when it does not.
pub(crate) fn plain_click_selection<K: PartialEq>(
    hit: Option<K>,
    selected: Option<&K>,
    selected_here: bool,
) -> Option<Option<K>> {
    match hit {
        Some(hit) if selected != Some(&hit) => Some(Some(hit)),
        Some(_) => None,
        None if selected.is_some() && selected_here => Some(None),
        None => None,
    }
}

#[cfg(test)]
mod tests;
