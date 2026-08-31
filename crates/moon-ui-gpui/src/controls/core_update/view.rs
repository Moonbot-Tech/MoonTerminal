//! The rendered update control: a single hover-revealed micro button, drawn inside whatever
//! version cell hosts it.
//!
//! Hover-revealing is the design decision that makes a per-core confirm unnecessary -- the press
//! is explicit rather than an ambient cell click -- and it sidesteps the "a clickable cell
//! swallows the row's double-click" trap entirely: the button is its own element and the cell
//! around it never becomes clickable. There is deliberately no confirm here.
//!
//! The reveal itself rides GPUI's own group-hover styling rather than any state this crate owns:
//! the hosting cell carries `.group(name)`, this button carries `.group_hover(name, ...)`, and the
//! button keeps its reserved footprint at zero opacity the rest of the time -- so appearing never
//! shifts a neighbouring column, the same "reserve the geometry, vary the content" rule
//! `core_run::view` follows for its own slots.

use std::rc::Rc;

use gpui::*;
use moon_core::feed::UpdateTarget;
use moon_core::session::CoreId;
use moon_ui::{MoonButtonIconSlot, MoonButtonVariant, MoonPalette};
use rust_i18n::t;

use super::{OfferCounts, SLOT_W, retry_core, update_core, update_scope};
use crate::Backend;
use crate::design;

/// Build the hover-revealed update control for one version cell.
///
/// Args:
///     id: Stable element identity for the button itself, unique among the controls drawn in one
///         frame -- a core uid for a row, the server's own tree id for a group.
///     group: The hover-reveal group name shared with the hosting cell (see the module doc);
///         derived from the same identity as `id`, never a literal shared across rows.
///     cores: Every core this control commands; one entry for a core row, the whole group for a
///         server row.
///     counts: This scope's cores, already classified by [`super::offer_state`].
///     failed_retry: The single core to retry, when this scope stands for exactly one core and
///         its last attempt is `Done(Failed(_))`. `None` for every other case, including a group.
///     backend: Shared terminal state the button commands.
///     palette: Active MoonUI palette.
///     cx: Application context used to scale geometry.
///
/// Returns:
///     The control, or `None` when this scope currently offers nothing to draw -- the queue would
///     accept none of its cores and there is no failed core to retry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_button(
    id: impl Into<ElementId>,
    group: SharedString,
    cores: Rc<[CoreId]>,
    counts: OfferCounts,
    failed_retry: Option<CoreId>,
    backend: &Entity<Backend>,
    palette: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    let id: ElementId = id.into();
    if let Some(core) = failed_retry {
        return Some(retry_slot(id, group, core, backend, palette, cx));
    }
    if counts.offerable == 0 {
        return None;
    }
    Some(enqueue_slot(id, group, cores, counts, backend, palette, cx))
}

/// Build the "start an update" flavour of the control.
fn enqueue_slot(
    id: ElementId,
    group: SharedString,
    cores: Rc<[CoreId]>,
    counts: OfferCounts,
    backend: &Entity<Backend>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let tip = enqueue_tooltip(counts, cores.len());
    // WEAK, taken once per drawn button: `Entity::clone` takes the process entity map's lock, and
    // this runs inside a virtual-list item builder. A click can also outlive the window it was
    // made in -- same reasoning as `core_run::view`'s buttons.
    let target = backend.downgrade();
    let icon = MoonButtonIconSlot::new("icons/arrow-up.svg").color(palette.text_soft);
    hover_wrap(
        group,
        crate::panels::micro_icon_button(
            id,
            icon,
            tip,
            MoonButtonVariant::Ghost,
            design::ui_value(cx, SLOT_W),
            move |_window, app| {
                app.stop_propagation();
                let Some(backend) = target.upgrade() else {
                    return;
                };
                // A row control commands its one core through `update_core`; a server (or wider)
                // control fills the queue through `update_scope` -- the two shared entry points,
                // dispatched here by scope size rather than duplicated per caller.
                if cores.len() == 1 {
                    update_core(&backend, cores[0], UpdateTarget::Release, app);
                } else {
                    update_scope(&backend, &cores, UpdateTarget::Release, app);
                }
            },
        ),
        cx,
    )
}

/// Build the "retry the failed attempt" flavour of the control.
fn retry_slot(
    id: ElementId,
    group: SharedString,
    core: CoreId,
    backend: &Entity<Backend>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let tip = t!("core_update.retry").to_string();
    let target = backend.downgrade();
    let icon = MoonButtonIconSlot::new("icons/redo-2.svg").color(palette.amber);
    hover_wrap(
        group,
        crate::panels::micro_icon_button(
            id,
            icon,
            tip,
            MoonButtonVariant::Ghost,
            design::ui_value(cx, SLOT_W),
            move |_window, app| {
                app.stop_propagation();
                if let Some(backend) = target.upgrade() {
                    retry_core(&backend, core, app);
                }
            },
        ),
        cx,
    )
}

/// Tooltip for the enqueue flavour: a plain instruction for a single core, a count for a scope of
/// many plus what the press would skip -- there is no confirm on this control, so the tooltip is
/// the only thing that tells the user what a bulk press actually reaches.
///
/// Args:
///     counts: This scope's cores, already classified.
///     scope_len: Number of cores this control commands.
///
/// Returns:
///     Localized tooltip text.
fn enqueue_tooltip(counts: OfferCounts, scope_len: usize) -> String {
    if scope_len <= 1 {
        return t!("core_update.row.action").to_string();
    }
    let mut tip = t!("core_update.server.action", n = counts.offerable).to_string();
    if counts.offline > 0 {
        tip = format!(
            "{tip} \u{2014} {}",
            t!("core_update.skipped_offline", n = counts.offline)
        );
    }
    if counts.tracked > 0 {
        tip = format!(
            "{tip} \u{2014} {}",
            t!("core_update.skipped_already", n = counts.tracked)
        );
    }
    tip
}

/// Wrap a button in its reserved, hover-revealed slot.
///
/// Args:
///     group: The hover-reveal group name, matching the `.group(...)` the hosting cell carries.
///     button: The rendered button.
///     cx: Application context used to scale the wrapper.
///
/// Returns:
///     The guarded, hover-revealed slot.
fn hover_wrap(group: SharedString, button: impl IntoElement, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .w(design::ui_px(cx, SLOT_W))
        .opacity(0.0)
        .group_hover(group, |s| s.opacity(1.0))
        // Guarded like every other micro button hosted in a clickable row: the pixel under the
        // pointer must not let a press fall through to the row's own click.
        .on_mouse_down(MouseButton::Left, |_, _, app| app.stop_propagation())
        .child(button)
        .into_any_element()
}
