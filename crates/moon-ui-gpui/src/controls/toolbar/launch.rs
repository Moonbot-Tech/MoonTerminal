//! Trailing window launchers and their overflow menu.

use super::*;

/// Open one launcher's singleton window from `window`, exactly as its toolbar button does.
fn launch_window(
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

/// A toolbar button that opens a singleton window, styled like Live
/// (Soft/Sm). `labeled_width = None` renders the icon alone with its name as a tooltip;
/// `Some(w)` renders icon + label inside the fixed width `w`. Every destination uses one shared
/// open signature and deduplicates or focuses its own singleton window.
///
/// Args:
///     target: The launcher's identity, label, glyph, width and destination.
///     backend: Shared terminal state passed to the destination.
///     p: Active palette used for icon and text colors.
///     cx: Application context used to resolve the control-tier icon-only width.
///
/// Returns:
///     One rendered compact launcher button.
pub(super) fn open_window_button(
    target: LaunchTarget,
    backend: Entity<Backend>,
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
        .variant(MoonButtonVariant::Soft)
        .leading_icon(MoonButtonIconSlot::new(icon).color(p.text_soft));
    btn = if labeled_width.is_some() {
        btn.padding_x(TOOLBAR_LAUNCHER_PAD_X)
            .text_segment(label, p.text, 500.0)
    } else {
        btn.tooltip(label)
    };
    btn.on_click(move |_, window, cx| {
        launch_window(workspace_owner.as_deref(), open, &backend, window, cx);
    })
    .render()
    .into_any_element()
}

/// Width bounds of the overflow menu, in design pixels; the menu fits its rows inside them.
const OVERFLOW_MENU_MIN_W: f32 = 120.0;
/// Upper width bound of the overflow menu, in design pixels.
const OVERFLOW_MENU_MAX_W: f32 = 280.0;

/// The «⋯» button holding the launchers the row could not fit, in the row's left-to-right order.
///
/// Sized like its icon-only neighbours ([`design::glyph_btn_w`]) so [`row_fit`] budgets it as one
/// more launcher. A click opens a MoonUI Root-owned menu under the pointer whose rows open the
/// same windows the folded buttons would; the Root owns focus and dismissal, so this row keeps no
/// open-state of its own.
///
/// Args:
///     folded: The launchers folded into the menu, in display order.
///     backend: Shared terminal state passed to each destination.
///     p: Active palette used for the glyph color.
///     cx: Application context used to resolve the control-tier width.
///
/// Returns:
///     One rendered overflow button.
pub(super) fn overflow_button(
    folded: Vec<LaunchTarget>,
    backend: Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let folded = std::rc::Rc::new(folded);
    MoonButton::new("toolbar-overflow")
        .width(design::glyph_btn_w(cx))
        .variant(MoonButtonVariant::Soft)
        .leading_icon(MoonButtonIconSlot::new("icons/ellipsis.svg").color(p.text_soft))
        .tooltip(t!("toolbar.more_launchers").to_string())
        .on_click(move |_, window, cx| {
            let items = folded
                .iter()
                .map(|target| {
                    let owner = target.workspace_owner.clone();
                    let open = target.open;
                    let backend = backend.clone();
                    MoonMenuItem::with_key(target.id, target.label.clone()).on_click(
                        move |_, window, app| {
                            window.close_context_menu(app);
                            launch_window(owner.as_deref(), open, &backend, window, app);
                        },
                    )
                })
                .collect();
            let at = window.mouse_position();
            window.open_fitted_moon_context_menu(
                cx,
                "toolbar-overflow-menu",
                at,
                items,
                OVERFLOW_MENU_MIN_W,
                OVERFLOW_MENU_MAX_W,
            );
        })
        .render()
        .into_any_element()
}
