//! Open an existing Core Status surface in Updates mode, respecting the workspace's panel route.

use gpui::*;
use moon_core::config::WorkspaceMode;
use moon_core::session::CoreId;

use super::CoreStatusMode;
use crate::Backend;

#[cfg(test)]
mod tests;

/// Stable dock and detached-window identity of Core Status.
const PANEL: &str = "CoreStatus";

/// Existing surface selected by the opener, or permission to create one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// Activate the group-owned dock tab.
    Focus,
    /// Raise the existing detached window without creating another.
    Raise,
    /// Create the panel through the normal Classic detached route.
    Open,
}

/// Panel and workspace facts needed to choose a route.
struct RouteState {
    /// Auto always keeps operational panels group-owned and forbids detachment.
    auto: bool,
    /// Core Status currently has a docked identity.
    docked: bool,
    /// Core Status currently has a detached specification.
    detached: bool,
}

/// Choose the only route that cannot duplicate an existing Core Status panel.
fn route(state: RouteState) -> Route {
    if state.auto || state.docked {
        Route::Focus
    } else if state.detached {
        Route::Raise
    } else {
        Route::Open
    }
}

/// Consume a matching request once and resolve the requested presentation.
pub(super) fn apply_updates_request(
    request: &mut Option<u64>,
    mode: &mut CoreStatusMode,
    mine: bool,
) -> Option<u64> {
    if !mine {
        return None;
    }
    let uid = request.take()?;
    *mode = CoreStatusMode::Updates;
    Some(uid)
}

/// Find a core in the Updates table's live-rows-then-history order.
pub(super) fn updates_row_for_core(
    live: impl IntoIterator<Item = CoreId>,
    history: impl IntoIterator<Item = CoreId>,
    uid: CoreId,
) -> Option<usize> {
    let live: Vec<CoreId> = live.into_iter().collect();
    live.iter().position(|core| *core == uid).or_else(|| {
        history
            .into_iter()
            .position(|core| core == uid)
            .map(|index| live.len() + index)
    })
}

/// Show Core Status for `uid` in Updates mode without introducing a second updater surface.
pub(crate) fn open_updates(backend: &Entity<Backend>, uid: u64, window: &mut Window, cx: &mut App) {
    let Some((group, state, group_open)) = route_state(backend, uid, cx) else {
        log::warn!("core updates opener: unknown core uid {uid}");
        return;
    };
    backend.update(cx, |backend, cx| {
        backend.core_status_updates_request = Some(uid);
        if state.auto {
            backend.set_auto_workspace_tab(&group, PANEL);
        }
        backend.mark_backend_dirty(cx);
    });

    match route(state) {
        Route::Focus => focus_group(backend, &group, cx),
        Route::Raise => {
            crate::window::detached::raise_existing(backend, &group, PANEL, cx);
        }
        Route::Open if group_open => open_detached(backend, group, window, cx),
        Route::Open => spawn_group(backend, group, cx),
    }
}

/// Resolve the core's group and whether Core Status is already docked or detached there.
fn route_state(
    backend: &Entity<Backend>,
    uid: u64,
    cx: &App,
) -> Option<(String, RouteState, bool)> {
    let backend = backend.read(cx);
    let group = backend
        .config
        .servers
        .iter()
        .find(|server| server.uid == uid)?
        .group
        .clone();
    let auto = backend.workspace_mode(&group) == WorkspaceMode::AutoTrading;
    let group_open = backend.group_windows.contains_key(&group);
    let detached = backend.is_detached(&group, PANEL);
    let saved_docked = crate::persistence::dock_persist::docked_panels(&backend.dock_states)
        .contains(&(group.as_str(), PANEL));
    // A live group with no saved Classic state uses the default dock unless the panel is detached.
    let default_docked = group_open && !detached && !backend.dock_states.contains_key(&group);
    let docked = auto || saved_docked || default_docked;
    Some((
        group,
        RouteState {
            auto,
            docked,
            detached,
        },
        group_open,
    ))
}

/// Activate a live group window, or restore that group with its normal dock.
fn focus_group(backend: &Entity<Backend>, group: &str, cx: &mut App) {
    let handle = backend.read(cx).group_windows.get(group).copied();
    if let Some(handle) = handle {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    } else {
        spawn_group(backend, group.to_string(), cx);
    }
}

/// Open a missing group through the application's normal group-window route.
fn spawn_group(backend: &Entity<Backend>, group: String, cx: &mut App) {
    let (config, layout, epoch) = {
        let backend = backend.read(cx);
        (
            backend.config.clone(),
            backend.layout.clone(),
            backend.epoch,
        )
    };
    crate::window::group_window::spawn_group_window(
        cx, backend, &config, group, epoch, &layout, 0.0,
    );
}

/// Create the one missing Classic panel through the same detached-window factory as the dock menu.
fn open_detached(backend: &Entity<Backend>, group: String, window: &mut Window, cx: &mut App) {
    let mut spec = crate::window::detached::DetachedSpec::with_saved_geom(
        backend,
        cx,
        group,
        PANEL.to_string(),
    );
    let owner_display = crate::window::windowing::window_display_id(window, cx);
    match crate::window::detached::spawn(
        cx,
        backend,
        &mut spec,
        Some(window.window_handle()),
        owner_display,
    ) {
        Ok(handle) => {
            backend.update(cx, |backend, _| {
                if !backend.is_detached(&spec.group, &spec.panel) {
                    backend.detached.push(spec);
                    backend.detached_dirty = true;
                }
            });
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
        Err(error) => log::warn!("core updates panel could not be opened: {error:#}"),
    }
}
