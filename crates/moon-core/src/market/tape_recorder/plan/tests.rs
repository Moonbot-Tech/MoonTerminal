use super::*;
use crate::feed::Side;

const MARGIN: i64 = 30_000;
const LONG: i64 = 300_000;
const T0: i64 = 1_800_000_000_000;
const ID: TradeId = TradeId { core: 1, rec_id: 7 };

fn tick(at: i64) -> Tick {
    Tick {
        time_ms: at as f64,
        price: 1.0,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// One print every `step` ms over `[from, to]`.
fn prints(from: i64, to: i64, step: i64) -> Vec<Tick> {
    (0..)
        .map(|i| from + i * step)
        .take_while(|&at| at <= to)
        .map(tick)
        .collect()
}

#[test]
fn an_open_selects_the_pair_and_is_seeded_once() {
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    assert!(task.wants_pair());
    assert!(task.needs_seed());

    task.seed_asked(T0 + 1_000, 0);
    assert!(!task.needs_seed(), "one archive in flight is enough");

    // A deep ring: the whole run-up is in it.
    let lost = task.seeded(prints(T0 - 120_000, T0 + 1_000, 100), T0 + 1_000, 0);
    assert!(lost.is_empty());
    assert!(task.is_live());
    assert!(!task.needs_seed(), "a recording pair is never asked again");

    // The stream is alive at T0 + 25 s: the run-up and the first seconds are due at once.
    let filing = task.flush(T0 + 25_000 - QUIET_TAIL_MS, false).unwrap();
    assert_eq!(filing.file.spans(), &[(T0 - MARGIN, T0 + 15_000)]);
    assert_eq!(filing.ticks.len(), 311);
    assert!(
        filing
            .ticks
            .windows(2)
            .all(|w| w[0].time_ms <= w[1].time_ms)
    );
}

/// A trade taken up again after a restart: what the file already holds is not recorded twice.
/// Held whole, the key wants no pair at all; held in part, the recording asks for the rest only
/// and files nothing over what is held.
#[test]
fn a_resumed_trade_records_only_what_the_file_lacks() {
    let mut whole = KeyTask::new();
    whole.filed_before(&[(T0 - MARGIN, T0 + LONG)]);
    whole.opened(ID, T0, MARGIN, LONG);
    assert!(whole.knows(ID));
    assert!(
        !whole.wants_pair(),
        "everything needed is in the file already"
    );

    let mut part = KeyTask::new();
    // Filed up to the restart at T0 + 60 s; the station came back at T0 + 180 s.
    part.filed_before(&[(T0 - MARGIN, T0 + 60_000)]);
    part.opened(ID, T0, MARGIN, LONG);
    assert!(part.wants_pair());
    assert!(part.needs_seed());
    part.seed_asked(T0 + 180_000, 0);
    let lost = part.seeded(prints(T0 + 100_000, T0 + 180_000, 1_000), T0 + 180_000, 0);
    // Only the downtime the archive no longer holds is lost, not the stretch already filed.
    assert_eq!(lost.spans(), &[(T0 + 60_001, T0 + 99_999)]);
    let filing = part.flush(T0 + 200_000, true).unwrap();
    assert_eq!(filing.file.spans(), &[(T0 + 100_000, T0 + 200_000)]);
}

#[test]
fn a_hot_ring_names_the_run_up_it_no_longer_holds() {
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    task.seed_asked(T0 + 1_000, 0);
    // BTC-level churn: the ring starts 12 s before the entry.
    let lost = task.seeded(prints(T0 - 12_000, T0 + 1_000, 10), T0 + 1_000, 0);
    assert_eq!(lost.spans(), &[(T0 - MARGIN, T0 - 12_001)]);
    let filing = task.flush(T0 + 40_000, false).unwrap();
    assert_eq!(filing.file.spans(), &[(T0 - 12_000, T0 + 40_000)]);
}

#[test]
fn live_rows_are_buffered_until_enough_of_them_is_due() {
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    task.seed_asked(T0, 0);
    task.seeded(prints(T0 - 60_000, T0, 1_000), T0, 0);
    task.flush(T0, false).unwrap();

    assert_eq!(task.drained(&prints(T0 + 1_000, T0 + 20_000, 1_000)), 0);
    assert!(
        task.flush(T0 + 20_000, false).is_none(),
        "20 s is not a row worth writing yet"
    );
    assert_eq!(task.drained(&prints(T0 + 21_000, T0 + 40_000, 1_000)), 0);
    let filing = task.flush(T0 + 35_000, false).unwrap();
    assert_eq!(filing.file.spans(), &[(T0 + 1, T0 + 35_000)]);
    assert_eq!(filing.ticks.len(), 35);

    // A print stamped behind what was filed is dropped, not written into a closed stretch.
    assert_eq!(task.drained(&[tick(T0 + 34_000), tick(T0 + 41_000)]), 1);
    // Forced at the end of a recording, whatever built up is written.
    let rest = task.flush(T0 + 42_000, true).unwrap();
    assert_eq!(rest.file.spans(), &[(T0 + 35_001, T0 + 42_000)]);
    assert_eq!(rest.ticks.len(), 6);
}

#[test]
fn a_quiet_market_is_filed_as_silent_up_to_the_frontier() {
    let mut task = KeyTask::new();
    task.closed(ID, T0, T0 + 5_000, MARGIN, LONG);
    task.seed_asked(T0 + 6_000, 0);
    // The last print was ten minutes before the entry; nothing has printed since.
    task.seeded(
        vec![tick(T0 - 3_600_000), tick(T0 - 600_000)],
        T0 + 6_000,
        0,
    );
    let frontier = T0 + 5_000 + MARGIN;
    let filing = task.flush(frontier, false).unwrap();
    assert_eq!(filing.file.spans(), &[(T0 - MARGIN, T0 + 5_000 + MARGIN)]);
    assert!(filing.ticks.is_empty(), "covered and empty");
    assert!(!task.wants_pair(), "the trail is filed: the pair can go");
    assert!(task.is_done());
    let settled = task.take_settled();
    assert_eq!(settled.len(), 1);
    assert_eq!((settled[0].0, settled[0].1), (T0, T0 + 5_000));
    assert!(
        task.take_settled().is_empty(),
        "a settled trade is handed out once"
    );
}

#[test]
fn a_short_position_keeps_the_pair_until_its_trail_is_filed() {
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    task.seed_asked(T0, 0);
    task.seeded(prints(T0 - 60_000, T0, 1_000), T0, 0);
    task.flush(T0 + 50_000, false).unwrap();

    let close = T0 + 60_000;
    task.closed(ID, T0, close, MARGIN, LONG);
    assert_eq!(task.needed().spans(), &[(T0 - MARGIN, close + MARGIN)]);
    assert!(!task.needs_seed(), "the exit is inside the live recording");
    assert_eq!(task.deadline(), Some(close + MARGIN));

    task.flush(close + MARGIN - 1, true);
    assert!(
        task.wants_pair(),
        "one millisecond of the trail is still ahead"
    );
    task.flush(close + MARGIN, true);
    assert!(!task.wants_pair());
    assert!(task.is_done());
}

#[test]
fn a_long_position_lets_the_pair_go_and_seeds_again_at_the_exit() {
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    task.seed_asked(T0, 0);
    task.seeded(prints(T0 - 60_000, T0, 1_000), T0, 0);
    task.flush(T0 + LONG, false).unwrap();
    assert!(
        !task.wants_pair(),
        "entry side settled at the long threshold"
    );
    assert!(!task.is_done(), "an open trade keeps the key");
    task.stop();

    let close = T0 + 3_600_000;
    task.closed(ID, T0 + 1_000, close, MARGIN, LONG);
    assert_eq!(
        task.needed().spans(),
        &[
            (T0 + 1_000 - MARGIN, T0 + 1_000 + MARGIN),
            (close - MARGIN, close + MARGIN)
        ]
    );
    assert!(task.wants_pair());
    assert!(
        task.needs_seed(),
        "the exit window is asked from the archive"
    );

    task.seed_asked(close + 1_000, 0);
    let lost = task.seeded(prints(close - 20_000, close + 1_000, 500), close + 1_000, 0);
    assert_eq!(lost.spans(), &[(close - MARGIN, close - 20_001)]);
    let filing = task.flush(close + MARGIN, false).unwrap();
    assert_eq!(filing.file.spans(), &[(close - 20_000, close + MARGIN)]);
    assert!(task.is_done());
}

#[test]
fn trades_of_one_key_share_the_pair_until_the_last_trail() {
    let other = TradeId { core: 2, rec_id: 7 };
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    task.opened(other, T0 + 1_000, MARGIN, LONG);
    task.seed_asked(T0 + 1_000, 0);
    task.seeded(prints(T0 - 60_000, T0 + 1_000, 1_000), T0 + 1_000, 0);

    task.closed(other, T0 + 1_000, T0 + 9_000, MARGIN, LONG);
    task.closed(ID, T0, T0 + 20_000, MARGIN, LONG);
    assert!(!task.needs_seed(), "the same subscription records both");
    assert_eq!(task.deadline(), Some(T0 + 20_000 + MARGIN));

    task.flush(T0 + 9_000 + MARGIN, true);
    assert_eq!(task.take_settled().len(), 1, "the first trail is filed");
    assert!(task.wants_pair(), "the second trade keeps the pair");
    task.flush(T0 + 20_000 + MARGIN, true);
    assert_eq!(task.take_settled().len(), 1);
    assert!(task.is_done());
}

#[test]
fn a_ring_that_overwrote_unread_rows_leaves_an_honest_gap() {
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    task.seed_asked(T0, 0);
    task.seeded(prints(T0 - 60_000, T0 + 10_000, 1_000), T0, 0);
    // Rows between the newest drained print and the first one read after the overwrite are gone.
    let lost = task.clipped(T0 + 14_000);
    assert_eq!(lost.spans(), &[(T0 + 10_001, T0 + 13_999)]);
    task.drained(&prints(T0 + 14_000, T0 + 40_000, 1_000));
    let filing = task.flush(T0 + 40_000, false).unwrap();
    assert_eq!(
        filing.file.spans(),
        &[(T0 - MARGIN, T0 + 10_000), (T0 + 14_000, T0 + 40_000)]
    );
}

#[test]
fn an_archive_that_never_comes_records_the_live_rows_alone() {
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    task.seed_asked(T0 + 2_000, 0);
    let lost = task.seed_failed();
    assert_eq!(lost.spans(), &[(T0 - MARGIN, T0 + 1_999)]);
    assert!(task.is_live());
    assert!(
        !task.needs_seed(),
        "the run-up is lost, not asked for forever"
    );
    task.drained(&prints(T0 + 2_000, T0 + 40_000, 1_000));
    let filing = task.flush(T0 + 40_000, false).unwrap();
    assert_eq!(filing.file.spans(), &[(T0 + 2_000, T0 + 40_000)]);
}

#[test]
fn a_need_behind_the_frontier_is_seeded_again() {
    let other = TradeId { core: 2, rec_id: 9 };
    let mut task = KeyTask::new();
    // A long position whose entry side is done; another trade keeps the pair selected.
    task.opened(ID, T0, MARGIN, LONG);
    task.opened(other, T0 + LONG - 1_000, MARGIN, LONG);
    task.seed_asked(T0, 0);
    task.seeded(prints(T0 - 60_000, T0, 1_000), T0, 0);
    task.flush(T0 + LONG + 60_000, true);

    // The first trade closes; its exit window lies in rows drained while nothing needed them.
    let close = T0 + 2 * LONG + 60_000;
    task.flush(close + 5_000, true);
    task.closed(ID, T0, close, MARGIN, LONG);
    assert!(task.needs_seed());
    task.seed_asked(close + 6_000, 0);
    let lost = task.seeded(
        prints(close - MARGIN - 5_000, close + 6_000, 1_000),
        close + 6_000,
        0,
    );
    assert!(lost.is_empty());
    let filing = task.flush(close + 6_000, true).unwrap();
    assert_eq!(filing.file.spans(), &[(close - MARGIN, close + 6_000)]);

    // An archive that cannot reach the hole names it lost instead of asking again.
    let mut hole = KeyTask::new();
    hole.opened(ID, T0, MARGIN, LONG);
    hole.opened(other, T0 + LONG - 1_000, MARGIN, LONG);
    hole.seed_asked(T0, 0);
    hole.seeded(prints(T0 - 60_000, T0, 1_000), T0, 0);
    hole.flush(close + 5_000, true);
    hole.closed(ID, T0, close, MARGIN, LONG);
    hole.seed_asked(close + 6_000, 0);
    let lost = hole.seed_failed();
    assert_eq!(lost.spans(), &[(close - MARGIN, close + 5_000)]);
    assert!(!hole.needs_seed());
}

#[test]
fn a_recording_that_never_happens_is_given_up_past_the_deadline() {
    let mut task = KeyTask::new();
    task.closed(ID, T0, T0 + 10_000, MARGIN, LONG);
    let deadline = T0 + 10_000 + MARGIN;
    assert!(!task.overdue(deadline + GIVE_UP_MS));
    assert!(task.overdue(deadline + GIVE_UP_MS + 1));
    assert_eq!(task.give_up().spans(), &[(T0 - MARGIN, deadline)]);
    assert!(task.is_done());
    assert_eq!(
        task.take_settled().len(),
        1,
        "settled as lost, and still compared"
    );
}

#[test]
fn an_open_whose_exit_never_came_is_forgotten() {
    let mut task = KeyTask::new();
    task.opened(ID, T0, MARGIN, LONG);
    task.give_up();
    assert!(!task.is_done(), "an open trade keeps the key");
    task.drop_stale_opens(T0 + 1);
    assert!(task.is_done());
}
