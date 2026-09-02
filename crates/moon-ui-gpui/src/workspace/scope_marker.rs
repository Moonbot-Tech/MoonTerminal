//! Shared assembled-facts producer for the scope marker every membership-filtered aggregate
//! renders when the active workspace preset hides at least one configured core.
//!
//! One producer, not one widget (frozen contract §6): the consuming surfaces render in
//! incompatible shapes — a fixed-height clipping dock footer, a structured `FooterFacts`, a single
//! caption line, a centred empty block — so a shared widget would be hand-rolled chrome, which
//! `CONTRIBUTING.md`'s UI section forbids. Deliberately no count of surfaces here: consumers are
//! added by ordinary work, and a number in this sentence is false the moment one is. What must be
//! identical across every consumer is the FACTS and their order, never the rendering; each surface
//! splices [`ScopeMarker::facts`] into its own existing clipping tail and passes that same `Vec`
//! into its own tooltip, exactly as `panels/assets/balances.rs` already does for its own facts.
//!
//! [`scope_footer`] and [`scope_footer_tooltip`] below are that splice, done ONCE. A surface whose
//! footer is a fixed-height dock line — one that cannot measure itself, so it degrades by CLIPPING
//! a priority-ordered tail rather than wrapping — differs from its neighbours only in the localized
//! HEAD it states; the head/tail split, the fact ORDER and the tooltip policy are the same
//! everywhere, so they live here and are tested once instead of per surface.

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

    /// Count a membership boundary the caller iterates itself.
    ///
    /// The Backend-free counterpart to [`Self::new`], for a surface that has no
    /// `EffectiveCoreScope` to read counts off — a group-less SINGLETON window, which resolves its
    /// preset through `Backend::display_preset(DisplayOwner::Singleton)` and then filters some
    /// universe of its own with `Backend::core_displayed`. The universe is deliberately the
    /// CALLER's: the Screener filters every live session, while the Strategies tree lists connected
    /// cores only, and a helper that picked one of those would mis-describe the other. Counting one
    /// universe and classifying against a different one is how a merely disconnected core gets
    /// reported as hidden by a preset.
    ///
    /// Args:
    ///     preset: Preset the surface displays under. `None` means unscoped or an unresolved
    ///         singleton focus — both mean show everything, so there is no marker to build.
    ///     displayed: One `Backend::core_displayed` answer per core in the caller's universe, in
    ///         any order.
    ///
    /// Returns:
    ///     A marker over that universe, or `None` when the preset is unresolved.
    pub(crate) fn from_membership(
        preset: Option<WorkspaceMode>,
        displayed: impl IntoIterator<Item = bool>,
    ) -> Option<Self> {
        preset?;
        let mut shown = 0;
        let mut configured = 0;
        for visible in displayed {
            configured += 1;
            shown += usize::from(visible);
        }
        Some(Self::new(preset, shown, configured))
    }

    /// Whether the active preset hides EVERY configured core, leaving the surface empty.
    ///
    /// The single predicate that switches an empty state's TEXT from "there is no data" to "the
    /// preset is hiding it" — two different facts that must never share a string, because the first
    /// sends the user to fix a connection that is already fine.
    ///
    /// `configured > 0` is load-bearing rather than defensive: a group whose cores are all
    /// disconnected counts zero of zero, and `0 < 0` is false for [`Self::hides_anything`] but
    /// `shown == 0` alone would be true here — so without the guard an empty terminal would claim a
    /// preset hid cores it never had.
    ///
    /// Returns:
    ///     `true` only when a non-empty universe was reduced to nothing by membership.
    pub(crate) fn hides_everything(&self) -> bool {
        self.configured > 0 && self.shown == 0
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

    /// The localized facts themselves, in clipping priority order, carrying NO separator.
    ///
    /// The separator is punctuation between neighbours, not part of a fact, so it belongs to
    /// whoever joins them — [`Self::facts`] for a footer tail that always follows a head,
    /// [`Self::line`] for a marker that stands alone.
    ///
    /// Returns:
    ///     Bare fact strings, or an empty `Vec` when nothing is hidden.
    fn bare_facts(&self) -> Vec<String> {
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
            t!("workspace.scope.preset", mode = mode).to_string(),
            t!(
                "workspace.scope.cores_n_of_m",
                n = fmt::group_thousands(&self.shown.to_string()),
                total = fmt::group_thousands(&self.configured.to_string())
            )
            .to_string(),
        ]
    }

    /// Localized facts for a footer TAIL, in clipping priority order (most important first).
    ///
    /// Each fact carries its own leading `· ` because a tail is always drawn AFTER a head, and
    /// each fact is its own clipping element: the separator has to travel with the fact it
    /// introduces, or clipping the second fact would leave a dangling bullet behind.
    ///
    /// Empty whenever [`Self::hides_anything`] is `false` — a full scope states nothing, exactly
    /// as decision 1 requires.
    ///
    /// Returns:
    ///     `· `-prefixed fact strings, or an empty `Vec`.
    pub(crate) fn facts(&self) -> Vec<String> {
        self.bare_facts()
            .into_iter()
            .map(|fact| format!("· {fact}"))
            .collect()
    }

    /// The marker as ONE standalone line, separated but never PREFIXED.
    ///
    /// A surface that renders the marker with nothing before it — the Analytics summary, the
    /// Strategies tree, a hover tooltip whose whole body is the marker — has nothing for a leading
    /// separator to separate it from, and [`Self::facts`]'s per-fact `· ` then reads as a stray
    /// bullet rather than as punctuation. Same facts, same order, one joiner difference.
    ///
    /// Returns:
    ///     `"режим: MANUAL · 3 из 56 ядер"`, or an empty string when nothing is hidden.
    pub(crate) fn line(&self) -> String {
        self.bare_facts().join(" · ")
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
        // "Some cores are hidden" is false when every one of them is, and the difference is not
        // pedantry: a user reading "some" next to an empty surface looks for the rest of the data,
        // which is not there. Same recovery advice either way, so only the first clause moves.
        let hint = if self.hides_everything() {
            t!("workspace.scope.all_hidden_hint")
        } else {
            t!("workspace.scope.hint")
        };
        out.push_str(&hint);
        out
    }
}

