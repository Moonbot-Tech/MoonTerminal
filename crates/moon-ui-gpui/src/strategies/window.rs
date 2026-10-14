//! Strategies window: the header and the open/goto entry points.

use super::*;

#[cfg(test)]
mod tests;

pub(super) const STRATEGIES_HEADER_H: f32 = 32.0;

/// Build the window header with native minimize, maximize, and close controls.
pub(super) fn strategies_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("strategies-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, STRATEGIES_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("strategies-titlebar-title", 0.0)
                .title_cluster(t!("strat.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("strategies-window-frame-visual", 0.0)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(STRATEGIES_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Which strategy a `Backend::strategies_goto` request points at.
///
/// Two forms because the two kinds of caller know different things: a chart order line or an
/// Orders row already holds the id, while a just-created copy does not; its id is assigned by
/// the core and only arrives with the echo, so it can be named but not numbered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevealTarget {
    /// A strategy the caller already knows the id of.
    Id(u64),
    /// A strategy identified by its name on that core.
    Name(String),
}

/// One strategy field a reveal should scroll to, plus the coin token to highlight there.
///
/// `key` is the schema field name. `coin` is the token, not a pair; an empty token requests
/// only the field flash, without coin highlighting or removal controls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RevealField {
    /// Schema field name, for example `CoinsBlackList`.
    pub key: &'static str,
    /// Coin token to highlight inside that field, or empty for a field-only reveal.
    pub coin: String,
}

/// One queued Strategies reveal plus immutable optional group workspace authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StrategyRevealRequest {
    pub(super) core: CoreId,
    pub(super) target: RevealTarget,
    pub(super) workspace_group: Option<String>,
    /// A saved version to open in the Versions pane once the strategy is selected, by its
    /// `valid_from` key. `None` leaves the pane on live mode, as every reveal did before.
    pub(super) version: Option<i64>,
    /// Schema field to scroll to once the strategy is selected.
    /// `None` leaves the params pane untouched, as every reveal did before.
    pub(super) field: Option<RevealField>,
}

impl StrategyRevealRequest {
    /// Capture one exact reveal request at its producer boundary.
    ///
    /// Args:
    ///     core: Core that owns the requested strategy.
    ///     target: Exact id or echo-resolved name to reveal.
    ///     workspace_group: Immutable owning group, or `None` for an unscoped producer.
    ///
    /// Returns:
    ///     One request carrying both the reveal identity and its workspace authority.
    pub(super) fn new(core: CoreId, target: RevealTarget, workspace_group: Option<String>) -> Self {
        Self {
            core,
            target,
            workspace_group,
            version: None,
            field: None,
        }
    }

    /// Ask the reveal to also open one saved version of the strategy.
    pub(super) fn with_version(mut self, version: Option<i64>) -> Self {
        self.version = version;
        self
    }

    /// Attach an optional field reveal; an empty coin requests only the field flash.
    ///
    /// Args:
    ///     field: Schema key and optional coin token, or `None` for strategy-only navigation.
    ///
    /// Returns:
    ///     The same request with that optional field attached.
    pub(super) fn with_field(mut self, field: impl Into<Option<RevealField>>) -> Self {
        self.field = field.into();
        self
    }

    /// Revalidate this request without changing the current workspace selection.
    ///
    /// Args:
    ///     backend: Current live session and workspace authority.
    ///
    /// Returns:
    ///     Whether the captured core remains visible to the original producer.
    pub(super) fn is_authorized(&self, backend: &Backend) -> bool {
        workspace_allows_reveal(backend, self.workspace_group.as_deref(), self.core)
    }
}

/// Revalidate one strategy reveal against the captured group workspace authority.
///
/// Args:
///     backend: Shared state containing current workspace scope.
///     workspace_group: Group that owned the callback, or `None` for an unscoped caller.
///     core: Core that owns the requested strategy.
///
/// Returns:
///     Whether the captured target remains visible without changing workspace selection.
fn workspace_allows_reveal(backend: &Backend, workspace_group: Option<&str>, core: CoreId) -> bool {
    backend.workspace_action_allows_core(workspace_group, core)
}

/// Open or focus the Strategies window and navigate to `strat_id` on `core`.
/// Render drains the request, clears any tree filter that hides the target, persists active-only as
/// disabled when it was hiding the target, expands the target core and folders, and selects the
/// strategy. Entry points include chart order-line context menus and the Orders table's Strat
/// column. Group-owned callers pass their workspace group so a retained callback cannot switch or
/// reveal a core hidden by a later rail selection; unscoped callers pass `None`.
///
/// Args:
///     backend: Shared state and singleton-window authority.
///     core: Core captured by the navigation producer.
///     strat_id: Exact strategy identity on `core` — live, or retained in the Deleted branch;
///         one found in neither is reported by the window rather than dropped.
///     workspace_group: Group owning the callback, or `None` for an unscoped caller.
///     owner: Optional native owner window.
///     owner_display: Display used when a new tool window is required.
///     cx: Application context used to validate, queue, and open.
///
/// Returns:
///     Nothing; a stale group-owned target is rejected without changing workspace state.
pub fn open_goto(
    backend: Entity<Backend>,
    core: CoreId,
    strat_id: u64,
    workspace_group: Option<String>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    open_goto_version(
        backend,
        core,
        strat_id,
        None,
        workspace_group,
        owner,
        owner_display,
        cx,
    );
}

