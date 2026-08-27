//! Unit tests for resolving optional Strategies display preferences.

use moon_core::config::layout::WindowLayout;

use super::{
    ACTIVE_ONLY, GROUP_BY_VENUE, POPUP_ROWS, PREF_ROWS, StrategiesPrefs,
    settings_content_width_value,
};

/// Return the brace-balanced implementation body for one unique source signature.
///
/// Args:
///     source: Rust source containing the target function.
///     signature: Unique prefix that starts the target function.
///
/// Returns:
///     The source slice from `signature` through its matching closing brace.
///
/// Panics:
///     When the signature or either matching brace is absent.
fn braced_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("expected `{signature}` in settings source"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("expected opening brace after `{signature}`"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("expected closing brace for `{signature}`");
}

/// `POPUP_ROWS` is a hand-written subset of `PREF_ROWS`, so nothing in the type system stops it
/// from listing a row that is edited elsewhere — which would give one preference two controls — or
/// from going empty, which renders the group as a caption over an empty frame. Persistence covers
/// the full set either way, and a row absent from BOTH lists would be saved but uneditable.
#[test]
fn the_popup_renders_a_non_empty_subset_that_excludes_the_filter_row_toggle() {
    assert!(
        !POPUP_ROWS.is_empty(),
        "an empty popup group renders a caption over an empty frame"
    );
    assert!(
        !POPUP_ROWS.iter().any(|row| row.id == ACTIVE_ONLY.id),
        "active-only is edited from the filter row, never from the popup"
    );
    for row in &POPUP_ROWS {
        assert!(
            PREF_ROWS.iter().any(|known| known.id == row.id),
            "{} is rendered but never restored or persisted",
            row.id
        );
    }
}

/// `StrategiesPrefs::default` changing active-only to ON would hide unchecked strategies after an
/// upgrade, while changing grouping to OFF would flatten the established exchange hierarchy.
#[test]
fn absent_preferences_keep_the_shipped_tree_defaults() {
    let restored = StrategiesPrefs::restore(&WindowLayout::default());
    assert!(restored.group_by_venue, "venue grouping must ship enabled");
    assert!(!restored.active_only, "active-only must ship disabled");
}

/// `StrategiesPrefs::restore` stamping one absent key from its saved neighbor would make editing a
/// single checkbox silently freeze both defaults for future launches.
#[test]
fn saved_preferences_restore_independently() {
    let mut grouped_only = WindowLayout::default();
    (GROUP_BY_VENUE.store)(&mut grouped_only, false);
    let restored = StrategiesPrefs::restore(&grouped_only);
    assert!(!restored.group_by_venue);
    assert!(
        !restored.active_only,
        "the absent active-only key keeps its default"
    );

    let mut active_only = WindowLayout::default();
    (ACTIVE_ONLY.store)(&mut active_only, true);
    let restored = StrategiesPrefs::restore(&active_only);
    assert!(
        restored.group_by_venue,
        "the absent grouping key keeps its default"
    );
    assert!(restored.active_only);

    let mut explicit = WindowLayout::default();
    (GROUP_BY_VENUE.store)(&mut explicit, false);
    (ACTIVE_ONLY.store)(&mut explicit, true);
    assert_eq!(
        StrategiesPrefs::restore(&explicit),
        StrategiesPrefs {
            group_by_venue: false,
            active_only: true,
            tree_text_step: 0.0,
            params_full: false,
        }
    );
}

/// `settings.rs::set_params_full` must not clear `field_edits`: a mode switch is presentation-only
/// and clearing staged edits would silently discard a draft the user still expects to apply.
#[test]
fn switching_parameters_mode_only_persists_its_preference() {
    let settings = include_str!("../settings.rs");
    let set_params_full = braced_body(settings, "pub(super) fn set_params_full(");
    let write_pref = braced_body(settings, "fn write_pref(");

    assert!(
        set_params_full.contains("self.write_pref(&PARAMS_FULL, value, cx);"),
        "the mode switch must delegate to the one preference writer"
    );
    for state in [
        "field_edits",
        "field_inputs",
        "field_memos",
        "field_colors",
        "focused_field",
        "last_edit_note_seq",
    ] {
        assert!(
            !set_params_full.contains(state) && !write_pref.contains(state),
            "switching presentation modes must retain {state}, including staged drafts"
        );
    }

    let params = include_str!("../params.rs");
    let field_row = braced_body(params, "pub(super) fn field_row(");
    assert!(
        field_row.contains("let row_id = editor_state_id(keys, &field_name);"),
        "both modes must retain editor state under the mode-independent key derived from keys and field"
    );
}

/// `settings.rs::settings_content_width_value` must keep the group frame inset inside only the
/// group-content arm; adding it after the outer maximum widens a title-only popup, while dropping
/// it clips the tree-text-step label beside its stepper.
#[test]
fn settings_popup_width_keeps_title_and_stepper_bounds_independent() {
    let title_only_width = settings_content_width_value(100.0, 40.0, 20.0, 10.0, 30.0, 20.0, 12.0);
    let stepper_bound_width =
        settings_content_width_value(50.0, 30.0, 20.0, 10.0, 60.0, 30.0, 12.0);

    assert_eq!(
        [title_only_width, stepper_bound_width],
        [100.0, 102.0],
        "the title remains a maximum while the stepper row includes the group frame inset"
    );
}