/// A fixed-height dock footer, split by what a narrow panel may take away.
///
/// A dock panel cannot measure itself — `design::ticker_visible` needs a window render root — so
/// instead of breakpoints the row is two boxes: a `flex_none` head that never yields, and one
/// `min_w_0` + `overflow_hidden` tail laid out left to right in descending priority, clipping at
/// its right edge. Assembling that split away from the render file is what makes the ORDER
/// unit-testable, and what lets the tooltip be built from the very strings the row draws.
/// `panels/report/totals.rs` is the richer worked example of the same shape, with per-fact tones.
pub(crate) struct ScopeFooter {
    /// The surface's own figures. Never clipped: they are what the row exists to state.
    pub(crate) head: String,
    /// The marker's facts, clipped from the right. Empty whenever nothing is hidden.
    pub(crate) tail: Vec<String>,
}

/// Splice a surface's localized head with the marker's facts, in the marker's own order.
///
/// The order is [`ScopeMarker::facts`]'s and is never re-derived or re-sorted at a call site: every
/// surface that states a scope states it identically, which is the whole reason one producer exists.
/// The figures lead because they are the answer; the marker follows because it QUALIFIES that
/// answer, and a qualifier is the right thing to lose first to a narrow dock — the tooltip keeps it
/// reachable either way.
///
/// Args:
///     head: Already-localized figures this footer states.
///     marker: Scope marker for the surface, or `None` when it is deliberately unscoped.
///
/// Returns:
///     The never-clipped head and the clip-ordered tail.
pub(crate) fn scope_footer(head: String, marker: Option<&ScopeMarker>) -> ScopeFooter {
    ScopeFooter {
        head,
        tail: marker.map(ScopeMarker::facts).unwrap_or_default(),
    }
}

