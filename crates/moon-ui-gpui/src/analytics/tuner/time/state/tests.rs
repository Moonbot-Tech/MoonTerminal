//! Generation tests for By-time scope and report invalidation.

use super::TimeTunerState;
use super::{fmt_min, fmt_week_ep};

/// A settled profile fixture uses the same complete source identity as the production loader.
fn ready_profile() -> (
    TimeTunerState,
    crate::analytics::cache::CacheKey,
    std::time::Instant,
) {
    use crate::analytics::cache::{CacheKey, CacheRevisions};
    use crate::load_state::LoadState;
    use moon_core::db::analytics::Query;
    use moon_core::db::tuner::{SliderProfiles, VarStats};
    use std::sync::Arc;

    let key = CacheKey::strategies(
        &Query::default(),
        None,
        CacheRevisions {
            read_state: Some(1),
            valuation_attachment: Some(1),
            ..Default::default()
        },
    );
    let at = std::time::Instant::now();
    let mut state = TimeTunerState::load();
    state.dirty = false;
    state.profiles = LoadState::Ready(Arc::new(Vec::new()));
    state.slider = LoadState::Ready(Arc::new(SliderProfiles {
        week: [0.0; 168],
        day: [0.0; 1440],
        hour: [0.0; 60],
        entry_hours: Default::default(),
    }));
    state.stats = LoadState::Ready(Arc::new(vec![VarStats::default()]));
    state.profile_basis = Some((key.clone(), at));
    (state, key, at)
}

/// A new source, failed surface or stale observation must take the complete profile loader.
#[test]
fn draft_rescore_requires_complete_current_certified_profiles() {
    use crate::analytics::cache::{CacheKey, CacheRevisions};
    use crate::load_state::LoadState;
    use moon_core::db::analytics::Query;

    let (mut state, key, at) = ready_profile();
    assert!(state.can_rescore(&key, at));
    for revisions in [
        CacheRevisions {
            reports: 1,
            read_state: Some(1),
            valuation_attachment: Some(1),
            ..Default::default()
        },
        CacheRevisions {
            valuation: 1,
            read_state: Some(1),
            valuation_attachment: Some(1),
            ..Default::default()
        },
        CacheRevisions {
            strategies: 1,
            read_state: Some(1),
            valuation_attachment: Some(1),
            ..Default::default()
        },
        CacheRevisions::default(),
    ] {
        let changed = CacheKey::strategies(&Query::default(), None, revisions);
        assert!(!state.can_rescore(&changed, at));
    }
    let shifted = CacheKey::strategies(
        &Query {
            to: 86400,
            ..Default::default()
        },
        None,
        CacheRevisions {
            read_state: Some(1),
            valuation_attachment: Some(1),
            ..Default::default()
        },
    );
    assert!(!state.can_rescore(&shifted, at));
    state.dirty = true;
    assert!(!state.can_rescore(&key, at));
    state.dirty = false;
    state.slider = LoadState::NotReady;
    assert!(!state.can_rescore(&key, at));
    let (mut state, key, at) = ready_profile();
    state.profiles.begin();
    assert!(!state.can_rescore(&key, at));
    let (mut state, key, at) = ready_profile();
    state.profile_basis = None;
    assert!(!state.can_rescore(&key, at));
}

/// Repeated KPI edits do not renew the profiles' sixty-second certification window.
#[test]
fn draft_rescore_expiry_uses_original_profile_observation() {
    use std::time::Duration;
    let (mut state, key, at) = ready_profile();
    assert!(state.can_rescore(&key, at + Duration::from_secs(60)));
    state.begin_draft_rescore();
    assert!(!state.can_rescore(&key, at + Duration::from_millis(60001)));
}

