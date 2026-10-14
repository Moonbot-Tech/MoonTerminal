//! Which core a blacklist action may name, shared by the places dialog and the chart filter lines.
//!
//! An empty workspace group is refused before the workspace gate. Passing `None` into
//! [`crate::Backend::workspace_action_allows_core`] would allow every core.

use moon_core::session::CoreId;

use crate::Backend;

/// The core's workspace group, when the session names one.
///
/// Args:
///     backend: Live session.
///     core: Core the action addresses.
///
/// Returns:
///     The group name, or `None` when the core is missing or its group is empty.
pub(crate) fn core_action_group(backend: &Backend, core: CoreId) -> Option<String> {
    let group = backend
        .session
        .sessions()
        .iter()
        .find(|session| session.id == core)?
        .group
        .clone();
    if group.is_empty() { None } else { Some(group) }
}

/// Whether this core's own group may act on it.
///
/// Args:
///     backend: Live session and workspace authority.
///     core: Core the action addresses.
///
/// Returns:
///     Whether that core's group authorizes the action.
pub(crate) fn core_action_allowed(backend: &Backend, core: CoreId) -> bool {
    let Some(group) = core_action_group(backend, core) else {
        return false;
    };
    backend.workspace_action_allows_core(Some(group.as_str()), core)
}

/// Session name of a core, or the core id when the name is empty or the session is missing.
///
/// Args:
///     backend: Live session.
///     core: Core to label.
///
/// Returns:
///     The session name, or `core` rendered as a decimal id.
pub(crate) fn core_session_name(backend: &Backend, core: CoreId) -> String {
    backend
        .session
        .sessions()
        .iter()
        .find(|session| session.id == core)
        .map(|session| {
            if session.name.is_empty() {
                core.to_string()
            } else {
                session.name.clone()
            }
        })
        .unwrap_or_else(|| core.to_string())
}
