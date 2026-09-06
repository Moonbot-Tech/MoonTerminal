//! Controlled multi-row selection: the click algorithm every table-like panel shares.
//!
//! LIFTED from `panels/report/selection.rs`, which ran it first over `MoonDataTable`'s
//! `controlled_row_selection` mode; Core Status now runs the same gestures over two presentations
//! at once. The algorithm is deliberately unchanged by the lift — the Report panel's own tests are
//! the oracle it kept passing.
//!
//! Keyed by a STABLE ROW IDENTITY, never by a line index. Both callers need that for the same
//! reason: the list is virtual, it re-sorts, and it draws synthetic rows a selection must never
//! address (Report's malformed legacy rows, Core Status' exchange headings). That is why every
//! entry point speaks `Option<K>` — `None` is "this line is not a selectable row", and it is a
//! no-op rather than an error.

use std::collections::HashSet;
use std::hash::Hash;

/// Controlled multi-selection with one stable Shift-range anchor.
pub(crate) struct RowSelection<K> {
    selected: HashSet<K>,
    anchor: Option<K>,
    /// Row of the LAST click in any mode, which a detail pane describes.
    ///
    /// Distinct from `anchor`: a Shift range deliberately keeps its anchor at the range base so the
    /// next Shift click re-measures from there, but the row the user just pointed at is the far end.
    last_clicked: Option<K>,
}

// Written out rather than derived: `#[derive(Default)]` would demand `K: Default`, and a row
// identity has no meaningful default. An empty selection is well defined for every key type.
impl<K> Default for RowSelection<K> {
    fn default() -> Self {
        Self {
            selected: HashSet::new(),
            anchor: None,
            last_clicked: None,
        }
    }
}

impl<K: Clone> Clone for RowSelection<K> {
    fn clone(&self) -> Self {
        Self {
            selected: self.selected.clone(),
            anchor: self.anchor.clone(),
            last_clicked: self.last_clicked.clone(),
        }
    }
}

impl<K: Copy + Eq + Hash> RowSelection<K> {
    /// Select every valid row identity in the current list without changing the Shift anchor.
    ///
    /// Args:
    ///     order: Current rendered row identities in visual order, `None` for a non-row line.
    ///
    /// Returns:
    ///     Nothing. Lines without a stable identity are excluded.
    pub(crate) fn select_all(&mut self, order: &[Option<K>]) {
        self.selected = order.iter().filter_map(|key| *key).collect();
    }

    /// Apply one row click using platform-independent modifier meaning.
    ///
    /// Args:
    ///     clicked: Stable identity of the clicked row, or `None` for a line that is not a row.
    ///     order: Current rendered row identities in visual order.
    ///     shift: Whether Shift was held.
    ///     secondary: Whether Ctrl on Windows/Linux or Command on macOS was held.
    ///
    /// Returns:
    ///     Nothing. Shift takes precedence over the secondary modifier, and a plain click that
    ///     lands on the sole selected row clears the selection instead of re-selecting it.
    pub(crate) fn click(
        &mut self,
        clicked: Option<K>,
        order: &[Option<K>],
        shift: bool,
        secondary: bool,
    ) {
        let Some(clicked) = clicked else {
            return;
        };
        self.last_clicked = Some(clicked);
        if shift {
            let span = self.anchor.and_then(|anchor| {
                let from = order.iter().position(|key| *key == Some(anchor))?;
                let to = order.iter().position(|key| *key == Some(clicked))?;
                Some(if from <= to { from..=to } else { to..=from })
            });
            self.selected.clear();
            if let Some(span) = span {
                self.selected
                    .extend(order[span].iter().filter_map(|key| *key));
            } else {
                self.selected.insert(clicked);
                self.anchor = Some(clicked);
            }
            return;
        }
        self.anchor = Some(clicked);
        if secondary {
            if !self.selected.insert(clicked) {
                self.selected.remove(&clicked);
            }
            return;
        }
        // A plain click on the row that IS the entire selection clears it: clicking the same row
        // twice reads as undoing that selection. With anything else selected the click still
        // collapses the set to the clicked row — that is the standard table behaviour and the only
        // way back from a Shift range to a single row. The anchor is NOT cleared with the set — it
        // was just moved to this row above — so a following Shift click still measures from here.
        let only_this = self.selected.len() == 1 && self.selected.contains(&clicked);
        self.selected.clear();
        if !only_this {
            self.selected.insert(clicked);
        }
    }

