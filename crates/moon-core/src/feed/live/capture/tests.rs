use super::*;
use moonproto::{ReportFieldValue, ReportRow, ReportValue};

const COIN: u16 = 1;
const BUY: u16 = 2;
const BUY_MS: u16 = 3;
const CLOSE: u16 = 4;
const CLOSE_MS: u16 = 5;
const STRATEGY: u16 = 6;
const EMULATOR: u16 = 7;

fn fields() -> CaptureFields {
    CaptureFields {
        coin: COIN,
        buy_date: BUY,
        buy_ms: Some(BUY_MS),
        close_date: CLOSE,
        close_ms: Some(CLOSE_MS),
        strategy: Some(STRATEGY),
        emulator: Some(EMULATOR),
    }
}

/// A row as moonproto delivers it: fields ascending by index, which `ReportRow::value` relies on
/// (a binary search) — an unsorted list would hide fields from the tracker.
fn row(rec_id: i64, values: &[(u16, ReportValue)]) -> ReportRow {
    let mut fields: Vec<ReportFieldValue> = values
        .iter()
        .map(|(field_index, value)| ReportFieldValue {
            field_index: *field_index,
            value: value.clone(),
        })
        .collect();
    fields.sort_by_key(|field| field.field_index);
    ReportRow { rec_id, fields }
}

fn text(s: &str) -> ReportValue {
    ReportValue::Text(s.into())
}

fn int(v: i64) -> ReportValue {
    ReportValue::Integer(v)
}

/// A close that carries everything is announced from its own row.
#[test]
fn a_complete_closing_row_is_announced() {
    let mut tracker = CaptureTracker::new(fields());
    let closed = tracker
        .on_row(&row(
            7,
            &[
                (COIN, text("SUE")),
                (BUY, int(1_000)),
                (BUY_MS, int(1_000_250)),
                (CLOSE, int(1_060)),
                (CLOSE_MS, int(1_060_900)),
            ],
        ))
        .and_then(RowEdge::closed);
    assert_eq!(
        closed,
        Some(ClosedTrade {
            coin: "SUE".into(),
            buy: ReportStamp::Millis(1_000_250),
            close: ReportStamp::Millis(1_060_900),
        })
    );
}

/// A partial close — `CloseDate` alone — completes itself from the open row seen earlier, and a
/// second closing upsert of the same row is not announced again.
#[test]
fn a_partial_close_is_completed_from_the_open_row_and_announced_once() {
    let mut tracker = CaptureTracker::new(fields());
    let open = row(
        7,
        &[(COIN, text("SUE")), (BUY, int(1_000)), (CLOSE, int(0))],
    );
    assert_eq!(
        tracker.on_row(&open),
        Some(RowEdge::Opened {
            rec_id: 7,
            coin: "SUE".into(),
            buy: ReportStamp::Seconds(1_000),
            strategy: None,
            emulator: None,
        }),
        "an open row announces its entry"
    );
    assert!(
        tracker.on_row(&open).is_none(),
        "the same open row upserted again is not a second entry"
    );
    let closed = tracker
        .on_row(&row(7, &[(CLOSE, int(1_060)), (CLOSE_MS, int(1_060_900))]))
        .and_then(RowEdge::closed);
    assert_eq!(
        closed,
        Some(ClosedTrade {
            coin: "SUE".into(),
            buy: ReportStamp::Seconds(1_000),
            close: ReportStamp::Millis(1_060_900),
        })
    );
    assert_eq!(
        tracker.on_row(&row(
            7,
            &[(CLOSE, int(1_060)), (COIN, text("SUE")), (BUY, int(1_000))]
        )),
        None,
        "the PnL edit that follows a close carries CloseDate too, and is not a second close"
    );
    assert_eq!(
        tracker.on_row(&row(7, &[(COIN, text("SUE")), (BUY, int(1_000))])),
        None,
        "an upsert of a just-closed row that omits CloseDate is not a new entry"
    );
}

/// A close the feed never saw open, and that carries no coin, is not a trade it can locate.
#[test]
fn an_unknown_partial_close_is_skipped() {
    let mut tracker = CaptureTracker::new(fields());
    assert!(tracker.on_row(&row(9, &[(CLOSE, int(1_060))])).is_none());
}

/// A page's closed rows are history, never a close; its open rows are remembered.
#[test]
fn page_rows_remember_open_rows_and_announce_nothing() {
    let mut tracker = CaptureTracker::new(fields());
    tracker.on_page_row(&row(
        3,
        &[(COIN, text("OLD")), (BUY, int(10)), (CLOSE, int(20))],
    ));
    tracker.on_page_row(&row(
        4,
        &[(COIN, text("OPEN")), (BUY, int(30)), (CLOSE, int(0))],
    ));
    assert!(
        tracker.on_row(&row(3, &[(CLOSE, int(20))])).is_none(),
        "a historical close on a page was not remembered as open"
    );
    assert!(
        tracker
            .on_row(&row(4, &[(COIN, text("OPEN")), (BUY, int(30))]))
            .is_none(),
        "a trade already open on a page is not an entry happening now"
    );
    let closed = tracker
        .on_row(&row(4, &[(CLOSE, int(40))]))
        .and_then(RowEdge::closed)
        .expect("the open page row closes");
    assert_eq!(closed.coin, "OPEN");
    assert_eq!(closed.buy, ReportStamp::Seconds(30));
}

