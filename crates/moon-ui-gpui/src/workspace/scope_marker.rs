//! Shared assembled-facts producer for the scope marker every membership-filtered aggregate
//! renders when the active workspace preset hides at least one configured core.
//!
//! One producer, not one widget (frozen contract §6): the five surfaces render in three
//! incompatible shapes — a fixed-height flex footer, a structured `FooterFacts`, a single caption
//! line — so a shared widget would be hand-rolled chrome, which `CONTRIBUTING.md`'s UI section
//! forbids. What must be identical across all five is the FACTS and their order, never the
//! rendering; each surface splices [`ScopeMarker::facts`] into its own existing clipping tail and
//! passes that same `Vec` into its own tooltip, exactly as `panels/assets/balances.rs` already
//! does for its own facts.

use moon_core::config::WorkspaceMode;
use moon_core::util::fmt;
use rust_i18n::t;

/// Typed facts about the scope an aggregate was computed over.
///
/// `moon-core` cannot localize (`rust_i18n!` lives in `main.rs`), so the preset arrives as a
/// [`WorkspaceMode`] value and the counts as `usize` — never as a pre-built `String`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScopeMarker {
    preset: Option<WorkspaceMode>,
    shown: usize,
    configured: usize,
}

impl ScopeMarker {
    /// Build a marker from the membership boundary's own counts.
    ///
    /// Args:
    ///     preset: Preset the surface displays under, resolved through `Backend::display_preset`.
    ///         `None` means unscoped or an unresolved singleton focus — both mean show everything.
    ///     shown: Cores that survive the current preset's membership filter.
    ///     configured: Cores that survived availability before the membership filter ran.
    ///
    /// Returns:
    ///     A marker ready for [`Self::hides_anything`], [`Self::facts`] and [`Self::tooltip`].
    pub(crate) fn new(preset: Option<WorkspaceMode>, shown: usize, configured: usize) -> Self {
        Self {
            preset,
            shown,
            configured,
        }
    }

    /// Whether the active preset hides at least one configured core.
    ///
    /// Frozen contract §10.6 supersedes the plan's first draft, which OR'd in `preset.is_none()`.
    /// An unresolved preset means SHOW EVERYTHING under the H3 rule, so it can never itself be a
    /// reason to render a marker — only an actual exclusion is.
    ///
    /// Returns:
    ///     `true` only when membership actually excluded a configured core.
    pub(crate) fn hides_anything(&self) -> bool {
        self.shown < self.configured
    }

    /// Localized facts, in clipping priority order (most important first).
    ///
    /// Empty whenever [`Self::hides_anything`] is `false` — a full scope states nothing, exactly
    /// as decision 1 requires.
    ///
    /// Returns:
    ///     `· `-prefixed fact strings, or an empty `Vec`.
    pub(crate) fn facts(&self) -> Vec<String> {
        if !self.hides_anything() {
            return Vec::new();
        }
        // `core_displayed(None, _)` always answers `true`, so `shown` cannot fall short of
        // `configured` while `preset` is `None` — the guard above already proved
        // `hides_anything()`, so a resolved preset is guaranteed here.
        let preset = self.preset.expect(
            "hides_anything() is true, so core_displayed's None-means-show-everything rule \
             guarantees a resolved preset",
        );
        let mode = match preset {
            WorkspaceMode::Classic => t!("workspace.mode.classic"),
            WorkspaceMode::AutoTrading => t!("workspace.mode.auto"),
        };
        vec![
            format!("· {}", t!("workspace.scope.preset", mode = mode)),
            format!(
                "· {}",
                t!(
                    "workspace.scope.cores_n_of_m",
                    n = fmt::group_thousands(&self.shown.to_string()),
                    total = fmt::group_thousands(&self.configured.to_string())
                )
            ),
        ]
    }

    /// Build the recovery tooltip from the SAME facts the row rendered.
    ///
    /// Args:
    ///     tail: Already-rendered facts, in the order they were drawn — this surface's own,
    ///         [`Self::facts`], or both concatenated.
    ///
    /// Returns:
    ///     `tail` joined by spaces with the closing hint appended, or an empty string when nothing
    ///     is hidden — a full scope has no hint to give.
    pub(crate) fn tooltip(&self, tail: &[String]) -> String {
        if !self.hides_anything() {
            return String::new();
        }
        let mut out = tail.join(" ");
        out.push('\n');
        out.push_str(&t!("workspace.scope.hint"));
        out
    }
}
