//! Singleton-window launchers shared by the header and the trading toolbar.
//!
//! The header carries Strategies, Analytics and Settings; the toolbar keeps Profit Monitor and
//! Screener plus the «⋯» overflow they fold into. One description per launcher
//! ([`LaunchTarget`]) feeds both the drawn button and the overflow row, so the two places cannot
//! open different things.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonContextMenuWindowExt as _, MoonMenuItem,
    MoonPalette, MoonWindowExt as _,
};
use rust_i18n::t;

use crate::{Backend, design};

#[cfg(test)]
mod tests;

/// Font weight used by a launcher's localized text segment.
pub(crate) const LAUNCHER_TEXT_WEIGHT: f32 = 500.0;
/// Two raw one-pixel borders enclosing a labeled Soft button's horizontal content.
pub(crate) const LAUNCHER_BORDER_W: f32 = 2.0;
/// Horizontal inset on each side of a labeled launcher. Action/Sm ship with `pad_x = 0` so
/// icon-only targets stay square; labeled buttons opt into the same 7-unit inset used by other
/// Action labels (`core_settings_popup`, connections tab).
pub(crate) const LAUNCHER_PAD_X: f32 = 7.0;

/// Singleton-window entry point shared by every launcher.
pub(crate) type OpenWindow =
    fn(Entity<Backend>, Option<AnyWindowHandle>, Option<DisplayId>, &mut App);

/// Everything one launcher needs, whether it is drawn on a row or listed in the overflow menu.
pub(crate) struct LaunchTarget {
    /// Stable button element identity; doubles as the overflow menu row key.
    pub id: &'static str,
    /// Localized launcher name: visible label, icon tooltip or menu row text.
    pub label: String,
    /// MoonUI asset path for the launcher glyph.
    pub icon: &'static str,
    /// Fixed labeled width, or `None` for an icon-only button.
    pub labeled_width: Option<f32>,
    /// Group to record before opening a workspace-scoped singleton.
    pub workspace_owner: Option<String>,
    /// Singleton-window entry point.
    pub open: OpenWindow,
}

/// Measure one complete localized launcher button at control-tier geometry.
///
/// The label is measured AND rendered in [`design::ui_font`]: it is a control caption, and every
/// host of a labeled launcher sets that family so `MoonButton`'s text segment inherits it (the
/// segment pins a family only when its own `mono` flag is set). Measuring in the other family
/// would budget the row wrongly. The reserved width is the leading icon plus its control-tier gap
/// ([`design::action_icon_reservation`]), both insets, and the two border pixels. The button is
/// never narrower than [`design::glyph_btn_w`].
///
/// Args:
///     cx: Application context supplying active font and UI scales.
///     label: Localized launcher label rendered by the button.
///
/// Returns:
///     Full icon-plus-label width in logical pixels.
pub(crate) fn launcher_label_width(cx: &App, label: &str) -> f32 {
    labeled_button_width(cx, label, LAUNCHER_TEXT_WEIGHT, true)
}

/// Rendered width of a labelled chrome button at the launcher insets: its UI-face label at
/// `weight`, the leading glyph and its gap when `icon`, both insets and the two border pixels —
/// never narrower than a square glyph button. The one measurement every labelled header button
/// (launchers, the updater, the quiet-mode button) is budgeted with.
pub(crate) fn labeled_button_width(cx: &App, label: &str, weight: f32, icon: bool) -> f32 {
    let text = design::ui_text_width_zoomed(cx, label, design::BODY_TEXT, weight, false);
    let chrome = if icon {
        design::action_icon_reservation(cx)
    } else {
        0.0
    } + design::ui_value(cx, LAUNCHER_PAD_X) * 2.0
        + LAUNCHER_BORDER_W;
    (text + chrome).max(design::glyph_btn_w(cx))
}

/// Open one launcher's singleton window from `window`, exactly as its button does.
pub(crate) fn launch_window(
    workspace_owner: Option<&str>,
    open: OpenWindow,
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(group) = workspace_owner {
        backend.update(cx, |backend, backend_cx| {
            backend.focus_singleton_owner(group, backend_cx);
        });
    }
    let owner_display = window.display(cx).map(|d| d.id());
    open(
        backend.clone(),
        Some(window.window_handle()),
        owner_display,
        cx,
    );
}

