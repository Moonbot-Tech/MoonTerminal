use super::*;
use moonproto::{ReportFieldValue, ReportRow, ReportValue};

const COIN: u16 = 1;
const BUY: u16 = 2;
const BUY_MS: u16 = 3;
const CLOSE: u16 = 4;
const CLOSE_MS: u16 = 5;

fn fields() -> CaptureFields {
    CaptureFields {
        coin: COIN,
        buy_date: BUY,
        buy_ms: Some(BUY_MS),
        close_date: CLOSE,
        close_ms: Some(CLOSE_MS),
    }
}

fn row(rec_id: i64, values: &[(u16, ReportValue)]) -> ReportRow {
    ReportRow {
        rec_id,
        fields: values
            .iter()
            .map(|(field_index, value)| ReportFieldValue {
                field_index: *field_index,
                value: value.clone(),
            })
            .collect(),
    }
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
    let closed = tracker.on_row(&row(
        7,
        &[
            (COIN, text("SUE")),
            (BUY, int(1_000)),
            (BUY_MS, int(1_000_250)),
            (CLOSE, int(1_060)),
            (CLOSE_MS, int(1_060_900)),
        ],
    ));
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
    assert!(
        tracker
            .on_row(&row(
                7,
                &[(COIN, text("SUE")), (BUY, int(1_000)), (CLOSE, int(0))]
            ))
            .is_none(),
        "an open row announces nothing"
    );
    let closed = tracker.on_row(&row(7, &[(CLOSE, int(1_060)), (CLOSE_MS, int(1_060_900))]));
    assert_eq!(
        closed,
        Some(ClosedTrade {
            coin: "SUE".into(),
            buy: ReportStamp::Seconds(1_000),
            close: ReportStamp::Millis(1_060_900),
        })
    );
    assert!(
        tracker
            .on_row(&row(
                7,
                &[(CLOSE, int(1_060)), (COIN, text("SUE")), (BUY, int(1_000))]
            ))
            .is_none(),
        "the PnL edit that follows a close carries CloseDate too, and is not a second close"
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
    let closed = tracker
        .on_row(&row(4, &[(CLOSE, int(40))]))
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
        .expect("B closes");
    assert_eq!(closed.coin, "B");
    assert_eq!(closed.buy, ReportStamp::Seconds(20));
    let closed = tracker
        .on_row(&row(1, &[(CLOSE, int(40))]))
        .expect("A closes");
    assert_eq!(closed.coin, "A");
}