/// [`open_goto`] that also opens one saved version in the Versions pane.
///
/// The trade window's version stamp is the caller: it prints the `valid_from` the pane labels a
/// version with, and a click on it lands on that very row instead of leaving the reader to find
/// the stamp by eye. A `None` version is exactly `open_goto`.
///
/// Args:
///     backend: Shared state and singleton-window authority.
///     core: Core captured by the navigation producer.
///     strat_id: Exact strategy identity on `core`, live or retained in the Deleted branch.
///     version: `valid_from` of the saved version to select, or `None` for live mode.
///     workspace_group: Group owning the callback, or `None` for an unscoped caller.
///     owner: Optional native owner window.
///     owner_display: Display used when a new tool window is required.
///     cx: Application context used to validate, queue, and open.
///
/// Returns:
///     Nothing; a stale group-owned target is rejected without changing workspace state.
#[allow(clippy::too_many_arguments)] // Mirrors `open_goto`'s producer-boundary shape plus the version.
pub fn open_goto_version(
    backend: Entity<Backend>,
    core: CoreId,
    strat_id: u64,
    version: Option<i64>,
    workspace_group: Option<String>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    let queued = backend.update(cx, |b, bcx| {
        if !workspace_allows_reveal(b, workspace_group.as_deref(), core) {
            return false;
        }
        b.strategies_goto = Some(
            StrategyRevealRequest::new(core, RevealTarget::Id(strat_id), workspace_group.clone())
                .with_version(version),
        );
        // Wake an existing window's observer because `open` only focuses a deduplicated window.
        bcx.notify();
        true
    });
    if !queued {
        return;
    }
    open(backend, owner, owner_display, cx);
}

/// [`open_goto`] with an optional field reveal and the destination group's singleton scope.
///
/// Places rows reveal `CoinsBlackList` with a coin. Chart refusal lines reveal another field
/// without a coin, or select only the strategy when the refusing field is unknown.
///
/// Args:
///     backend: Shared state and singleton-window authority.
///     core: Core captured by the navigation producer.
///     strat_id: Exact strategy identity on `core`, live or retained in the Deleted branch.
///     field: Optional schema key and coin token to reveal once the strategy is selected.
///     workspace_group: Group owning the callback, or `None` for an unscoped caller.
///     owner: Optional native owner window.
///     owner_display: Display used when a new tool window is required.
///     cx: Application context used to validate, queue, and open.
///
/// Returns:
///     Nothing; a stale group-owned target is rejected without changing workspace state.
#[allow(clippy::too_many_arguments)] // Mirrors `open_goto_version`'s producer-boundary shape plus the field.
pub(crate) fn open_goto_field(
    backend: Entity<Backend>,
    core: CoreId,
    strat_id: u64,
    field: impl Into<Option<RevealField>>,
    workspace_group: Option<String>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    let queued = backend.update(cx, |b, bcx| {
        if !workspace_allows_reveal(b, workspace_group.as_deref(), core) {
            return false;
        }
        b.strategies_goto = Some(
            StrategyRevealRequest::new(core, RevealTarget::Id(strat_id), workspace_group.clone())
                .with_field(field),
        );
        bcx.notify();
        true
    });
    if !queued {
        return;
    }
    focus_reveal_scope(&backend, workspace_group.as_deref(), cx);
    open(backend, owner, owner_display, cx);
}

/// [`open_goto_field`] for a strategy the caller can name but not yet number.
///
/// The chart's filter line is the caller: the caption carries the strategy name, and the id is
/// known only when that name is already in the store. A miss still opens the window and parks
/// [`RevealTarget::Name`] with any optional field attached, using the existing name-reveal path.
///
/// Args:
///     backend: Shared state and singleton-window authority.
///     core: Core captured by the navigation producer.
///     name: Strategy name, compared exactly with the store.
///     field: Optional schema key and coin token to reveal once the strategy is selected.
///     workspace_group: Group owning the callback, or `None` for an unscoped caller.
///     owner: Optional native owner window.
///     owner_display: Display used when a new tool window is required.
///     cx: Application context used to validate, queue, and open.
///
/// Returns:
///     Nothing; a stale group-owned target is rejected without changing workspace state.
#[allow(clippy::too_many_arguments)] // Mirrors `open_goto_field`'s producer-boundary shape.
pub(crate) fn open_goto_named_field(
    backend: Entity<Backend>,
    core: CoreId,
    name: String,
    field: impl Into<Option<RevealField>>,
    workspace_group: Option<String>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    let queued = backend.update(cx, |b, bcx| {
        if !workspace_allows_reveal(b, workspace_group.as_deref(), core) {
            return false;
        }
        b.strategies_goto = Some(
            StrategyRevealRequest::new(core, RevealTarget::Name(name), workspace_group.clone())
                .with_field(field),
        );
        bcx.notify();
        true
    });
    if !queued {
        return;
    }
    focus_reveal_scope(&backend, workspace_group.as_deref(), cx);
    open(backend, owner, owner_display, cx);
}

