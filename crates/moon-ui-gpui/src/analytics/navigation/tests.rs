//! Tab-priority cancellation and shared scheduling contracts.

use std::time::Instant;

use crate::analytics::bg::{LatestReads, ReadLane};
use crate::analytics::refresh::{RefreshGate, RefreshPlan, RefreshUrgency};

/// A tab change stops its hidden base scan while retaining explicit user search ownership.
#[test]
fn hidden_base_cancellation_preserves_search_and_waits_for_acknowledgment() {
    let mut reads = LatestReads::default();
    let base = reads.replace(&[ReadLane::Summary]);
    let search = reads.replace(&[ReadLane::TicksSearch]);
    reads.cancel(&[ReadLane::Summary]);
    assert!(base.is_cancelled());
    assert!(!search.is_cancelled());

    let now = Instant::now();
    let mut gate = RefreshGate::new(0, now);
    gate.request_refresh(now, false, RefreshUrgency::User);
    assert_eq!(gate.plan(now, true), RefreshPlan::Idle);
    reads.finish(&base);
    assert_eq!(
        gate.plan(now, true),
        RefreshPlan::Idle,
        "an explicit search or write must retain the database interlock"
    );
    reads.finish(&search);
    assert_eq!(
        gate.plan(now, false),
        RefreshPlan::Now {
            show_overlay: false
        },
        "the selected tab must start without the writer's quiet delay"
    );
}

/// Root tabs and mode changes share ordinary-axis retirement without destructive draft resets.
#[test]
fn tab_handler_uses_nondestructive_ordinary_read_priority() {
    let toolbar = include_str!("../toolbar.rs");
    assert!(toolbar.contains("this.activate_tab(t, window, cx);"));
    let navigation = include_str!("../navigation.rs");
    let activation = navigation
        .split_once("fn activate_tab(")
        .expect("tab activation")
        .1
        .split_once("fn retire_hidden_reads(")
        .expect("hidden ordinary-read retirement")
        .0;
    let retire = activation
        .find("self.retire_hidden_reads();")
        .expect("cancel old tab");
    let select = activation.find("self.tab = tab;").expect("select new tab");
    assert!(retire < select);
    assert!(activation.contains("if needs_refresh && !request_owned {"));
    assert!(activation.contains("if tab == Tab::Strategies && !self.strategy_dirty {"));
    let ownership = activation
        .find("let request_owned = self.restore_or_join_snapshot(&request, cx);")
        .expect("joined prefetch owns navigation");
    let queue = activation
        .find("if needs_refresh && !request_owned {")
        .expect("only unowned navigation is queued");
    assert!(ownership < queue);
    let owned = activation
        .split_once("if needs_refresh && !request_owned {")
        .expect("navigation ownership gate")
        .1
        .split_once("} else {")
        .expect("owned navigation branch")
        .1;
    let acknowledge = owned
        .find("self.acknowledge_navigation_refresh(request.report_generation, cx);")
        .expect("joined request consumes pending navigation");
    let axis = owned
        .find("self.request_axis_if_stale(self.strat_mode, cx);")
        .expect("only a published base can chain its axis");
    assert!(acknowledge < axis);
    let publication = activation
        .find("self.observe_report_publication(request.report_generation);")
        .expect("captured publication invalidates every dependent page");
    assert!(
        publication < ownership,
        "fresh restoration must only clear its own page"
    );
    assert!(activation.contains("self.request_report_refresh(RefreshUrgency::User, true, cx);"));
    let retirement = navigation
        .split_once("fn retire_hidden_reads(")
        .expect("hidden ordinary-read retirement")
        .1;
    let old_tab = retirement
        .split_once("if self.tab == Tab::Strategies {")
        .expect("only the old Strategies tab owns the axis being left")
        .1
        .split_once('}')
        .expect("old-tab guard")
        .0;
    assert!(old_tab.contains("self.retire_hidden_tuner_reads();"));
    let tuner = include_str!("../tuner/mod.rs");
    let mode_change = tuner
        .split_once("fn set_strat_mode(")
        .expect("axis mode change")
        .1
        .split_once("\n    }")
        .expect("axis mode boundary")
        .0;
    let mode_retire = mode_change
        .find("self.retire_hidden_tuner_reads();")
        .expect("same preserved-work retirement as root-tab navigation");
    assert!(mode_retire < mode_change.find("self.strat_mode = mode;").unwrap());
    for sequence in ["self.summary_seq", "self.strategy_seq", "self.cal_seq"] {
        assert!(
            retirement.contains(sequence),
            "cancelled {sequence} must reject its old completion"
        );
    }
    for destructive in [
        "self.reload(cx)",
        "cancel_axis_reads",
        ".invalidate()",
        "TicksSearch",
    ] {
        assert!(
            !navigation.contains(destructive),
            "tab changes must preserve {destructive}"
        );
    }
}

