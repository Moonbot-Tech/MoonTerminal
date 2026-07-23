//! Column width-budget clamp policy keeping the table's stored widths within the viewport.

use super::*;

/// Complete a partially persisted width map with defaults for every current column.
///
/// Partial maps from older sessions or renamed columns caused the table engine to rescale untouched
/// neighbors on every drag. Completing a non-empty map lets later observations recognize a
/// single-column drag once the previous snapshot has the same membership. Leave an empty map
/// untouched so automatic fill remains active; the first drag then snapshots all widths and may use
/// the proportional overflow path.
pub(super) fn complete_widths(
    widths: &mut std::collections::HashMap<String, f32>,
    cols: &[String],
) {
    if widths.is_empty() {
        return;
    }
    for c in cols {
        widths
            .entry(c.clone())
            .or_insert_with(|| columns::width_for(c));
    }
}

/// Minimum column width, mirroring the floor the fork enforces in
/// `MoonDataTableState::set_column_width` (`data_table.rs`). The fork does not export it,
/// so it is duplicated here; the clamp math must use the same floor, or its "does this
/// layout fit" test would disagree with the widths the engine actually stores.
const MIN_COL_W: f32 = 40.0;

/// Pure width-budget policy, split out of [`ReportPanel::clamp_table_widths`] so its
/// no-infinite-loop invariant can be unit-tested without a gpui `App`.
///
/// Returns the width map to write (and notify on), or `None` when nothing should change:
/// the columns already fit, no fitting solution exists, or the computed widths do not
/// actually move. The `None` on an infeasible layout is what stops the observe→notify
/// loop: with a 40px floor per column (enforced here and by the fork's `set_column_width`),
/// a viewport narrower than `40px × visible` can never be satisfied, so writing and
/// notifying anyway would re-arm the observer on every pass — CPU pegged, window frozen.
fn plan_clamp(
    cur: &std::collections::HashMap<String, f32>,
    visible: &[String],
    prev: &std::collections::HashMap<String, f32>,
    viewport: f32,
) -> Option<std::collections::HashMap<String, f32>> {
    if cur.is_empty() || viewport <= 80.0 {
        return None;
    }
    let width_of = |c: &str| cur.get(c).copied().unwrap_or_else(|| columns::width_for(c));
    let sum: f32 = visible.iter().map(|c| width_of(c)).sum();
    if sum - viewport <= 0.5 {
        return None; // already within budget
    }
    // No packing fits at the floor. Leave the stored widths intact and do NOT notify: the
    // fork already fits the DISPLAY itself at render time — `downscale_columns_to_available`
    // shrinks columns down to the floor, and its horizontal scrollbar owns whatever still
    // overflows — all without touching `column_widths`. So there is nothing to store, and
    // notifying on a pass that cannot reduce the overflow would spin observe→clamp→notify.
    if visible.len() as f32 * MIN_COL_W > viewport {
        return None;
    }
    let overflow = sum - viewport;
    // One changed column at an unchanged key set = a live single-column drag.
    let changed: Vec<&str> = cur
        .iter()
        .filter(|(k, v)| prev.get(*k).is_none_or(|p| (**v - p).abs() > 0.5))
        .map(|(k, _)| k.as_str())
        .collect();
    let single_drag = changed.len() == 1 && prev.len() == cur.len();
    let mut next = cur.clone();
    let mut moved = false;
    if single_drag {
        if let Some(w) = next.get_mut(changed[0]) {
            let nw = (*w - overflow).max(MIN_COL_W);
            if (nw - *w).abs() > 0.5 {
                *w = nw;
                moved = true;
            }
        }
    } else {
        let scale = viewport / sum;
        for col in visible {
            if let Some(w) = next.get_mut(col) {
                let nw = (*w * scale).max(MIN_COL_W);
                if (nw - *w).abs() > 0.5 {
                    *w = nw;
                    moved = true;
                }
            }
        }
    }
    // Notify only on real movement; a no-op pass must not re-arm the observer.
    moved.then_some(next)
}

impl ReportPanel {
    /// Reduce visible column widths toward the table viewport budget.
    ///
    /// The fork rescales columns at render to fit the viewport — `auto_width_columns` upscales to
    /// fill spare room, `downscale_columns_to_available` shrinks down to the floor — so neighbors
    /// "float" when one column is dragged; keeping the stored sum within the viewport stops that
    /// rescale from engaging. The decision is delegated to [`plan_clamp`], kept pure so
    /// its termination invariant is unit-testable. `None` means no write and no notify: this runs
    /// from an `observe` on the table state and its own `notify()` re-enters that observe, so a
    /// notify on a pass that made no progress (an infeasible `40px × visible > viewport` layout —
    /// e.g. every column shown in a narrow detached window) would spin `flush_effects` forever.
    pub(super) fn clamp_table_widths(&mut self, state: &Entity<MoonDataTableState>, cx: &mut App) {
        let (cur, viewport) = {
            let s = state.read(cx);
            (s.column_widths.clone(), s.viewport_width())
        };
        let prev = std::mem::replace(&mut self.last_widths, cur.clone());
        let visible: Vec<String> = self
            .cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .cloned()
            .collect();
        if let Some(next) = plan_clamp(&cur, &visible, &prev, viewport) {
            state.update(cx, |s, c| {
                s.column_widths = next;
                c.notify();
            });
            self.last_widths = state.read(cx).column_widths.clone();
        }
    }
}

#[cfg(test)]
mod tests;
