//! Regressions for the shared scope-marker facts and fixed-footer assembly.

use moon_core::config::WorkspaceMode;
use rust_i18n::t;

use super::{ScopeMarker, scope_empty_text, scope_footer, scope_footer_tooltip};

/// An unresolved singleton scope means show everything, so it creates no marker.
///
/// Plausible breakage: dropping `from_membership`'s `preset?` guard would create a marker for an
/// unresolved focus.
#[test]
fn from_membership_returns_none_without_a_preset() {
    assert!(ScopeMarker::from_membership(None, [true, false]).is_none());
}

/// A membership marker counts hidden cores in its caller's complete universe.
///
/// Plausible breakage: not incrementing `configured` for a hidden core would turn this into two
/// shown cores out of two instead of two out of three.
#[test]
fn from_membership_counts_shown_against_the_whole_universe() {
    let marker = ScopeMarker::from_membership(Some(WorkspaceMode::Classic), [true, false, true])
        .expect("a resolved preset produces a marker");

    assert!(marker.hides_anything());
    let facts = marker.facts();
    assert_eq!(facts.len(), 2);
    assert_eq!(
        facts[1],
        format!("· {}", t!("workspace.scope.cores_n_of_m", n = 2, total = 3))
    );
}

/// Only an actual non-empty universe can be wholly hidden by a preset.
///
/// Plausible breakage: removing the `configured > 0` guard would call an all-disconnected
/// terminal preset-hidden.
#[test]
fn hides_everything_requires_a_nonempty_universe() {
    assert!(!ScopeMarker::new(Some(WorkspaceMode::Classic), 0, 0).hides_everything());
    assert!(ScopeMarker::new(Some(WorkspaceMode::Classic), 0, 3).hides_everything());
    assert!(!ScopeMarker::new(Some(WorkspaceMode::Classic), 1, 3).hides_everything());
}

/// A full scope leaves both the row facts and recovery hover exactly absent.
///
/// Plausible breakage: removing either `hides_anything` early return adds a marker or empty hover
/// to an otherwise unchanged surface.
#[test]
fn a_full_scope_states_nothing_and_offers_no_hint() {
    let marker = ScopeMarker::new(Some(WorkspaceMode::Classic), 3, 3);
    let unrelated_tail = vec!["unrelated fact".to_string()];

    assert!(marker.facts().is_empty());
    assert_eq!(marker.tooltip(&unrelated_tail), "");
}

/// Fixed-height footer figures remain ahead of the clipping marker tail.
///
/// Plausible breakage: concatenating head and marker facts would let a narrow dock clip the
/// figures that the row exists to state.
#[test]
fn the_footer_head_never_joins_the_clipping_tail() {
    let marker = ScopeMarker::new(Some(WorkspaceMode::AutoTrading), 1, 2);
    let head = "figures".to_string();
    let expected_tail = marker.facts();

    let footer = scope_footer(head.clone(), Some(&marker));

    assert_eq!(footer.head, head);
    assert_eq!(footer.tail, expected_tail);
}

/// Footer assembly has no clipping tail when no scope exclusion exists.
///
/// Plausible breakage: bypassing `ScopeMarker::facts` would add marker text for a full scope or an
/// absent marker.
#[test]
fn the_footer_tail_is_empty_when_nothing_is_hidden() {
    let full_scope = ScopeMarker::new(Some(WorkspaceMode::Classic), 2, 2);

    assert!(
        scope_footer("figures".to_string(), Some(&full_scope))
            .tail
            .is_empty()
    );
    assert!(scope_footer("figures".to_string(), None).tail.is_empty());
}

/// The hover preserves the rendered facts in row order before the recovery hint.
///
/// Plausible breakage: hand-writing a tooltip independently from the assembled footer loses a
/// row fact, reorders it, or omits the closing hint.
#[test]
fn the_footer_tooltip_repeats_the_row_then_closes_with_the_hint() {
    let marker = ScopeMarker::new(Some(WorkspaceMode::AutoTrading), 1, 3);
    let footer = scope_footer("figures".to_string(), Some(&marker));
    let tooltip = scope_footer_tooltip(&footer, Some(&marker));

    assert!(tooltip.starts_with(&footer.head));
    let mut previous_position = 0;
    for fact in &footer.tail {
        let position = tooltip
            .find(fact)
            .expect("the tooltip retains every rendered footer fact");
        assert!(
            position > previous_position,
            "footer facts stay in row order"
        );
        previous_position = position;
    }
    assert!(tooltip.ends_with(t!("workspace.scope.hint").as_ref()));
}

/// A hover is absent unless a marker is actively hiding a scope.
///
/// Plausible breakage: dropping either gate attaches an empty tooltip bubble to an unscoped or
/// full-scope footer.
#[test]
fn the_footer_tooltip_is_empty_without_a_hiding_marker() {
    let full_scope = ScopeMarker::new(Some(WorkspaceMode::Classic), 2, 2);
    let full_footer = scope_footer("figures".to_string(), Some(&full_scope));
    let unscoped_footer = scope_footer("figures".to_string(), None);

    assert_eq!(scope_footer_tooltip(&full_footer, Some(&full_scope)), "");
    assert_eq!(scope_footer_tooltip(&unscoped_footer, None), "");
}

/// Only a completely hidden non-empty scope replaces a genuine empty-state sentence.
///
/// Plausible breakage: loosening the predicate to `hides_anything` makes a partially narrowed
/// surface claim that every core is hidden.
#[test]
fn the_empty_text_switches_only_when_every_core_is_hidden() {
    let genuine = t!("screener.empty_no_data").to_string();
    let all_hidden = ScopeMarker::new(Some(WorkspaceMode::AutoTrading), 0, 5);
    let partially_hidden = ScopeMarker::new(Some(WorkspaceMode::AutoTrading), 2, 5);

    assert_eq!(
        scope_empty_text(Some(&all_hidden), genuine.clone()),
        t!("workspace.scope.all_hidden")
    );
    assert_eq!(
        scope_empty_text(Some(&partially_hidden), genuine.clone()),
        genuine
    );
    assert_eq!(scope_empty_text(None, genuine.clone()), genuine);
}