/// Both notification and navigation must invalidate hidden pages and axes on a newly seen commit.
#[test]
fn navigation_publication_observation_marks_every_dependent_surface() {
    let observer = include_str!("../observe.rs");
    let observation = observer
        .split_once("fn observe_report_publication(")
        .expect("shared publication observation")
        .1
        .split_once("fn mark_report_data_stale(")
        .expect("shared publication invalidation")
        .0;
    assert!(observation.contains(".observe_generation(generation,"));
    assert!(observation.contains("self.mark_report_data_stale();"));
    let invalidation = observer
        .split_once("fn mark_report_data_stale(")
        .expect("publication invalidation")
        .1
        .split_once("fn acknowledge_report_refresh(")
        .expect("invalidation boundary")
        .0;
    for dependent in [
        "self.data_dirty = true;",
        "self.strategy_dirty = true;",
        "self.cal_dirty = true;",
        "self.tuner.mark_report_stale();",
        "self.time_tuner.mark_report_stale();",
        "self.coins.mark_report_stale();",
        "self.ticks.mark_report_stale();",
    ] {
        assert!(
            invalidation.contains(dependent),
            "publication must invalidate {dependent}"
        );
    }
    let settle = observer
        .split_once("fn acknowledge_navigation_refresh(")
        .expect("navigation acknowledgement")
        .1
        .split_once("fn settle_report_refresh_retry(")
        .expect("navigation acknowledgement boundary")
        .0;
    assert!(settle.contains("self.report_refresh.navigation_settled("));
    assert!(settle.contains("self.current_report_generation()"));
    assert!(settle.contains("self.mark_report_data_stale();"));
    assert!(settle.contains("self.schedule_report_refresh(cx);"));
}

/// A commit between navigation and its queued read must invalidate hidden axes before routing.
#[test]
fn delayed_refresh_observes_its_publication_before_acknowledgement_and_route_selection() {
    let observer = include_str!("../observe.rs");
    let acknowledge = observer
        .split_once("fn acknowledge_report_refresh(")
        .expect("foreground refresh acknowledgement")
        .1
        .split_once("fn acknowledge_navigation_refresh(")
        .expect("foreground acknowledgement boundary")
        .0;
    let observed = acknowledge
        .find("self.observe_report_publication(generation);")
        .expect("newly covered commits must invalidate every dependent surface");
    let acknowledged = acknowledge
        .find(".refresh_started(generation,")
        .expect("acknowledgement of the same captured publication");
    assert!(observed < acknowledged);

    let dispatch = observer
        .split_once("fn refresh_visible_report_data(")
        .expect("deferred visible dispatch")
        .1
        .split_once("fn active_period(")
        .expect("visible dispatch boundary")
        .0;
    let acknowledge = dispatch
        .find("self.acknowledge_report_refresh();")
        .expect("dispatch must observe the publication being covered");
    let route = dispatch
        .find("let base_dirty = match self.tab")
        .expect("base freshness chooses axis-only or base-and-axis dispatch");
    assert!(acknowledge < route);
}
