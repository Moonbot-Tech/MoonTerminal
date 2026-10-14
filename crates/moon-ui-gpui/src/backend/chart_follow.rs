//! Detached chart windows following their group's main window, by the cores' checkbox (#850).
//!
//! The rule and its pure decisions live in [`crate::window::chart_follow`]; this module reads the
//! cores' flags and drives the native calls in [`crate::window::windowing`].

use gpui::{App, Entity, Window};

use crate::Backend;
use crate::window::chart_follow::{ChartFollow, charts_to_raise};
use crate::window::windowing;

impl Backend {
    /// Whether `group`'s detached charts follow its main window.
    ///
    /// Reads each core's last projected `charts_stay_on_top`. A stale projection still counts: it
    /// is the last checkbox state the core reported, and staleness only blocks edits.
    ///
    /// Args:
    ///     group: Group whose cores decide.
    ///
    /// Returns:
    ///     The group's [`ChartFollow`].
    pub(crate) fn chart_follow(&self, group: &str) -> ChartFollow {
        let store = self.session.store();
        ChartFollow::for_group(self.group_cores(group).iter().map(|(core, _)| {
            store
                .core(*core)
                .and_then(|data| data.core_config.as_ref())
                .map(|config| config.charts_stay_on_top)
        }))
    }
}

/// Apply a detached chart window's taskbar policy for its group's current mode.
///
/// Called when the chart opens and on every activation of it. Following: arm the deletion burst,
/// as every independent window of the terminal does. Independent: keep the button Windows gives an
/// unowned window, re-adding it once when an earlier burst removed it.
///
/// Args:
///     mode: The chart's group's [`ChartFollow`].
///     window: The chart window.
///     hidden: Whether this window's button was last deleted; updated in place.
///
/// Returns:
///     The token the chart host must keep, replacing (and so cancelling) its previous one.
pub(crate) fn chart_taskbar_policy(
    mode: ChartFollow,
    window: &Window,
    hidden: &mut bool,
) -> windowing::TaskbarHideTask {
    if mode.hides_chart_taskbar_buttons() {
        *hidden = true;
        windowing::hide_window_from_taskbar_soon(window)
    } else {
        if std::mem::take(hidden) {
            windowing::show_window_in_taskbar(window);
        }
        windowing::TaskbarHideTask::idle()
    }
}

/// Apply the follow rule when `group`'s main window has just been activated.
///
/// `FollowMain`: raise the group's detached charts in place, just beneath `main`, which keeps focus.
/// `Independent`: raise nothing. Taskbar buttons are not touched here: each chart host owns its
/// own burst and resyncs it when the mode changes, so a re-add here could race a delete there.
///
/// Args:
///     backend: Shared terminal state holding the chart registry.
///     group: Group whose main window was activated.
///     main: That main window.
///     cx: Application context used to reach the chart windows.
pub(crate) fn on_main_window_activated(
    backend: &Entity<Backend>,
    group: &str,
    main: &Window,
    cx: &mut App,
) {
    let Some(main_hwnd) = windowing::window_hwnd(main) else {
        return;
    };
    let handles = {
        let b = backend.read(cx);
        charts_to_raise(group, b.chart_follow(group), &b.detached_chart_windows)
    };
    let charts: Vec<isize> = handles
        .into_iter()
        .filter_map(|handle| {
            handle
                .update(cx, |_, window, _| windowing::window_hwnd(window))
                .ok()
                .flatten()
        })
        .collect();
    windowing::raise_charts_below(main_hwnd, &charts);
}
