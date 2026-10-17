use super::*;
use crate::analytics::tuner::ticks::state::{DealRow, TapeStatus};
use moon_core::market::trade_replay::TickStatus;

fn deal(core_uid: u64, report_uid: i64) -> Deal {
    Deal {
        report_uid,
        core_uid,
        core_name: "CORE_X".into(),
        strategy_id: 1,
        kind: "MoonShot".into(),
        coin: "XALPHA".into(),
        buy_ms: 100_000,
        close_ms: 110_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Default::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        drops_detect: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    }
}

fn key(tape_rev: u64, refusal_revision: u64) -> ReloadKey {
    ReloadKey {
        scope: signature_of(&[deal(1, 1), deal(1, 2)]),
        period: (0, 1_000),
        model: Default::default(),
        sample: SamplePolicy {
            min_tail_s: 60,
            switches: (true, false),
            margin_ms: 30_000,
            long_position_ms: 3_600_000,
        },
        refusal_revision,
        tape_rev,
        addresses: 7,
    }
}

/// The closed deals in any order sign the same; an open deal beside them changes nothing.
#[test]
fn ignores_open_deals_and_order() {
    let a = deal(1, 1);
    let b = deal(2, 1);
    let mut open = deal(1, 3);
    open.close_ms = 0;
    let base = signature_of(&[a.clone(), b.clone()]);
    assert_eq!(base, signature_of(&[b.clone(), a.clone()]));
    assert_eq!(base, signature_of(&[a.clone(), open.clone(), b.clone()]));
    // The same uid on another core is another deal.
    assert_ne!(
        signature_of(std::slice::from_ref(&a)),
        signature_of(std::slice::from_ref(&b))
    );

    // The open deal closing joins the signature.
    let mut closed = open;
    closed.close_ms = 120_000;
    assert_ne!(base, signature_of(&[a, b, closed]));
}

/// A same-uid correction of any model input, or of the fact, signs differently.
#[test]
fn same_uid_correction_changes_signature() {
    let base = deal(1, 1);
    let edits: Vec<Box<dyn Fn(&mut Deal)>> = vec![
        Box::new(|d| d.buy_ms += 1),
        Box::new(|d| d.close_ms += 1),
        Box::new(|d| d.buy_price = 1.0001),
        Box::new(|d| d.sell_price = 1.02),
        Box::new(|d| d.buy_set_ms = Some(99_000)),
        Box::new(|d| d.kind = "MoonHook".into()),
        Box::new(|d| d.coin = "XBETA".into()),
        Box::new(|d| d.is_short = true),
        Box::new(|d| d.sell_reason = "Auto Price Down".into()),
        Box::new(|d| d.deltas.market1h = 0.5),
        Box::new(|d| d.hook_stated_take_pct = Some(2.5)),
        Box::new(|d| d.hook_depth_pct = Some(3.0)),
        Box::new(|d| d.strategy_id = 2),
        Box::new(|d| d.fact_pnl = 1.0),
        Box::new(|d| d.spent = 50.0),
    ];
    let sig = signature_of(std::slice::from_ref(&base));
    for (i, edit) in edits.iter().enumerate() {
        let mut changed = base.clone();
        edit(&mut changed);
        assert_ne!(sig, signature_of(&[changed]), "edit {i} went unseen");
    }
}

/// An unchanged report still refreshes when the store filed tape or the station's refusals
/// moved, and when a publication wait came due.
#[test]
fn unchanged_report_with_tape_or_refusal_change_refreshes() {
    let stored = key(5, 2);
    assert!(can_skip(
        Some(&stored),
        &key(5, 2),
        &[],
        0,
        i64::MAX,
        |_| true
    ));
    assert!(
        !can_skip(Some(&stored), &key(6, 2), &[], 0, i64::MAX, |_| true),
        "tape refiled"
    );
    assert!(
        !can_skip(Some(&stored), &key(5, 3), &[], 0, i64::MAX, |_| true),
        "refusal moved"
    );
    assert!(
        !can_skip(None, &key(5, 2), &[], 0, i64::MAX, |_| true),
        "never settled"
    );

    let waiting = DealRow {
        deal: deal(1, 1),
        tape: TapeStatus::Refused(TickStatus::ArchivePending { retry_at_ms: 500 }),
        verdict: None,
        address: None,
        ticks: None,
        entry_line: None,
        held: None,
    };
    let rows = [waiting];
    assert!(can_skip(
        Some(&stored),
        &key(5, 2),
        &rows,
        499,
        i64::MAX,
        |_| true
    ));
    assert!(
        !can_skip(Some(&stored), &key(5, 2), &rows, 500, i64::MAX, |_| true),
        "retry due"
    );
}

/// A covered short tail settled while a source could still extend it stops vouching for the
/// rows at the retention edge: the fast path refuses from then on, and a missing row the clock
/// moved refuses it at once.
#[test]
fn clock_edges_defeat_the_fast_path() {
    use crate::analytics::tuner::ticks::state::RowAddress;
    let close = deal(1, 1).close_ms;
    let covered = DealRow {
        deal: deal(1, 1),
        tape: TapeStatus::Covered,
        verdict: None,
        address: Some(std::sync::Arc::new(RowAddress {
            core_uid: 1,
            venue: None,
            exchange_key: moon_core::fixture::unlisted::EXCHANGE_KEY.into(),
            market: "XALPHA".into(),
            btc_market: None,
        })),
        ticks: None,
        entry_line: None,
        held: Some((30_000, 10_000)),
    };
    let rows = [covered];
    let settled_at = close + 3_600_000;
    let until = settled_until(&rows, 60, settled_at);
    assert_eq!(
        until,
        close + 10_000 + moon_core::market::CORE_ARCHIVE_RETENTION_MS + 1
    );
    let stored = key(5, 2);
    assert!(can_skip(
        Some(&stored),
        &key(5, 2),
        &rows,
        until - 1,
        until,
        |_| true
    ));
    assert!(
        !can_skip(Some(&stored), &key(5, 2), &rows, until, until, |_| true),
        "the short tail's admission deadline passed"
    );

    let mut missing = rows[0].clone();
    missing.tape = TapeStatus::Missing;
    assert!(!can_skip(
        Some(&stored),
        &key(5, 2),
        &[missing],
        0,
        i64::MAX,
        |_| false
    ));
    assert_eq!(settled_until(&[], 60, settled_at), i64::MAX);
}