/// Rows are kept apart by `rec_id`: closing one does not spend another's memory.
#[test]
fn rows_are_kept_apart() {
    let mut tracker = CaptureTracker::new(fields());
    tracker.on_row(&row(1, &[(COIN, text("A")), (BUY, int(10))]));
    tracker.on_row(&row(2, &[(COIN, text("B")), (BUY, int(20))]));
    let closed = tracker
        .on_row(&row(2, &[(CLOSE, int(30))]))
        .and_then(RowEdge::closed)
        .expect("B closes");
    assert_eq!(closed.coin, "B");
    assert_eq!(closed.buy, ReportStamp::Seconds(20));
    let closed = tracker
        .on_row(&row(1, &[(CLOSE, int(40))]))
        .and_then(RowEdge::closed)
        .expect("A closes");
    assert_eq!(closed.coin, "A");
}

/// A limit buy files its row when the order is placed, before the fill: the row comes with its
/// coin and strategy and no entry stamp, and the fill arrives later as a partial upsert of the
/// stamp alone. The entry is announced on that upsert, completed from the row seen before.
#[test]
fn an_entry_filled_after_its_row_was_filed_is_announced() {
    let mut tracker = CaptureTracker::new(fields());
    let placed = row(
        7,
        &[
            (COIN, text("COLLECT")),
            (BUY, int(0)),
            (CLOSE, int(0)),
            (STRATEGY, int(42)),
            (EMULATOR, int(0)),
        ],
    );
    assert!(
        tracker.on_row(&placed).is_none(),
        "a buy still waiting is no entry"
    );
    assert_eq!(
        tracker.on_row(&row(7, &[(BUY, int(1_000)), (BUY_MS, int(1_000_400))])),
        Some(RowEdge::Opened {
            rec_id: 7,
            coin: "COLLECT".into(),
            buy: ReportStamp::Millis(1_000_400),
            strategy: Some(42),
            emulator: Some(false),
        }),
        "the fill announces the entry with what was filed at placement"
    );
    assert!(
        tracker
            .on_row(&row(7, &[(BUY, int(1_000)), (COIN, text("COLLECT"))]))
            .is_none(),
        "an open row upserted again is no second entry"
    );
    let closed = tracker
        .on_row(&row(7, &[(CLOSE, int(1_060))]))
        .and_then(RowEdge::closed);
    assert_eq!(closed.map(|trade| trade.coin), Some("COLLECT".to_string()));
}

/// A buy still waiting on a catch-up page is completed by its live fill the same way.
#[test]
fn a_waiting_buy_on_a_page_is_announced_on_its_live_fill() {
    let mut tracker = CaptureTracker::new(fields());
    tracker.on_page_row(&row(
        5,
        &[
            (COIN, text("ADA")),
            (BUY, int(0)),
            (CLOSE, int(0)),
            (STRATEGY, int(9)),
        ],
    ));
    let opened = tracker.on_row(&row(5, &[(BUY, int(2_000))]));
    assert!(
        matches!(
            &opened,
            Some(RowEdge::Opened { coin, strategy: Some(9), .. }) if coin == "ADA"
        ),
        "{opened:?}"
    );
}

/// On UDP the fill may overtake the placement: the stamp is kept, and the entry is announced when
/// the placement brings the coin.
#[test]
fn a_fill_that_overtakes_its_placement_is_announced_with_it() {
    let mut tracker = CaptureTracker::new(fields());
    assert!(tracker.on_row(&row(8, &[(BUY, int(1_000))])).is_none());
    let opened = tracker.on_row(&row(
        8,
        &[(COIN, text("ADA")), (CLOSE, int(0)), (STRATEGY, int(3))],
    ));
    assert_eq!(
        opened,
        Some(RowEdge::Opened {
            rec_id: 8,
            coin: "ADA".into(),
            buy: ReportStamp::Seconds(1_000),
            strategy: Some(3),
            emulator: None,
        })
    );
}

/// The memory of waiting buys is bounded by pushing out the longest-filed, not by forgetting all
/// of them; an edit that carries nothing to keep files nothing; a new schema revision keeps it.
#[test]
fn waiting_buys_are_bounded_oldest_first_and_survive_a_schema_revision() {
    let mut tracker = CaptureTracker::new(fields());
    assert!(tracker.on_row(&row(1, &[(CLOSE, int(0))])).is_none());
    assert!(
        tracker.filed.is_empty(),
        "an edit carrying nothing is not filed"
    );
    for rec_id in 0..=MAX_FILED_ROWS as i64 {
        tracker.on_page_row(&row(
            rec_id,
            &[(COIN, text("C")), (BUY, int(0)), (CLOSE, int(0))],
        ));
    }
    assert_eq!(tracker.filed.len(), MAX_FILED_ROWS);
    assert!(
        !tracker.filed.contains_key(&0),
        "the longest-filed went first"
    );
    tracker.refield(fields());
    let newest = MAX_FILED_ROWS as i64;
    assert!(
        matches!(
            tracker.on_row(&row(newest, &[(BUY, int(5))])),
            Some(RowEdge::Opened { ref coin, .. }) if coin == "C"
        ),
        "the newest waiting buy is still completed after a revision"
    );
    assert_eq!(tracker.filed.len(), MAX_FILED_ROWS - 1);
    assert_eq!(tracker.filed_order.len(), tracker.filed.len());
}