/// A chrome button that opens a singleton window: a flat (ghost) icon+label, tinted on hover.
///
/// `labeled_width = None` renders the icon alone with its name as a tooltip; `Some(w)` renders
/// icon + label inside the fixed width `w`, and the tooltip still names it. With no `backend` the
/// button keeps its geometry but opens nothing (layout tests render the header without a backend).
///
/// Args:
///     target: The launcher's identity, label, glyph, width and destination.
///     backend: Shared terminal state passed to the destination; `None` renders it inert.
///     p: Active palette used for icon and text colors.
///     cx: Application context used to resolve the control-tier icon-only width.
///
/// Returns:
///     One rendered compact launcher button.
pub(crate) fn open_window_button(
    target: LaunchTarget,
    backend: Option<Entity<Backend>>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let LaunchTarget {
        id,
        label,
        icon,
        labeled_width,
        workspace_owner,
        open,
    } = target;
    let mut btn = MoonButton::new(id)
        .width(labeled_width.unwrap_or_else(|| design::glyph_btn_w(cx)))
        // Borderless: the header's actions are flat, as in the approved mockup.
        .variant(MoonButtonVariant::Ghost)
        .leading_icon(MoonButtonIconSlot::new(icon).color(p.text_soft));
    btn = if labeled_width.is_some() {
        btn.padding_x(LAUNCHER_PAD_X)
            .text_segment(label.clone(), p.text_soft, LAUNCHER_TEXT_WEIGHT)
            .tooltip(label)
    } else {
        btn.tooltip(label)
    };
    if let Some(backend) = backend {
        btn = btn.on_click(move |_, window, cx| {
            launch_window(workspace_owner.as_deref(), open, &backend, window, cx);
        });
    }
    div()
        .flex_none()
        .debug_selector(|| id.to_string())
        .child(btn.render())
        .into_any_element()
}

/// Width bounds of the overflow menu, in design pixels; the menu fits its rows inside them.
const OVERFLOW_MENU_MIN_W: f32 = 120.0;
/// Upper width bound of the overflow menu, in design pixels.
const OVERFLOW_MENU_MAX_W: f32 = 280.0;

/// One overflow-menu row standing for a control the row folded in: closes the menu, then acts.
pub(crate) fn overflow_row(
    key: impl Into<SharedString>,
    label: impl Into<SharedString>,
    act: impl Fn(&mut Window, &mut App) + 'static,
) -> MoonMenuItem {
    MoonMenuItem::with_key(key, label).on_click(move |_, window, app| {
        window.close_context_menu(app);
        act(window, app);
    })
}

/// The «⋯» button holding the launchers a row could not fit, in the row's left-to-right order.
///
/// Sized like its icon-only neighbours ([`design::glyph_btn_w`]) so a row budgets it as one more
/// launcher. A click opens a MoonUI Root-owned menu under the pointer whose rows open the same
/// windows the folded buttons would; the Root owns focus and dismissal, so the row keeps no
/// open-state of its own. Every row closes the menu before it acts.
///
/// Args:
///     id: The button's id and debug selector; its menu is keyed `<id>-menu`.
///     lead: Rows listed before the launchers, each built by [`overflow_row`].
///     folded: The launchers folded into the menu, in display order.
///     rows: The other controls the row folded in, listed after the launchers, each built by
///         [`overflow_row`].
///     backend: Shared terminal state passed to each destination; `None` renders the button inert.
///     p: Active palette used for the glyph color.
///     cx: Application context used to resolve the control-tier width.
///
/// Returns:
///     One rendered overflow button.
pub(crate) fn overflow_button(
    id: &'static str,
    lead: Vec<MoonMenuItem>,
    folded: Vec<LaunchTarget>,
    rows: Vec<MoonMenuItem>,
    backend: Option<Entity<Backend>>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let lead = std::rc::Rc::new(lead);
    let folded = std::rc::Rc::new(folded);
    let rows = std::rc::Rc::new(rows);
    let button = MoonButton::new(id)
        .width(design::glyph_btn_w(cx))
        .variant(MoonButtonVariant::Ghost)
        .leading_icon(MoonButtonIconSlot::new("icons/ellipsis.svg").color(p.text_soft))
        .tooltip(t!("toolbar.more_launchers").to_string())
        .on_click(move |_, window, cx| {
            let Some(backend) = backend.as_ref() else {
                return;
            };
            let mut items: Vec<MoonMenuItem> = lead.iter().cloned().collect();
            items.extend(folded.iter().map(|target| {
                let owner = target.workspace_owner.clone();
                let open = target.open;
                let backend = backend.clone();
                MoonMenuItem::with_key(target.id, target.label.clone()).on_click(
                    move |_, window, app| {
                        window.close_context_menu(app);
                        launch_window(owner.as_deref(), open, &backend, window, app);
                    },
                )
            }));
            items.extend(rows.iter().cloned());
            let at = window.mouse_position();
            window.open_fitted_moon_context_menu(
                cx,
                format!("{id}-menu"),
                at,
                items,
                OVERFLOW_MENU_MIN_W,
                OVERFLOW_MENU_MAX_W,
            );
        })
        .render();
    div()
        .flex_none()
        .debug_selector(move || id.into())
        .child(button)
        .into_any_element()
}
