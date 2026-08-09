// Explicit imports avoid pulling the parent's `gpui::*`, whose `test` shadows the built-in
// attribute and recursively expands `#[test]`.
use super::Period;
use chrono::{TimeZone as _, Utc};

/// Replacing `report::Period::range_at`'s existing-day step with forward-clamping `day_start`
/// makes Apia Yesterday empty on December 31 instead of selecting December 29.
#[test]
fn yesterday_uses_the_previous_existing_day_across_a_dateline_skip() {
    let now = Utc
        .with_ymd_and_hms(2011, 12, 30, 12, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();

    assert_eq!(
        Period::Yesterday.range_at(now, chrono_tz::Pacific::Apia),
        (Some(1_325_152_800), Some(1_325_239_199))
    );
}

/// Group Report keeps its Core shortcut passive in Auto and scopes delayed coin navigation.
///
/// Mutation: restore the Auto writer in `actions.rs` or use unconditional Main navigation in
/// `columns.rs`. A retained transaction row could then select or reveal its previous core.
#[test]
fn report_navigation_cannot_override_current_auto_scope() {
    let actions = include_str!("actions.rs");
    let columns = include_str!("columns.rs");

    assert!(!actions.contains("select_auto_workspace_core"));
    assert!(columns.contains("(!panel.standalone).then(|| panel.group.clone())"));
    assert!(columns.contains("b.open_on_main_if_authorized("));
}
