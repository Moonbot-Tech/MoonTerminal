//! Whether a group's detached chart windows follow its main window (#850).
//!
//! MoonBot's "Charts windows on top of other windows" checkbox (`ChartsSettings.stay_on_top`,
//! default on) decides it, per core; the terminal has no setting of its own. The pure decisions
//! live here so they are testable without a window; [`crate::window::windowing`] owns the native
//! raise and the taskbar calls, and `docs/WINDOWING.md` states the rule.

/// The charts' relationship to their group's main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChartFollow {
    /// Activating the main window raises the group's detached charts in place, and the charts keep
    /// no taskbar button of their own: the terminal presents one icon.
    FollowMain,
    /// The charts are on their own: activating the main window leaves them where they are in the
    /// z-order, and each keeps the taskbar button Windows gives an unowned window, so it can be
    /// raised by itself.
    Independent,
}

impl ChartFollow {
    /// Decide a group's mode from each of its cores' checkbox.
    ///
    /// ANY core on is enough (the owner's call for a group whose cores disagree). A core whose
    /// settings have not arrived yet counts as on, MoonBot's default, so a group starts in the
    /// mode most users are in rather than flipping once the first snapshot lands; a group with no
    /// core at all is the same default for the same reason.
    ///
    /// Args:
    ///     flags: One entry per core of the group: `Some(stay_on_top)`, or `None` when unknown.
    ///
    /// Returns:
    ///     `FollowMain` unless every core is known and has the checkbox off.
    pub(crate) fn for_group(flags: impl IntoIterator<Item = Option<bool>>) -> Self {
        let mut any_core = false;
        for flag in flags {
            any_core = true;
            if flag.unwrap_or(true) {
                return Self::FollowMain;
            }
        }
        if any_core {
            Self::Independent
        } else {
            Self::FollowMain
        }
    }

    /// Whether the charts' own taskbar buttons are deleted in this mode.
    ///
    /// Returns:
    ///     `true` when the terminal should present one taskbar icon for the whole group.
    pub(crate) fn hides_chart_taskbar_buttons(self) -> bool {
        matches!(self, Self::FollowMain)
    }
}

/// Pick the detached chart windows one activation of `group`'s main window raises.
///
/// Only that group's charts, and only in `FollowMain`; the registry order is kept so the caller
/// can stack them deterministically.
///
/// Args:
///     group: Group whose main window was activated.
///     mode: That group's [`ChartFollow`].
///     registry: Every detached chart window as `(group, handle)`.
///
/// Returns:
///     Handles to raise, possibly empty.
pub(crate) fn charts_to_raise<H: Copy>(
    group: &str,
    mode: ChartFollow,
    registry: &[(String, H)],
) -> Vec<H> {
    if mode != ChartFollow::FollowMain {
        return Vec::new();
    }
    registry
        .iter()
        .filter(|(g, _)| g == group)
        .map(|(_, h)| *h)
        .collect()
}

#[cfg(test)]
mod tests;
