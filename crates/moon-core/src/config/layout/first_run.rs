//! First-run workspace selection and screen placement.

use super::*;

/// Share of a display's WORK AREA the very first window of a brand-new profile occupies.
///
/// Proportional rather than a pixel size on purpose: monitors differ, and any fixed number is
/// somebody's monitor and nobody else's.
pub const FIRST_RUN_WINDOW_FRACTION: f32 = 0.75;

/// Decide the workspace preset a layout should be seeded with, if any.
///
/// Split out of [`WindowLayout::load`] so the decision is a pure function with a mutation-sensitive
/// test rather than an inline branch reachable only by running startup against a real filesystem.
///
/// Auto is chosen for a brand-new profile because the rail gives a user with no cores something
/// coherent to look at, where Classic opens onto an empty chart. It is a SEED and not an override:
/// once written it is an ordinary stored value, and any group's own entry outranks it.
///
/// Args:
///     age: Whether any file of a configured profile existed at launch.
///     stored: The preset already in the layout, if the file carried one.
///
/// Returns:
///     `Some` preset to write into the layout, or `None` to leave it exactly as loaded — which is
///     the answer for every established profile, and for a first run that somehow already has one.
pub fn first_run_workspace_mode(
    age: super::ProfileAge,
    stored: Option<WorkspaceMode>,
) -> Option<WorkspaceMode> {
    match (age, stored) {
        (super::ProfileAge::FirstRun, None) => Some(WorkspaceMode::AutoTrading),
        (super::ProfileAge::FirstRun, Some(_)) | (super::ProfileAge::Established, _) => None,
    }
}

/// A screen rectangle in logical pixels, free of any windowing toolkit.
///
/// Deliberately plain `f32` rather than a GPUI `Bounds`: this crate has no `gpui` dependency at
/// all, so the geometry below is unit-testable without a display, and the compiler — not
/// discipline — is what keeps it that way.
///
/// The coordinate SPACE is whatever the caller's is. Platforms disagree (Windows reports global
/// desktop coordinates while macOS reports every display relative to its own origin), and
/// [`first_run_window_rect`] never mixes spaces because it derives its result solely from the
/// rectangle it is handed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Minimum overlap with an attached display, in square logical pixels, for window restoration.
pub const MIN_VISIBLE_PX: u64 = 200 * 30;

impl ScreenRect {
    /// Fit size and origin inside `work`, keeping an already contained rectangle unchanged.
    ///
    /// The work area wins over any window minimum so title bars remain reachable on small screens.
    #[must_use]
    pub(super) fn clamped_to(self, work: Self) -> Self {
        let w = self.w.min(work.w).max(0.0);
        let h = self.h.min(work.h).max(0.0);
        Self {
            x: self.x.clamp(work.x, work.x + (work.w - w).max(0.0)),
            y: self.y.clamp(work.y, work.y + (work.h - h).max(0.0)),
            w,
            h,
        }
    }
}

/// Place the first window of a brand-new profile: [`FIRST_RUN_WINDOW_FRACTION`] of the work area,
/// centred on it.
///
/// The preferred minimum is applied here before fitting to the work area. Callers must also cap
/// the native min-size hint to the resulting rectangle, or native size validation can undo that
/// fit on a display smaller than the preferred minimum.
///
/// Ordering note, and it is the only real trade-off in this function: the minimum is applied
/// first and the work-area clamp SECOND, so on a display too small to hold the minimum the clamp
/// WINS. A window wider than the screen puts its title bar and controls out of reach, which the
/// user cannot recover from; a window narrower than the preferred minimum is merely cramped.
///
/// Args:
///     work: The display's work area — the monitor minus its taskbar or dock.
///     min_w: Narrowest the window may be created.
///     min_h: Shortest the window may be created.
///
/// Returns:
///     A rectangle wholly inside the sanitized work area. A non-finite ORIGIN falls back to zero
///     and non-finite or non-positive DIMENSIONS collapse to zero — the two are sanitized
///     separately — so a display the platform could not describe yields a degenerate rectangle
///     rather than a `NaN` origin, and never a window nothing can place.
pub fn first_run_window_rect(work: ScreenRect, min_w: f32, min_h: f32) -> ScreenRect {
    let sane = |v: f32| if v.is_finite() && v > 0.0 { v } else { 0.0 };
    let (work_w, work_h) = (sane(work.w), sane(work.h));
    let (min_w, min_h) = (sane(min_w), sane(min_h));
    let (x0, y0) = (
        if work.x.is_finite() { work.x } else { 0.0 },
        if work.y.is_finite() { work.y } else { 0.0 },
    );

    let w = (work_w * FIRST_RUN_WINDOW_FRACTION)
        .round()
        .max(min_w)
        .min(work_w);
    let h = (work_h * FIRST_RUN_WINDOW_FRACTION)
        .round()
        .max(min_h)
        .min(work_h);

    // The clamp is not redundant with the centring: rounding a half-pixel can push the far edge one
    // pixel past the work area, and that pixel is the difference between a window that opens flush
    // against the screen edge and one the compositor may reposition.
    let x = (x0 + ((work_w - w) * 0.5).round()).clamp(x0, x0 + work_w - w);
    let y = (y0 + ((work_h - h) * 0.5).round()).clamp(y0, y0 + work_h - h);

    ScreenRect { x, y, w, h }
}

/// Persisted terminal workspace preset.
///
/// The serialized codes are an external layout contract. Unknown or wrong-typed values fall back
/// to [`Self::Classic`] so a newer or hand-edited preference cannot make the complete layout
/// document unreadable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WorkspaceMode {
    /// The existing chart-first, freely editable terminal workspace.
    #[default]
    Classic,
    /// The shared modular workspace with coordinated rail-owned core navigation.
    AutoTrading,
}

impl WorkspaceMode {
    /// Return the stable code written to `layout.toml`.
    ///
    /// Returns:
    ///     The English machine-readable code for this preset.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::AutoTrading => "auto-trading",
        }
    }

    /// Resolve a persisted code without rejecting the surrounding layout.
    ///
    /// Args:
    ///     code: Value read from the hand-editable layout document.
    ///
    /// Returns:
    ///     The matching preset, or [`Self::Classic`] for an unknown code.
    pub fn from_code(code: &str) -> Self {
        match code.trim() {
            "auto-trading" => Self::AutoTrading,
            _ => Self::Classic,
        }
    }
}

impl Serialize for WorkspaceMode {
    /// Serialize through [`Self::code`] so one stable-code authority serves every caller.
    ///
    /// Args:
    ///     serializer: Serde output receiving the machine-readable workspace code.
    ///
    /// Returns:
    ///     Serializer-specific success value.
    ///
    /// Errors:
    ///     Propagates serializer failures while writing the stable string.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for WorkspaceMode {
    /// Deserialize a workspace code leniently so it cannot invalidate `layout.toml`.
    ///
    /// Args:
    ///     deserializer: Serde input positioned at one workspace-mode value.
    ///
    /// Returns:
    ///     The saved preset, defaulting to Classic for every unsupported shape.
    ///
    /// Errors:
    ///     Propagates only input errors that prevent Serde from visiting the value.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// A supported text code or an ignored malformed value.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StoredMode {
            /// Stable workspace code.
            Text(String),
            /// Any future or malformed non-text shape.
            Other(serde::de::IgnoredAny),
        }

        Ok(match StoredMode::deserialize(deserializer)? {
            StoredMode::Text(code) => Self::from_code(&code),
            StoredMode::Other(_) => Self::Classic,
        })
    }
}
