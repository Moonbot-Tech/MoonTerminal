//! Checkboxes of the strategy tree: the per-strategy box and the bulk box on core and folder rows.
//!
//! The bulk switch is its own state, not a summary of the strategies below it — Moonbot keeps the
//! two independent, so a folder reads unchecked while everything inside it is checked, and
//! Start/Stop pays no attention to folders at all. Only the side effect crosses over: flipping the
//! switch writes the same value into [`StrategiesView::staged`] for the rows it covers. Start/Stop
//! reads that map and the server flags, never this one, so a folder's switch reaches no core.
//!
//! It also keeps no memory of what a previous click covered. That is what makes a stale switch
//! harmless — the next click is a fresh bulk action over whatever the row covers then, not an undo
//! of the last one.
//!
//! Both boxes are built here so their tone, size and id convention cannot drift apart, and both
//! stage through [`stage_value`], the one rule deciding what a click leaves in `staged`.

use moon_ui::{MoonCheckbox, MoonCheckboxSize, MoonTone};

use super::super::logic::{
    strategy_core_is_visible, subtree_check_targets, subtree_folder_paths, toggle,
};
use super::super::*;
use super::ops;

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
                // The reported value is ignored on purpose: this switch is derived from nothing the
                // widget knows, so the view flips its own retained state and cannot follow a stale
                // widget value.
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
        .child(row_checkbox(SharedString::from(format!("chk:{row_id}")), false))
        .into_any_element()
}

impl StrategiesView {
    /// Flip one core or folder row's bulk checkbox and carry everything it covers with it.
    ///
    /// The click reaches two kinds of row: the strategies below it, which are staged, and the
    /// nested FOLDER rows, whose own switches follow the clicked one. Without the second half a
    /// checked folder would sit above unchecked subfolders whose strategies it had just staged.
    ///
    /// A hidden Auto core is refused whole, exactly as the per-strategy checkbox refuses it: the
    /// switch stays where it was rather than staging rows this workspace may not act on.
    ///
    /// Args:
    ///     core: Core owning the clicked row.
    ///     path: Folder segments, empty for the core root.
    ///     cx: View context used to read the store and publish the new staging.
    ///
    /// Returns:
    ///     Nothing; a core missing from the store leaves both the switch and staging untouched.
    pub(super) fn toggle_row_check(
        &mut self,
        core: CoreId,
        path: &[String],
        cx: &mut Context<Self>,
    ) {
        if !strategy_core_is_visible(self.workspace_cores.as_deref(), core) {
            return;
        }
        let key = (core, ops::join_path(path));
        let checked = !self.folder_checks.contains(&key);
        // Prepared before the store borrow, from the live filter: the click acts on what the row
        // covers NOW. That is a narrower set than the row's caption counts, which ignores search
        // and active-only on purpose — see `subtree_check_targets`.
        let filter = self.filter.prepare();
        let (targets, mut folders) = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else {
                return;
            };
            (
                subtree_check_targets(&cd.strategies, path, &filter),
                subtree_folder_paths(&cd.strategies, path, &filter),
            )
        };
        // Empty UI-only folders draw a row too, and no strategy path can name them.
        folders.extend(
            self.ui_folders
                .iter()
                .filter(|(c, p)| {
                    *c == core && p.as_str() != key.1 && ops::path_starts_with(p, path)
                })
                .map(|(_, p)| p.clone()),
        );
        toggle(&mut self.folder_checks, key);
        for folder in folders {
            if checked {
                self.folder_checks.insert((core, folder));
            } else {
                self.folder_checks.remove(&(core, folder));
            }
        }
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

    /// Move one folder's bulk-check switches with it, or drop them when it is deleted.
    ///
    /// Called beside the UI-folder maintenance for the same reason it exists: deleting or renaming
    /// an EMPTY folder moves no strategy, so no snapshot arrives and
    /// [`Self::reconcile_row_checks`] never runs. Without this the switch would outlive the folder
    /// and a folder recreated under the old name would come back already ticked.
    ///
    /// Args:
    ///     core: Core owning the folder.
    ///     old_path: Folder segments being renamed or deleted.
    ///     new_path: Replacement segments, or `None` for a deletion.
    ///
    /// Returns:
    ///     Nothing; switches below the folder follow it, and every other core is untouched.
    pub(super) fn move_row_checks(
        &mut self,
        core: CoreId,
        old_path: &[String],
        new_path: Option<&[String]>,
    ) {
        if old_path.is_empty() {
            return;
        }
        let moved: Vec<String> = self
            .folder_checks
            .iter()
            .filter(|(c, path)| *c == core && ops::path_starts_with(path, old_path))
            .map(|(_, path)| path.clone())
            .collect();
        let old_key = ops::join_path(old_path);
        for path in moved {
            self.folder_checks.remove(&(core, path.clone()));
            if let Some(new_path) = new_path {
                // `replacen` on the joined key, matching how `rename_ui_folder` rebases its own
                // paths; membership was already decided segment by segment above.
                let rebased = path.replacen(&old_key, &ops::join_path(new_path), 1);
                self.folder_checks.insert((core, rebased));
            }
        }
    }

}
