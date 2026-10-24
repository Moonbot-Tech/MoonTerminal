//! The queues hold ids, not clones: what they plan with and where they route must stay what the
//! per-entry clones and the per-tick routing walk gave.

use super::*;

/// A live close's `protect` overwrites the protection set, never the version already queued.
#[test]
fn protect_after_listing_keeps_the_queued_version() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(1825, true);
    let shape = catchup::Shape {
        margin_ms: 0,
        long_ms: DAY,
        tail_ms: 0,
    };
    let listed = row();
    let mut live = row();
    live.open_ms -= 30_000;
    let mut state = RecorderCatchup::default();
    state.submit(vec![listed.clone()]);
    state.protect(vec![live.clone()]);
    assert_eq!(
        state.known_trade(&listed.id).map(|t| t.open_ms),
        Some(live.open_ms),
        "retention protects the version written last"
    );
    let batch = state.prepare(|_| Coverage::none(), NOW, &shape, true);
    assert_eq!(batch.jobs.len(), 1);
    assert_eq!(batch.jobs[0].trade.open_ms, listed.open_ms);
    // A later listing makes the entry read the protection set again.
    state.submit(vec![listed.clone()]);
    assert!(matches!(state.pending.get(&listed.id), Some(None)));
}

/// A skip rule that moves without a settings generation still routes the queues on the next
/// call, as the walk on every call did.
#[test]
fn a_rule_change_routes_without_a_generation_bump() {
    use std::sync::atomic::Ordering;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(1825, true);
    let mut emulated = row();
    emulated.emulator = true;
    let mut state = RecorderCatchup::default();
    state.submit(vec![emulated.clone()]);
    assert!(state.pending.contains_key(&emulated.id));
    catchup::RECORD_EMU.store(false, Ordering::Relaxed);
    state.sync_switches();
    assert!(!state.pending.contains_key(&emulated.id));
    assert!(state.skipped.contains_key(&emulated.id));
    catchup::RECORD_EMU.store(true, Ordering::Relaxed);
    state.sync_switches();
    assert!(state.pending.contains_key(&emulated.id));
    assert!(!state.skipped.contains_key(&emulated.id));
}

/// A settings change refills the queue from the protection set and routes the skipped rows
/// out again, so an emulator row does not linger in the fetch queue.
#[test]
fn a_refill_routes_skipped_rows_again() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(1825, false);
    let mut emulated = row();
    emulated.emulator = true;
    let mut state = RecorderCatchup::default();
    state.submit(vec![emulated.clone()]);
    assert!(state.skipped.contains_key(&emulated.id));
    catchup::window_changed();
    state.sync_switches();
    assert!(!state.pending.contains_key(&emulated.id));
    assert!(state.skipped.contains_key(&emulated.id));
}

/// A skipped row overwritten in the protection set keeps the version it was listed with, so the
/// rule admitting it again queues that version, as the skipped clone did.
#[test]
fn protect_keeps_the_version_a_skipped_row_was_listed_with() {
    use std::sync::atomic::Ordering;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(1825, false);
    let shape = catchup::Shape {
        margin_ms: 0,
        long_ms: DAY,
        tail_ms: 0,
    };
    let mut listed = row();
    listed.emulator = true;
    let mut live = listed.clone();
    live.emulator = false;
    live.open_ms -= 30_000;
    let mut state = RecorderCatchup::default();
    state.submit(vec![listed.clone()]);
    assert!(state.skipped.contains_key(&listed.id));
    state.protect(vec![live.clone()]);
    catchup::RECORD_EMU.store(true, Ordering::Relaxed);
    let batch = state.prepare(|_| Coverage::none(), NOW, &shape, true);
    assert_eq!(batch.jobs.len(), 1);
    assert_eq!(batch.jobs[0].trade.open_ms, listed.open_ms);
    assert!(batch.jobs[0].trade.emulator);
}

/// Switches `tuner_only` back off however the test ends.
struct TunerOnlyOff;

impl Drop for TunerOnlyOff {
    /// Leave the process default for the fixtures that follow.
    fn drop(&mut self) {
        catchup::TUNER_ONLY.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The `tuner_only` half of the skip rule routes the queues like the emulator half.
#[test]
fn a_tuner_only_change_routes_without_a_generation_bump() {
    use std::sync::atomic::Ordering;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    let _off = TunerOnlyOff;
    catchup::set_keep(1825, true);
    let mut untunable = row();
    untunable.tunable = false;
    let mut state = RecorderCatchup::default();
    state.submit(vec![untunable.clone()]);
    assert!(state.pending.contains_key(&untunable.id));
    catchup::TUNER_ONLY.store(true, Ordering::Relaxed);
    state.sync_switches();
    assert!(!state.pending.contains_key(&untunable.id));
    assert!(state.skipped.contains_key(&untunable.id));
    catchup::TUNER_ONLY.store(false, Ordering::Relaxed);
    state.sync_switches();
    assert!(state.pending.contains_key(&untunable.id));
}

/// A row the protection set drops at the retention edge is handed over once, so the deal-chart
/// map can serve the rest of its hour; a row the queue still holds stays known.
#[test]
fn prune_hands_over_the_rows_it_drops() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(1825, false);
    let mut skipped = row();
    skipped.emulator = true;
    let mut queued = row();
    queued.id.rec_id += 1;
    let mut state = RecorderCatchup::default();
    state.submit(vec![skipped.clone(), queued.clone()]);
    let past_edge = skipped.close_ms + 1825 * DAY + 1;
    state.prune(past_edge);
    let dropped: Vec<_> = state.take_expired().into_iter().map(|t| t.id).collect();
    assert_eq!(dropped, vec![skipped.id]);
    assert!(state.take_expired().is_empty());
    assert!(state.known_trade(&queued.id).is_some());
}
