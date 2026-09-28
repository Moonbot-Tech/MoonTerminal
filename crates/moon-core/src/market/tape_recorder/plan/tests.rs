use super::*;

const MARGIN: i64 = 30_000;
const LONG: i64 = 300_000;
const T0: i64 = 1_800_000_000_000;
const ID: TradeId = TradeId { core: 1, rec_id: 7 };

#[test]
fn an_open_asks_at_once_and_files_the_run_up_up_to_the_last_second() {
    let mut task = KeyTask::new();
    task.opened(T0 + 2_000, ID, T0, MARGIN, LONG);
    assert_eq!(task.due_ms(), Some(T0 + 2_000));

    // A hot ring: two minutes deep, newest print a moment ago.
    let now = T0 + 3_000;
    let filing = task.on_answer(now, Some((now - 120_200, now - 200)));
    assert_eq!(
        filing.file.spans(),
        &[(T0 - MARGIN, now - 200 - LIVE_TAIL_MS)]
    );
    assert!(filing.lost.is_empty());
    // Depth 120 s -> a 12 s step, well before the position could turn long.
    assert_eq!(task.due_ms(), Some(now + 12_000));
}

#[test]
fn a_quiet_market_is_filed_as_silent_up_to_shortly_before_the_ask() {
    let mut task = KeyTask::new();
    task.closed(T0 + 40_000, ID, T0, T0 + 5_000, MARGIN, LONG);
    // The last print was ten minutes before the entry; nothing has printed since.
    let now = T0 + 5_000 + MARGIN + LAST_ASK_SLACK_MS;
    let filing = task.on_answer(now, Some((T0 - 3_600_000, T0 - 600_000)));
    assert_eq!(
        filing.file.spans(),
        &[(T0 - MARGIN, T0 + 5_000 + MARGIN)],
        "the silence reaches past the exit's trail, so the whole window is covered and empty"
    );
    assert!(task.is_done());
    assert_eq!(task.due_ms(), None);
    let settled = task.take_settled();
    assert_eq!(settled.len(), 1);
    assert_eq!((settled[0].0, settled[0].1), (T0, T0 + 5_000));
    assert!(
        task.take_settled().is_empty(),
        "a settled trade is handed out once"
    );
}

#[test]
fn a_ring_that_starts_after_the_window_names_the_gap_and_asks_twice_as_often() {
    let mut task = KeyTask::new();
    task.opened(T0, ID, T0, MARGIN, LONG);
    // BTC-level churn: the ring is 20 s deep and starts 5 s after the entry.
    let now = T0 + 25_500;
    let filing = task.on_answer(now, Some((T0 + 5_000, T0 + 25_000)));
    assert_eq!(filing.lost.spans(), &[(T0 - MARGIN, T0 + 4_999)]);
    assert_eq!(filing.file.spans(), &[(T0 + 5_000, T0 + 24_000)]);
    // Depth 20 s -> 2 s, floored at the minimum step.
    assert_eq!(task.due_ms(), Some(now + MIN_STEP_MS));

    // A lost stretch is never lost twice, and a filed one never filed twice.
    let later = now + 3_000;
    let again = task.on_answer(later, Some((T0 + 10_000, later - 100)));
    assert!(again.lost.is_empty());
    assert_eq!(
        again.file.spans(),
        &[(T0 + 24_001, later - 100 - LIVE_TAIL_MS)]
    );
}

#[test]
fn a_gap_halves_the_step_without_going_under_the_floor() {
    // An hour's threshold keeps the open position asking for the whole test.
    let long = 3_600_000;
    let mut task = KeyTask::new();
    task.opened(T0, ID, T0, MARGIN, long);
    // A deep ring first: ten minutes -> the 60 s ceiling.
    task.on_answer(T0 + 1_000, Some((T0 - 600_000, T0 + 900)));
    assert_eq!(task.due_ms(), Some(T0 + 1_000 + MAX_STEP_MS));
    // A burst: the next answer is 380 s deep but starts after what was filed.
    let now = T0 + 400_000;
    let filing = task.on_answer(now, Some((T0 + 20_000, now - 50)));
    assert_eq!(filing.lost.spans(), &[(T0 - 99, T0 + 19_999)]);
    assert_eq!(
        task.due_ms(),
        Some(now + 18_997),
        "depth alone says 38 s; the gap halves it"
    );
    // Halving never goes under the floor.
    let mut hot = KeyTask::new();
    hot.opened(T0, ID, T0, MARGIN, long);
    hot.on_answer(T0 + 10_000, Some((T0, T0 + 9_000)));
    assert_eq!(hot.due_ms(), Some(T0 + 10_000 + MIN_STEP_MS));
}