/// A second edit replaces pending KPIs without resampling profiles or the write baseline.
#[test]
fn rapid_draft_rescores_keep_profiles_and_cancel_superseded_kpis() {
    use crate::analytics::bg::{LatestReads, ReadLane};
    use crate::load_state::LoadState;
    use std::sync::Arc;

    let (mut state, key, at) = ready_profile();
    let profiles = state.profiles.data().unwrap().clone();
    let slider = state.slider.data().unwrap().clone();
    state.current_raw = ["1-5".into(), "08:00-20:00".into()];
    state.ignore_staged = Some(true);
    let mut reads = LatestReads::default();
    let first_seq = state.begin_draft_rescore();
    let first = reads.replace(&[ReadLane::Time]);
    assert!(state.can_rescore(&key, at));
    let second_seq = state.begin_draft_rescore();
    let second = reads.replace(&[ReadLane::Time]);
    assert_ne!(first_seq, second_seq);
    assert!(first.is_cancelled());
    assert!(!second.is_cancelled());
    assert!(matches!(state.stats, LoadState::Loading { .. }));
    assert!(Arc::ptr_eq(state.profiles.data().unwrap(), &profiles));
    assert!(Arc::ptr_eq(state.slider.data().unwrap(), &slider));
    assert_eq!(state.current_raw, ["1-5", "08:00-20:00"]);
    assert_eq!(state.ignore_staged, Some(true));
    assert_eq!(state.profile_basis.as_ref().unwrap().1, at);
}

/// A day wrap or symmetric endpoint shortening changes the strategy schedule input text.
#[test]
fn time_inputs_keep_unwrapped_hours_and_asymmetric_week_boundaries() {
    for (minutes, expected) in [
        (0, "00:00"),
        (61, "01:01"),
        (1439, "23:59"),
        (1440, "24:00"),
    ] {
        assert_eq!(fmt_min(minutes), expected);
    }
    for (minute, is_end, expected) in [
        (0, false, "1"),
        (1439, true, "1"),
        (1424, false, "1.23:44"),
        (10079, true, "7"),
        (1440, true, "2.00:00"),
    ] {
        assert_eq!(fmt_week_ep(minute, is_end), expected);
    }
}

/// `time/state.rs:TimeTunerState::invalidate` must advance both request generations; removing the
/// `seq` bump lets a pre-scope profile completion clear `dirty` under the newly selected strategy.
#[test]
fn scope_change_retires_profile_and_suggestion_requests() {
    let mut state = TimeTunerState::load();
    let (seq, sugg_seq) = (state.seq, state.sugg_seq);

    state.invalidate();

    assert_ne!(state.seq, seq);
    assert_ne!(state.sugg_seq, sugg_seq);
    assert!(state.dirty);
}

/// `time/state.rs:TimeTunerState::mark_report_stale` retires suggestions but keeps profile state.
///
/// Removing the suggestion-generation bump lets an optimizer pinned before a report commit write
/// a stale schedule into saveable v1 fields after the report inputs have changed.
#[test]
fn report_change_retires_suggestion_but_keeps_profile_generation() {
    let mut state = TimeTunerState::load();
    let (seq, sugg_seq) = (state.seq, state.sugg_seq);

    state.mark_report_stale();

    assert_eq!(state.seq, seq);
    assert_ne!(state.sugg_seq, sugg_seq);
    assert!(state.dirty);
}

/// `time/state.rs:TimeTunerState::apply_current_read` must preserve a confirmed Save baseline
/// when an automatic refresh cannot read the strategy row; clearing it would make the next Save
/// propose empty schedules and false ignore-flag changes.
#[test]
fn automatic_missing_schedule_read_preserves_the_save_baseline() {
    let mut state = TimeTunerState::load();
    state.apply_current_read(
        Some((["1.00:00-5.23:59".into(), "08:00-20:00".into()], true, true)),
        false,
    );

    state.apply_current_read(None, true);

    assert_eq!(state.current_raw[0], "1.00:00-5.23:59");
    assert_eq!(state.current_raw[1], "08:00-20:00");
    assert!(state.ignore_cur);
    assert!(state.ign_filters_cur);
}