/// Render the clipping tail as the element every dock footer draws it as, or nothing.
///
/// The module doc above claims the head/tail split and the tooltip policy are identical across
/// surfaces and therefore live here; until this function existed that was true only of the DATA,
/// while three panels hand-wrote the same twenty lines of flex to express it. What differs per
/// surface is the element id and the palette token, so those are arguments and the rest is not.
///
/// `None` whenever the tail is empty, and that is the "nothing hidden looks exactly as before"
/// criterion made structural rather than remembered: an attached-but-empty box still consumes its
/// neighbour's `gap` slot and still changes the row's child count. The caller splices the result
/// with `.children(..)`, which draws nothing for a `None`.
///
/// Three properties of the returned element are load-bearing. It is `flex_1` + `min_w_0` so it
/// takes a zero flex basis and grows into whatever the row leaves, contributing nothing to shrink
/// — a shortfall lands on it rather than on the figures. It is `overflow_hidden` so it CLIPS at its
/// right edge instead of wrapping, which a fixed-height dock row cannot afford. And every fact
/// inside it is `flex_none`, so earlier facts keep their intrinsic widths and later ones disappear
/// first, in the marker's own priority order.
///
/// Args:
///     id: Element id, unique within the row that hosts it.
///     tail: [`ScopeFooter::tail`], moved.
///     tip: [`scope_footer_tooltip`]'s result. Attached only when non-empty — an empty tooltip
///         still paints an empty bubble on hover.
///     text_size: The row's own body size.
///     color: The row's own muted or soft text token, resolved by the caller from the palette.
///
/// Returns:
///     The tail element, or `None` when there is nothing to state.
pub(crate) fn scope_footer_tail(
    id: &'static str,
    tail: Vec<String>,
    tip: String,
    text_size: gpui::Pixels,
    color: u32,
) -> Option<impl gpui::IntoElement> {
    use gpui::prelude::*;
    use gpui::{div, rgb};
    use moon_ui::h_flex;

    if tail.is_empty() {
        return None;
    }
    Some(
        h_flex()
            .id(id)
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap_2()
            .when(!tip.is_empty(), |el| {
                el.tooltip(crate::panels::common::text_tooltip(
                    gpui::SharedString::from(tip),
                ))
            })
            .children(tail.into_iter().enumerate().map(move |(i, text)| {
                div()
                    .id(i)
                    .flex_none()
                    .text_size(text_size)
                    .text_color(rgb(color))
                    .child(text)
            })),
    )
}

/// Build the recovery tooltip from the SAME assembled facts the row is about to render.
///
/// Never a second hand-written string: a tooltip assembled independently is a tooltip that drifts
/// from the row the first time either changes, and the row is exactly where the facts get clipped
/// away. Empty when nothing is hidden, which every render site must gate on — an empty tooltip
/// still paints an empty bubble.
///
/// Args:
///     footer: The split this row is rendering.
///     marker: The same marker [`scope_footer`] was given.
///
/// Returns:
///     Head, then every tail fact, then the closing hint — or an empty string.
pub(crate) fn scope_footer_tooltip(footer: &ScopeFooter, marker: Option<&ScopeMarker>) -> String {
    let Some(marker) = marker else {
        return String::new();
    };
    let mut facts = Vec::with_capacity(footer.tail.len() + 1);
    facts.push(footer.head.clone());
    facts.extend(footer.tail.iter().cloned());
    marker.tooltip(&facts)
}

/// Pick an empty surface's sentence: the hidden-by-preset fact, or the genuine one.
///
/// The two are different facts and must not share a string. "There is nothing here" tells the user
/// to go and make something happen; "the preset is hiding it" tells them the data exists and names
/// the switch that brings it back. A surface that states the first while the second is true sends
/// the user to fix a problem they do not have.
///
/// Args:
///     marker: Scope marker for the surface, or `None` when it is deliberately unscoped.
///     genuine: What this surface says when it really is empty.
///
/// Returns:
///     The shared hidden-by-preset sentence, or `genuine` unchanged.
pub(crate) fn scope_empty_text(marker: Option<&ScopeMarker>, genuine: String) -> String {
    if marker.is_some_and(ScopeMarker::hides_everything) {
        return t!("workspace.scope.all_hidden").to_string();
    }
    genuine
}

#[cfg(test)]
mod tests;
