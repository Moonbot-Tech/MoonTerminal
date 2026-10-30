use super::store::read_conn;
use super::*;

const EX: &str = "ex:00000000";

const W: Window = Window {
    margin_ms: 60_000,
    long_ms: 600_000,
    tail_ms: 70_000,
};

fn list(t_ms: i64, bid: f64) -> Batch {
    Batch {
        exchange: Arc::from(EX),
        t_ms,
        rows: vec![Row {
            market: "XUSDT".to_owned(),
            bid,
            ask: bid + 1.0,
            mark: bid + 0.5,
            funding: None,
        }],
        broken: false,
    }
}

/// Drive a recorder the way its thread does: a list every two seconds, a flush every thirty,
/// trade events at their moments. `silent` is a stretch with no lists at all.
fn drive(
    rec: &mut Recorder,
    until_ms: i64,
    silent: Option<(i64, i64)>,
    mut events: impl FnMut(&mut Recorder, i64),
) {
    let mut t = 0;
    while t <= until_ms {
        if !silent.is_some_and(|(a, b)| (a..=b).contains(&t)) {
            rec.list(list(t, t as f64));
        }
        events(rec, t);
        if t % 30_000 == 0 {
            rec.flush(t, t - SETTLE_MS, W);
        }
        t += 2_000;
    }
}

fn trade(rec_id: i64) -> TradeId {
    TradeId { core: 7, rec_id }
}

fn key(market: &str) -> Key {
    (EX.to_owned(), market.to_owned())
}

fn read(rec: &Recorder, market: &str) -> Vec<PriceSpan> {
    let store = rec.store.as_ref().unwrap();
    read_conn(store.conn(), EX, market, 0, i64::MAX).unwrap()
}

fn spans(rec: &Recorder) -> Vec<(i64, i64)> {
    read(rec, "XUSDT")
        .iter()
        .map(|s| (s.from_ms, s.to_ms))
        .collect()
}

#[test]
fn a_trade_is_filed_from_before_its_entry_through_its_tail() {
    let mut rec = Recorder::new(Some(Store::in_memory().unwrap()));
    drive(&mut rec, 400_000, None, |rec, t| match t {
        200_000 => rec.plan(key("XUSDT")).opened(trade(1), 200_000),
        250_000 => rec.plan(key("XUSDT")).closed(trade(1), 200_000, 250_000),
        _ => {}
    });
    // From entry − margin to exit + tail, unbroken, and the key let go once filed.
    assert_eq!(spans(&rec), vec![(140_000, 320_000)]);
    assert!(rec.keys.is_empty());
    let span = &read(&rec, "XUSDT")[0];
    // Every list changed the bid: one sample per list from the first inside the span on.
    assert_eq!(span.samples.first().map(|s| s.t_ms), Some(140_000));
    assert_eq!(span.samples.len(), (320_000 - 140_000) / 2_000 + 1);
    assert!(span.samples.iter().all(|s| s.mark == s.bid + 0.5));
}

#[test]
fn a_next_trade_does_not_file_its_predecessors_tail_again() {
    let mut rec = Recorder::new(Some(Store::in_memory().unwrap()));
    drive(&mut rec, 400_000, None, |rec, t| match t {
        100_000 => rec.plan(key("XUSDT")).opened(trade(1), 100_000),
        120_000 => rec.plan(key("XUSDT")).closed(trade(1), 100_000, 120_000),
        // The first key is done (tail to 190 000) and let go at the 210 000 flush; this entry's
        // margin reaches back to 170 000.
        230_000 => rec.plan(key("XUSDT")).opened(trade(2), 230_000),
        250_000 => rec.plan(key("XUSDT")).closed(trade(2), 230_000, 250_000),
        _ => {}
    });
    let spans = read(&rec, "XUSDT");
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].from_ms, spans[0].to_ms), (40_000, 320_000));
    assert!(
        spans[0].samples.windows(2).all(|w| w[0].t_ms < w[1].t_ms),
        "every moment once, in order"
    );
}

#[test]
fn a_silent_source_leaves_a_gap_and_nothing_across_it() {
    let mut rec = Recorder::new(Some(Store::in_memory().unwrap()));
    drive(
        &mut rec,
        400_000,
        Some((220_000, 240_000)),
        |rec, t| match t {
            200_000 => rec.plan(key("XUSDT")).opened(trade(1), 200_000),
            250_000 => rec.plan(key("XUSDT")).closed(trade(1), 200_000, 250_000),
            _ => {}
        },
    );
    // The last list before the silence vouches for GAP_MS past it; then nothing until 242 000.
    assert_eq!(
        spans(&rec),
        vec![(140_000, 218_000 + ring::GAP_MS), (242_000, 320_000)]
    );
}

#[test]
fn what_left_the_ring_before_the_entry_arrived_is_lost_not_invented() {
    let mut rec = Recorder::new(Some(Store::in_memory().unwrap()));
    // The open of a trade that entered long ago reaches the recorder now, as after a reconnect.
    drive(&mut rec, 420_000, None, |rec, t| match t {
        300_000 => rec.plan(key("XUSDT")).opened(trade(1), 100_000),
        330_000 => rec.plan(key("XUSDT")).closed(trade(1), 100_000, 330_000),
        _ => {}
    });
    // The ring reaches margin + spare back from the first flush that knew of the trade.
    let first_flush = 300_000;
    let floor = first_flush - ring_depth_ms(W.margin_ms);
    assert_eq!(spans(&rec), vec![(floor, 400_000)]);
}

#[test]
fn a_spot_trade_files_nothing_and_is_let_go() {
    let mut rec = Recorder::new(Some(Store::in_memory().unwrap()));
    drive(&mut rec, 400_000, None, |rec, t| {
        if t == 200_000 {
            rec.plan(key("SPOTUSDT")).closed(trade(1), 190_000, 200_000);
        }
    });
    assert!(read(&rec, "SPOTUSDT").is_empty());
    assert!(rec.keys.is_empty());
}

#[test]
fn what_the_file_refused_is_filed_once_it_opens() {
    let mut rec = Recorder::new(None);
    drive(&mut rec, 400_000, None, |rec, t| match t {
        200_000 => rec.plan(key("XUSDT")).opened(trade(1), 200_000),
        250_000 => rec.plan(key("XUSDT")).closed(trade(1), 200_000, 250_000),
        _ => {}
    });
    assert!(!rec.unfiled.is_empty());
    rec.store = Some(Store::in_memory().unwrap());
    rec.flush(402_000, 401_000, W);
    assert!(rec.unfiled.is_empty());
    assert_eq!(spans(&rec), vec![(140_000, 320_000)]);
}

#[test]
fn the_ring_keeps_no_more_than_its_depth() {
    let mut rec = Recorder::new(None);
    drive(&mut rec, 1_000_000, None, |_, _| {});
    let (_, points) = rec.rings[EX].size();
    // One point per list over the depth, plus the 30 s since the last trim and the state point.
    let most = (ring_depth_ms(W.margin_ms) + 30_000) / 2_000 + 2;
    assert!(
        points as i64 <= most,
        "{points} points held, at most {most}"
    );
}
