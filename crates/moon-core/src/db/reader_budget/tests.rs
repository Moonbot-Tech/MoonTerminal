use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::*;

/// `db/reader_budget.rs:acquire` must refuse a ninth concurrent reader.
///
/// Dropping `*slots -= 1` (or the `*slots > 0` guard) from the pre-wait path
/// lets every caller take a permit, so descriptors climb back past the cliff
/// and issue #667 returns with no error.
#[test]
fn a_ninth_reader_times_out_until_a_slot_is_freed() {
    assert_eq!(
        available(),
        READER_BUDGET,
        "budget must be idle before this test takes every slot"
    );

    let mut held = Vec::with_capacity(READER_BUDGET);
    for i in 0..READER_BUDGET {
        let permit = acquire(Duration::from_millis(50))
            .unwrap_or_else(|| panic!("slot {i} of {READER_BUDGET} must be granted at the bound"));
        held.push(permit);
    }
    assert_eq!(available(), 0, "every slot is held");
    assert!(
        acquire(Duration::from_millis(80)).is_none(),
        "one past the bound must time out rather than mint a ninth permit"
    );

    let (tx, rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let permit = acquire(Duration::from_secs(2));
        tx.send(permit.is_some())
            .expect("send whether the waiter got a slot");
        permit
    });
    drop(held.pop().expect("held a permit to free"));
    assert!(
        rx.recv_timeout(Duration::from_secs(2))
            .expect("waiter finished"),
        "a waiter must proceed after one slot is freed"
    );
    drop(waiter.join().expect("waiter thread"));
    drop(held);
    assert_eq!(
        available(),
        READER_BUDGET,
        "dropping every permit must restore the bound"
    );
}

/// `db/reader_budget.rs::ReportReader` field order is a Drop-order contract.
///
/// Reordering `_permit` before `conn` returns the slot while the connection's
/// descriptors are still open, so the bound silently under-counts.
#[test]
fn report_reader_declares_conn_before_permit() {
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/db/reader_budget.rs"
    ));
    let start = src
        .find("pub struct ReportReader")
        .expect("ReportReader must exist");
    let after = &src[start..];
    let open = after.find('{').expect("ReportReader body");
    let close = after.find('}').expect("ReportReader body end");
    let body: String = after[open..=close]
        .lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect();
    let conn = body.find("conn:").expect("ReportReader must declare conn");
    let permit = body
        .find("_permit:")
        .expect("ReportReader must declare _permit");
    assert!(
        conn < permit,
        "conn must be declared before _permit so descriptors drop first"
    );
}
