use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div, px,
};

use super::{
    from_to_text, live_from_to, outcome_cell, outcome_progress_value, outcome_steps_fit,
    version_distribution,
};
use moon_core::feed::{ConnStatus, CoreTimeOffsetStatus, UpdateTarget, named_build_label};
use moon_core::session::core_update::{
    CoreUpdateOutcome, CoreUpdatePhase, CoreUpdateRecord, UpdateFailure,
};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};

use crate::backend::core_warn::LatencySeverity;
use crate::panels::core_status::model::{ApiKeyState, CoreStatusRow};

/// A named build shows as its short label; a name without the `MoonBot-` prefix stays whole.
///
/// Breaks when the prefix compare becomes case-sensitive or the bare prefix is cut to nothing.
#[test]
fn core_update_named_build_label_drops_the_moonbot_prefix() {
    assert_eq!(named_build_label("MoonBot-R2"), "R2");
    assert_eq!(named_build_label("moonbot-F8"), "F8");
    assert_eq!(named_build_label("SynthBuild"), "SynthBuild");
    assert_eq!(named_build_label("MoonBot-"), "MoonBot-");
    assert_eq!(named_build_label("Moon"), "Moon");
}

/// A test build installed on the release's own number names itself in the row, a Release row and
/// an `Unchanged` row do not.
///
/// Breaks when the label is dropped: the row reads `7.71 -> 7.71` and looks like nothing happened.
#[test]
fn core_update_from_to_names_an_installed_test_build() {
    let named = UpdateTarget::Named("MoonBot-R2".to_string());
    assert_eq!(
        from_to_text(Some(771), None, Some(771), None, true, &named),
        "7.71 \u{2192} 7.71 R2"
    );
    assert_eq!(
        from_to_text(
            Some(771),
            None,
            Some(771),
            None,
            true,
            &UpdateTarget::Release
        ),
        "7.71 \u{2192} 7.71"
    );
    assert_eq!(
        from_to_text(Some(771), None, Some(771), None, false, &named),
        "7.71 \u{2192} 7.71"
    );
}

/// A history row prints the letter the outcome stored, and does not append the target label
/// on top of it.
///
/// Breaks when both are printed: the cell reads `7.71 -> 7.71 R3 R3` and names the build twice.
#[test]
fn history_row_prints_the_outcome_letter_once() {
    let named = UpdateTarget::Named("MoonBot-R3".to_string());
    assert_eq!(
        from_to_text(Some(771), None, Some(771), Some("R3"), true, &named),
        "7.71 \u{2192} 7.71 R3"
    );
    assert_eq!(
        from_to_text(
            Some(771),
            None,
            Some(771),
            Some("R2"),
            false,
            &UpdateTarget::Named("MoonBot-R2".to_string())
        ),
        "7.71 \u{2192} 7.71 R2"
    );
}

/// An in-flight Waiting row shows the baseline letter captured at send.
///
/// Breaks when the phase letter is dropped: the cell reads `7.71 -> ...` while the core that
/// left was `7.71 R2`.
#[test]
fn in_flight_waiting_shows_the_baseline_letter() {
    assert_eq!(
        live_from_to(Some(771), Some("R2")),
        "7.71 R2 \u{2192} \u{2026}"
    );
    assert_eq!(live_from_to(Some(771), None), "7.71 \u{2192} \u{2026}");
}

fn core(name: &str, version: Option<u32>, suffix: Option<&str>) -> CoreStatusRow {
    CoreStatusRow {
        id: 1,
        name: name.to_string(),
        status: ConnStatus::Ready,
        sys: CoreSysStatus::default(),
        startup: CoreStartupStatus::default(),
        time_offset: CoreTimeOffsetStatus::default(),
        fault: None,
        mode_suggestion: None,
        endpoint: None,
        ping_warn: false,
        ping_sev: LatencySeverity::Normal,
        exch_sev: LatencySeverity::Normal,
        api_key: ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        api_quota: None,
        api_quota_warn: false,
        server_version: version,
        server_version_suffix: suffix.map(str::to_string),
        version_behind: None,
        update: None,
    }
}

/// Shares have to add up to the fleet. A dropped core, or an unknown bucket that is not last,
/// makes the bar show a different population than the table.
#[test]
fn version_shares_sum_to_the_rows_and_only_the_top_build_is_newest() {
    let rows = [
        core("CORE-A1", Some(771), Some("R3")),
        core("CORE-A2", Some(771), Some("R3")),
        core("CORE-A3", Some(771), Some(" R3 ")),
        core("CORE-A4", Some(770), None),
        core("CORE-A5", None, None),
        core("CORE-A6", None, Some("R9")),
    ];
    let shares = version_distribution(&rows);
    assert_eq!(
        shares.iter().map(|share| share.cores).sum::<usize>(),
        rows.len()
    );
    assert_eq!(shares.len(), 3);
    assert_eq!(shares[0].parts.number, "7.71");
    assert_eq!(shares[0].parts.tag.as_deref(), Some("R3"));
    assert_eq!(shares[0].cores, 3);
    assert!(shares[0].newest);
    assert_eq!(shares[1].parts.number, "7.70");
    assert_eq!(shares[1].parts.tag, None);
    assert_eq!(shares[1].cores, 1);
    assert!(!shares[1].newest);
    assert_eq!(shares[2].parts.number, "-");
    assert_eq!(shares[2].parts.tag, None);
    assert_eq!(shares[2].cores, 2);
    assert!(!shares[2].newest);
}