#[test]
fn a_close_reshapes_the_open_trade_and_a_long_position_skips_its_middle() {
    let mut task = KeyTask::new();
    task.opened(T0, ID, T0, MARGIN, LONG);
    // Stamps lifted a second apart still name the same trade.
    let close = T0 + 3_600_000;
    task.closed(close + 1_000, ID, T0 + 1_000, close, MARGIN, LONG);
    assert_eq!(
        task.needed().spans(),
        &[
            (T0 + 1_000 - MARGIN, T0 + 1_000 + MARGIN),
            (close - MARGIN, close + MARGIN)
        ]
    );
    assert_eq!(task.due_ms(), Some(close + 1_000));
}

#[test]
fn an_open_position_past_the_long_threshold_waits_for_its_exit() {
    let mut task = KeyTask::new();
    task.opened(T0, ID, T0, MARGIN, LONG);
    let now = T0 + LONG + LAST_ASK_SLACK_MS;
    task.on_answer(now, Some((T0 - 900_000, now - 500)));
    assert_eq!(task.due_ms(), None, "entry settled, exit not seen yet");
    assert!(!task.is_done(), "an open trade keeps the key");

    task.drop_stale_opens(T0 + 1);
    assert!(task.is_done());
}

#[test]
fn the_last_ask_lands_past_the_deadline_however_long_the_step() {
    let mut task = KeyTask::new();
    let close = T0 + 10_000;
    task.closed(close, ID, T0, close, MARGIN, LONG);
    // A deep quiet ring: the 60 s step would overshoot the trail's end.
    let now = close + 500;
    task.on_answer(now, Some((T0 - 3_600_000, T0 - 1_000_000)));
    assert_eq!(task.due_ms(), Some(close + MARGIN + LAST_ASK_SLACK_MS));
}

#[test]
fn an_empty_ring_claims_nothing_and_backs_off() {
    let mut task = KeyTask::new();
    task.opened(T0, ID, T0, MARGIN, LONG);
    assert_eq!(task.on_answer(T0 + 1_000, None), Filing::default());
    assert_eq!(task.due_ms(), Some(T0 + 1_000 + MAX_STEP_MS));
    task.on_failure(T0 + 2_000);
    assert_eq!(task.due_ms(), Some(T0 + 2_000 + RETRY_MS));
}

#[test]
fn two_cores_opening_one_key_together_are_two_trades() {
    let other = TradeId { core: 2, rec_id: 7 };
    let mut task = KeyTask::new();
    task.opened(T0, ID, T0, MARGIN, LONG);
    task.opened(T0, other, T0 + 1_000, MARGIN, LONG);
    // One closes quickly; the other stays open and keeps its whole window asked for.
    task.closed(T0 + 10_000, other, T0 + 1_000, T0 + 9_000, MARGIN, LONG);
    assert_eq!(task.needed().spans(), &[(T0 - MARGIN, T0 + LONG)]);
    task.on_answer(T0 + 60_000, Some((T0 - 600_000, T0 + 59_000)));
    assert_eq!(task.take_settled().len(), 1, "only the closed one settles");
    assert!(!task.is_done());
}

#[test]
fn a_donor_that_keeps_failing_is_given_up_past_the_deadline() {
    let mut task = KeyTask::new();
    task.closed(T0 + 10_000, ID, T0, T0 + 10_000, MARGIN, LONG);
    assert!(task.on_failure(T0 + 20_000).is_empty());
    let deadline = T0 + 10_000 + MARGIN;
    let lost = task.on_failure(deadline + GIVE_UP_MS + 1);
    assert_eq!(lost.spans(), &[(T0 - MARGIN, deadline)]);
    assert!(task.is_done());
    assert_eq!(
        task.take_settled().len(),
        1,
        "settled as lost, and still compared"
    );
}