    /// Select exactly one row, whatever was selected before.
    ///
    /// Unlike a plain [`Self::click`], this never clears: it exists for the second half of a
    /// physical double-click. MoonDataTable invokes the row-select callback on BOTH clicks and
    /// gives it no click count, so the deselecting second click has to be undone from the table's
    /// own authoritative double-click callback rather than guessed at from timing.
    ///
    /// Args:
    ///     clicked: Stable identity of the double-clicked row, or `None` for a non-row line.
    ///
    /// Returns:
    ///     Nothing. The anchor follows the row, as it does for a plain click.
    pub(crate) fn select_only(&mut self, clicked: Option<K>) {
        let Some(clicked) = clicked else {
            return;
        };
        self.anchor = Some(clicked);
        self.last_clicked = Some(clicked);
        self.selected.clear();
        self.selected.insert(clicked);
    }

    /// Remove selections no longer present in a newly published list.
    ///
    /// The one call that keeps a selection HONEST: a row the user can no longer see must not stay
    /// in a set that later acts on it. Every owner calls this the moment its rows are rebuilt.
    ///
    /// Args:
    ///     visible: Stable row identities in the new list.
    ///
    /// Returns:
    ///     Nothing. A missing anchor is cleared with its vanished row.
    pub(crate) fn retain_visible(&mut self, visible: &[Option<K>]) {
        let visible: HashSet<K> = visible.iter().filter_map(|key| *key).collect();
        self.selected.retain(|key| visible.contains(key));
        if self.anchor.is_some_and(|key| !visible.contains(&key)) {
            self.anchor = None;
        }
        if self.last_clicked.is_some_and(|key| !visible.contains(&key)) {
            self.last_clicked = None;
        }
    }

    /// Clear every selected row and the Shift anchor.
    ///
    /// Returns:
    ///     Nothing after selection state becomes empty.
    pub(crate) fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.last_clicked = None;
    }

    /// Return whether one stable row is selected.
    ///
    /// Args:
    ///     key: Stable row identity, or `None` for an unselectable line.
    ///
    /// Returns:
    ///     `true` only when a concrete identity belongs to the controlled set.
    pub(crate) fn contains(&self, key: Option<K>) -> bool {
        key.is_some_and(|key| self.selected.contains(&key))
    }

    /// Return the row the user last clicked, while it is still selected.
    ///
    /// Returns:
    ///     The last-clicked identity, or `None` once it has been deselected or has left the list.
    ///     The membership check matters for Ctrl-click: it clears the row but keeps it as the
    ///     anchor for a following Shift range.
    pub(crate) fn current(&self) -> Option<K> {
        self.last_clicked.filter(|key| self.selected.contains(key))
    }

    /// Return the number of selected rows.
    ///
    /// Returns:
    ///     Current controlled selection size.
    pub(crate) fn len(&self) -> usize {
        self.selected.len()
    }

    /// Walk the selected identities in the set's own arbitrary order.
    ///
    /// Deliberately NOT the order to act in: a caller that commands the selection resolves it
    /// against the rendered row order instead, so what it does matches what the user sees. This
    /// exists for membership folds and per-key lookups that do not care about order.
    ///
    /// Returns:
    ///     An iterator over the selected identities.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &K> {
        self.selected.iter()
    }
}

#[cfg(test)]
mod tests;