/// A letter is not a newer build. Both slices on the highest number are newest, the bare
/// number comes before the letter, and a missing letter is the same slice as a blank one.
#[test]
fn the_same_number_with_two_letters_is_one_newest_rank() {
    let rows = [
        core("CORE-A1", Some(771), Some("R3")),
        core("CORE-A2", Some(771), None),
        core("CORE-A3", Some(771), Some("")),
        core("CORE-A4", Some(770), Some("R1")),
    ];
    let shares = version_distribution(&rows);
    assert_eq!(shares.len(), 3);
    assert_eq!(shares[0].parts.number, "7.71");
    assert_eq!(shares[0].parts.tag, None);
    assert_eq!(shares[0].cores, 2);
    assert!(shares[0].newest);
    assert_eq!(shares[1].parts.tag.as_deref(), Some("R3"));
    assert_eq!(shares[1].cores, 1);
    assert!(shares[1].newest);
    assert!(!shares[2].newest);
}

/// No rows is an empty bar, not a fake unknown slice.
#[test]
fn no_rows_have_no_version_shares() {
    assert!(version_distribution(&[]).is_empty());
}

/// The production outcome column is 220 wide and the standard row is 25 tall. Horizontal Xs
/// steps wrap there and do not fit; a 600-wide column holds one row of steps.
#[test]
fn horizontal_steps_fit_only_when_the_column_holds_one_row() {
    assert!(!outcome_steps_fit(220.0, 25.0));
    assert!(outcome_steps_fit(600.0, 25.0));
    assert!(!outcome_steps_fit(119.0, 25.0));
}

/// Progress tracks the phase index over four live steps. Done sits past the last step.
#[test]
fn outcome_progress_follows_the_phase_index() {
    let queued = CoreUpdatePhase::Queued {
        lane: "127.0.0.1".parse().unwrap(),
        held: false,
        not_ready_since: None,
    };
    assert_eq!(outcome_progress_value(&queued), 0.0);
    assert_eq!(
        outcome_progress_value(&CoreUpdatePhase::Done(CoreUpdateOutcome::Failed(
            UpdateFailure::Timeout
        ))),
        125.0
    );
}

/// A scope whose only reported build is older than the fleet target it is behind must not
/// paint that older slice as newest.
#[test]
fn a_scope_behind_the_fleet_target_flags_no_slice_newest() {
    let mut lagging = core("CORE-A1", Some(790), None);
    lagging.version_behind = Some(800);
    let shares = version_distribution(&[lagging]);
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].parts.number, "7.90");
    assert!(!shares[0].newest);
}

/// The production outcome cell, which is the progress fallback at 220 by 25.
struct OutcomeProbe {
    /// Phase drawn in the cell.
    phase: CoreUpdatePhase,
}

impl Render for OutcomeProbe {
    /// Host the production cell in the standard row's box.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(220.0))
            .h(px(25.0))
            .overflow_hidden()
            .debug_selector(|| "cs-outcome-host".into())
            .child(outcome_cell(0, &self.phase))
    }
}

/// `cs-update-outcome-0` stays inside a 220 by 25 host. Steps wrap past that height.
#[gpui::test]
fn outcome_cell_stays_inside_the_standard_row(cx: &mut gpui::TestAppContext) {
    super::super::layout_fixture::install_dark(cx);
    let phase = CoreUpdatePhase::Queued {
        lane: "127.0.0.1".parse().unwrap(),
        held: false,
        not_ready_since: None,
    };
    let mut visual =
        super::super::layout_fixture::open(cx, 220.0, 25.0, move |_, _| OutcomeProbe { phase });
    super::super::layout_fixture::draw(&mut visual);
    let host = visual.debug_bounds("cs-outcome-host").expect("host");
    let cell = visual
        .debug_bounds("cs-update-outcome-0")
        .expect("outcome cell");
    assert!(
        super::super::layout_fixture::inside(cell, host),
        "outcome cell left the 220x25 host: {cell:?} / {host:?}"
    );
    assert!(
        super::super::layout_fixture::height_of(cell) <= 25.5,
        "outcome cell taller than the row: {cell:?}"
    );
}

fn history_record(outcome: CoreUpdateOutcome) -> CoreUpdateRecord {
    CoreUpdateRecord {
        core: 7,
        core_name: "core".to_string(),
        lane_addr: "127.0.0.1".parse().unwrap(),
        from: Some(771),
        from_suffix: None,
        started_ms: 1,
        ended_ms: 2,
        target: UpdateTarget::Release,
        outcome,
    }
}

/// An older failure on a core that has since succeeded must not offer retry. The newest
/// failure offers it only while the core's current phase is still `Done(Failed)`.
#[test]
fn offer_history_retry_requires_the_newest_failure_still_failed() {
    let failed = history_record(CoreUpdateOutcome::Failed(UpdateFailure::Rejected));
    let succeeded = history_record(CoreUpdateOutcome::Succeeded {
        from: Some(770),
        to: 771,
        to_suffix: None,
    });
    let done_ok = CoreUpdatePhase::Done(CoreUpdateOutcome::Succeeded {
        from: Some(770),
        to: 771,
        to_suffix: None,
    });
    let done_bad = CoreUpdatePhase::Done(CoreUpdateOutcome::Failed(UpdateFailure::Rejected));
    assert!(!super::offer_history_retry(&failed, false, Some(&done_ok)));
    assert!(super::offer_history_retry(&failed, true, Some(&done_bad)));
    assert!(!super::offer_history_retry(&failed, true, Some(&done_ok)));
    assert!(!super::offer_history_retry(
        &succeeded,
        true,
        Some(&done_bad)
    ));
    assert!(!super::offer_history_retry(&failed, true, None));
}
