//! Checkboxes of the strategy tree: the per-strategy box and the bulk box on core and folder rows.
//!
//! The bulk box is a summary of the strategies the row covers: on only when every visible child
//! strategy currently shows checked, and off when the set is empty, mixed, or all unchecked.
//! Clicking it writes that opposite value into [`StrategiesView::staged`] for every covered
//! strategy. Start/Stop reads that map and the server flags, never a folder identity, so a
//! folder's box reaches no core as its own flag.
//!
//! The next click is a fresh bulk action over whatever the row covers then — coverage is the live
//! filter, not an undo of the last click, and the painted box is derived from the same set.
//!
//! Both boxes are built here so their tone, size and id convention cannot drift apart, and both
//! stage through [`stage_value`], the one rule deciding what a click leaves in `staged`.

use moon_ui::{MoonCheckbox, MoonCheckboxSize, MoonTone};

use super::super::logic::{
    strategy_core_is_visible, subtree_check_targets, subtree_displayed_all_checked,
};
use super::super::*;

#[cfg(test)]
mod tests;

/// Build a tree-row checkbox in the tree's shared style.
///
/// Green for on rather than the default Info tone, which produced a pale blue box indistinguishable
/// from empty on the light theme; Positive also makes the checkmark glyph green.
///
/// Args:
///     id: Element id, derived by the caller from the row's own stable node id.
///     checked: Value to display; both callers are controlled, so the widget stores nothing.
///
/// Returns:
///     The configured checkbox, still needing its `on_change`.
pub(super) fn row_checkbox(id: SharedString, checked: bool) -> MoonCheckbox {
    MoonCheckbox::new(id)
        .checked(checked)
        .tone(MoonTone::Positive)
        .size(MoonCheckboxSize::Compact)
}

/// Decide what one checkbox click leaves in [`StrategiesView::staged`] for a single strategy.
///
/// Args:
///     clicked: Value the click asks for.
///     server: Value the core last acknowledged.
///
/// Returns:
///     `Some(value)` to stage, or `None` to drop any staging — a value equal to the core's own flag
///     is not a change, and storing it would inflate the footer's change count with rows that ask
///     the core for nothing. A bulk click can produce a whole account of those at once.
pub(super) fn stage_value(clicked: bool, server: bool) -> Option<bool> {
    (clicked != server).then_some(clicked)
}

/// Render one core or folder row's bulk checkbox.
///
/// Args:
///     view: Strategies view flipped by the click.
///     row_id: The row's own tree id, reused so the widget keeps this node's identity.
///     core: Core owning the row.
///     path: Folder segments, empty for the core root.
///     checked: The row's current switch.
///
/// Returns:
///     The checkbox wrapped in the press-swallowing container the surrounding row needs.
pub(super) fn bulk_check(
    view: &Entity<StrategiesView>,
    row_id: &SharedString,
    core: CoreId,
    path: Vec<String>,
    checked: bool,
) -> AnyElement {
    let view = view.clone();
    div()
        // Matching the reserved slot below, so the control column keeps one width rule.
        .flex_none()
        // The row itself carries expand/collapse and the folder drag, and the vendored checkbox
        // stops neither: it only calls `prevent_default`. Both of those start from the row's own
        // `pending_mouse_down`, recorded on Bubble, and Bubble runs children first — so swallowing
        // the press here is what keeps one click on the box from also collapsing the folder under
        // it, or dragging that folder away when the press drifts a pixel.
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, app| {
            app.stop_propagation();
        })
        .child(
            row_checkbox(SharedString::from(format!("chk:{row_id}")), checked)
                // The reported value is ignored on purpose: the painted box is derived from child
                // strategy checkboxes, so the view recomputes the next bulk value itself and cannot
                // follow a stale widget value.
                .on_change(move |_: &bool, _window, app| {
                    view.update(app, |this, cx| {
                        this.toggle_row_check(core, &path, cx);
                    });
                }),
        )
        .into_any_element()
}

/// Reserve the checkbox column for a row that carries no bulk checkbox.
///
/// Without it that row's caption would sit one control to the left of every sibling at the same
/// depth, which reads as a different indentation level rather than as a missing control.
///
/// The reservation is a real checkbox made invisible rather than a width taken from a mirrored
/// metric: MoonUI resolves the Compact box against the active typography (`MoonCheckboxMetrics`
/// grows it with the font delta), so any number stated here would be right only at the shipped
/// font size. Hidden costs nothing beyond layout — GPUI returns before painting the subtree or
/// registering its mouse listeners (`div.rs`), so the reserved column has no hitbox of its own.
pub(super) fn bulk_check_slot(row_id: &SharedString) -> AnyElement {
    div()
        .flex_none()
        .invisible()
        .child(row_checkbox(
            SharedString::from(format!("chk:{row_id}")),
            false,
        ))
        .into_any_element()
}

impl StrategiesView {
    /// Bulk-stage every strategy a core or folder row covers to the opposite of its current summary.
    ///
    /// The painted box is derived from those strategies, so the click asks for check-all when the
    /// box is off and uncheck-all when it is on. Nested folder rows have no independent bit: the
    /// next paint derives them from the same overlay.
    ///
    /// A hidden Auto core is refused whole, exactly as the per-strategy checkbox refuses it: staging
    /// is left untouched rather than covering rows this workspace may not act on.
    ///
    /// Args:
    ///     core: Core owning the clicked row.
    ///     path: Folder segments, empty for the core root.
    ///     cx: View context used to read the store and publish the new staging.
    ///
    /// Returns:
    ///     Nothing; a core missing from the store leaves staging untouched.
    pub(super) fn toggle_row_check(
        &mut self,
        core: CoreId,
        path: &[String],
        cx: &mut Context<Self>,
    ) {
        if !strategy_core_is_visible(self.workspace_cores.as_deref(), core) {
            return;
        }
        // Prepared before the store borrow, from the live filter: the click acts on what the row
        // covers NOW. That is a narrower set than the row's caption counts, which ignores search
        // and active-only on purpose — see `subtree_check_targets`.
        let filter = self.filter.prepare();
        let targets = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else {
                return;
            };
            subtree_check_targets(&cd.strategies, path, &filter)
        };
        let checked = !subtree_displayed_all_checked(&targets, &self.staged, core);
        for (id, server_checked) in targets {
            self.stage_check((core, id), checked, server_checked);
        }
        cx.notify();
    }

    /// Apply [`stage_value`] to one strategy's retained staging.
    ///
    /// Args:
    ///     strategy: Core-qualified strategy identity.
    ///     clicked: Value the click asks for.
    ///     server: Value the core last acknowledged for that strategy.
    ///
    /// Returns:
    ///     Nothing; the entry is stored or dropped so `staged` holds only genuine differences.
    pub(super) fn stage_check(&mut self, strategy: Key, clicked: bool, server: bool) {
        match stage_value(clicked, server) {
            Some(value) => {
                self.staged.insert(strategy, value);
            }
            None => {
                self.staged.remove(&strategy);
            }
        }
    }
}