/// Group a field reveal must focus before the Strategies window opens.
///
/// The singleton lists only the current Auto owner's cores. A destination in another group
/// hides the strategy until that group owns the window, which is what that group's toolbar
/// does when it opens Strategies.
///
/// Args:
///     destination: Group that owns the strategy, when the caller named one.
///     current: The singleton's current Auto workspace owner, when it has one.
///
/// Returns:
///     `destination` when it differs from `current`.
pub(crate) fn reveal_scope_group<'a>(
    destination: Option<&'a str>,
    current: Option<&str>,
) -> Option<&'a str> {
    match destination {
        Some(group) if current != Some(group) => Some(group),
        _ => None,
    }
}

/// Move singleton scope onto the reveal's group when that group is not already the owner.
///
/// Does not change the active trading core. A group with no live window is left alone;
/// [`focus_singleton_owner`] refuses it.
fn focus_reveal_scope(backend: &Entity<Backend>, destination: Option<&str>, cx: &mut App) {
    let current = backend
        .read(cx)
        .singleton_workspace()
        .map(|workspace| workspace.group);
    let Some(group) = reveal_scope_group(destination, current.as_deref()) else {
        return;
    };
    backend.update(cx, |backend, backend_cx| {
        backend.focus_singleton_owner(group, backend_cx);
    });
}

/// Ask an ALREADY-OPEN Strategies window to reveal a strategy by name.
///
/// By name because the caller cannot know the id: a created strategy's id is assigned core-side
/// and only arrives with the snapshot echo. Render clears any hiding tree filter, persisting
/// active-only as disabled when required, before selecting the echoed row.
///
/// Deliberately does NOT open the window. A dialog in another window that spawns a second window
/// is a surprise, and a request parked on `Backend` for a window that is never opened would fire
/// hours later and yank the user's selection out from under them.
///
/// Liveness is proved by UPDATING the handle rather than by `is_some()`: nothing clears
/// `strategies_window` when the user closes the window, so the handle outlives it.
///
/// Args:
///     backend: Shared state and existing Strategies window handle.
///     core: Core that should own the echoed strategy.
///     name: Strategy name assigned by the core.
///     cx: Application context used to validate and queue the reveal.
///
/// Returns:
///     Whether a live window accepted a still-authorized request.
pub fn reveal_name(backend: &Entity<Backend>, core: CoreId, name: String, cx: &mut App) -> bool {
    let Some(handle) = backend.read(cx).strategies_window else {
        return false;
    };
    if handle.update(cx, |_, _, _| ()).is_err() {
        return false;
    }
    let queued = backend.update(cx, |b, bcx| {
        let workspace_group = b.singleton_workspace().map(|workspace| workspace.group);
        if !workspace_allows_reveal(b, workspace_group.as_deref(), core) {
            return false;
        }
        b.strategies_goto = Some(StrategyRevealRequest::new(
            core,
            RevealTarget::Name(name),
            workspace_group,
        ));
        bcx.notify();
        true
    });
    if !queued {
        return false;
    }
    true
}

/// Open the Strategies tool window, deduplicated through `Backend`.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    // Focus an existing window and expand the Auto-selected core if it is collapsed.
    if let Some(handle) = backend.read(cx).strategies_window {
        match handle.update(cx, |root, window, _cx| {
            window.activate_window();
            root.view().clone().downcast::<StrategiesView>().ok()
        }) {
            Ok(Some(view)) => {
                view.update(cx, |view, cx| {
                    view.ensure_auto_selected_core_expanded(cx);
                });
                return;
            }
            Ok(None) => return,
            Err(_) => {}
        }
    }
    // The tool window behaves as part of the terminal rather than a separate taskbar application.
    // Restore geometry persisted by `StrategiesView`.
    let saved = backend.read(cx).layout.strategies_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(120.0), px(90.0)),
            size: size(px(1180.0), px(680.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Choose a display from the saved position where supported, otherwise from the owner. Without a
    // display id, GPUI creates the window on the primary display and may discard off-screen bounds.
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.and_then(|g| g.display_uuid),
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::window::windowing::tool_window_options(
        t!("strat.window_title").to_string(),
        crate::window::windowing::restored_window_bounds(
            saved,
            crate::window::windowing::reachable_window_bounds(bounds, display_id, None, cx),
        ),
        Some(size(px(920.0), px(560.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| StrategiesView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.strategies_window = Some(handle));
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}
