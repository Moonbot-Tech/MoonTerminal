//! The station's tape answers: pieces cut to what was wanted, and a budget that stops cleanly.

use moon_core::feed::types::{Side, Tick};
use moon_core::market::trade_replay::TileSource;
use moon_core::market::trade_replay::trade_cache::{StoredSpan, decode_prints};
use moon_core::station_api::TapeWant;

use super::*;

fn tick(time_ms: i64) -> Tick {
    Tick {
        time_ms: time_ms as f64,
        price: 1.0,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// A held file of one market: `(from, to, print times)`.
fn held(
    spans: &'static [(i64, i64, &'static [i64])],
) -> impl FnMut(&str, &str, i64, i64) -> Result<Vec<StoredSpan>, String> {
    move |_, _, from, to| {
        Ok(spans
            .iter()
            .filter(|(a, b, _)| *b >= from && *a <= to)
            .map(|&(from_ms, to_ms, times)| StoredSpan {
                from_ms,
                to_ms,
                ticks: times.iter().map(|&t| tick(t)).collect(),
                source: TileSource::Core,
            })
            .collect())
    }
}

fn want(spans: &[(i64, i64)]) -> TapeWant {
    TapeWant {
        exchange: "x".into(),
        market: "M".into(),
        spans: spans.to_vec(),
    }
}

fn times(piece: &moon_core::station_api::TapePiece) -> Vec<i64> {
    decode_prints(&piece.prints)
        .expect("decodes")
        .iter()
        .map(|t| t.time_ms as i64)
        .collect()
}

/// A held span is cut to the wanted stretch, a quiet span is still a piece, and what the station
/// does not hold is simply absent.
#[test]
fn pieces_are_what_was_wanted_and_held() {
    let tape = answer_tape(
        &[want(&[(150, 450)])],
        1 << 20,
        held(&[
            (100, 199, &[120, 160, 190]),
            (200, 299, &[]),
            (500, 599, &[550]),
        ]),
    )
    .unwrap();
    let bounds: Vec<_> = tape.pieces.iter().map(|p| (p.from_ms, p.to_ms)).collect();
    assert_eq!(bounds, vec![(150, 199), (200, 299)]);
    assert_eq!(times(&tape.pieces[0]), vec![160, 190]);
    assert!(times(&tape.pieces[1]).is_empty());
    assert_eq!(tape.resume, None);
}

/// Past the budget the answer stops at a millisecond boundary and says where to go on; asking
/// again from there gets the rest, and nothing is lost or doubled between the two.
#[test]
fn a_full_answer_stops_at_a_millisecond_and_goes_on() {
    static TIMES: [i64; 400] = {
        let mut t = [0i64; 400];
        let mut i = 0;
        while i < 400 {
            t[i] = 1_000 + (i as i64 / 2);
            i += 1;
        }
        t
    };
    static SPANS: [(i64, i64, &[i64]); 1] = [(1_000, 1_199, &TIMES)];
    // Half of what the whole span packs to: the codec packs these prints tightly, so the
    // budget is measured, not guessed.
    let whole: Vec<_> = TIMES.iter().map(|&t| tick(t)).collect();
    let budget = PIECE_OVERHEAD + encode_prints(&whole).len() / 2;
    let first = answer_tape(&[want(&[(1_000, 1_199)])], budget, held(&SPANS)).unwrap();
    let resume = first.resume.clone().expect("stopped");
    assert_eq!(first.pieces.len(), 1);
    let piece = &first.pieces[0];
    assert_eq!(resume.from_ms, piece.to_ms + 1);
    let got = times(piece);
    // Both prints of the last millisecond, never one.
    assert_eq!(got.iter().filter(|&&t| t == piece.to_ms).count(), 2);
    let second = answer_tape(&[want(&[(resume.from_ms, 1_199)])], 1 << 20, held(&SPANS)).unwrap();
    let mut all = got;
    all.extend(times(&second.pieces[0]));
    assert_eq!(all, TIMES.to_vec());
}

/// An answer whose first piece alone is past the budget still sends its first millisecond, so a
/// client asking again always gets further; a later piece that does not fit waits whole.
#[test]
fn the_first_piece_always_goes_out() {
    static SPANS: [(i64, i64, &[i64]); 2] = [(0, 9, &[1, 1, 2, 3]), (10, 19, &[11, 12])];
    let tight = answer_tape(&[want(&[(0, 19)])], 0, held(&SPANS)).unwrap();
    assert_eq!(tight.pieces.len(), 1);
    assert_eq!(times(&tight.pieces[0]), vec![1, 1]);
    assert_eq!(tight.resume.map(|r| r.from_ms), Some(2));
    // Room for the first piece, not the second: the second waits whole.
    let one = answer_tape(&[want(&[(0, 9)])], 1 << 20, held(&SPANS)).unwrap();
    let room = one.pieces[0].prints.len() + PIECE_OVERHEAD;
    let split = answer_tape(&[want(&[(0, 19)])], room, held(&SPANS)).unwrap();
    assert_eq!(split.pieces.len(), 1);
    assert_eq!(split.resume.map(|r| (r.item, r.from_ms)), Some((0, 10)));
}

/// Requests past the bounds, or with a reversed stretch, are refused whole.
#[test]
fn a_request_past_the_bounds_is_refused() {
    let too_many: Vec<_> = (0..=MAX_TAPE_ITEMS).map(|_| want(&[(0, 1)])).collect();
    assert!(check_wants(&too_many).is_err());
    assert!(check_wants(&[want(&[(5, 1)])]).is_err());
    assert!(check_wants(&[want(&[(1, 5)])]).is_ok());
}

/// A traces answer stops at the budget and says how far it got; the first trade always goes out,
/// and one alone past the budget is skipped rather than refused whole.
#[test]
fn a_traces_answer_stops_at_the_budget() {
    use moon_core::db::order_traces::TraceEntry;
    use moon_core::feed::{ArchivedLineKind, ArchivedOrderTrace};
    let trace = |points: usize| {
        TraceEntry::Lines(std::sync::Arc::from(vec![ArchivedOrderTrace {
            own: true,
            kind: ArchivedLineKind::Entry,
            stop_price: None,
            stop_time_ms: None,
            points: (0..points).map(|i| (i as f64, 1.0)).collect(),
        }]))
    };
    let held = std::collections::HashMap::from([
        (1, trace(10)),
        (2, TraceEntry::Empty { checked_at_ms: 0 }),
        (3, trace(10)),
        (4, trace(10)),
    ]);
    let all = answer_traces(&[1, 2, 3, 4, 5], &held, 1 << 20);
    assert_eq!(all.answered, 5);
    assert_eq!(
        all.trades.iter().map(|t| t.report_uid).collect::<Vec<_>>(),
        vec![1, 3, 4]
    );
    let one = serde_json::to_vec(&all.trades[0]).unwrap().len();
    // Room for two trades: the answer covers 1, 2 (no lines) and 3, and stops before 4.
    let part = answer_traces(&[1, 2, 3, 4, 5], &held, 2 * one + one / 2);
    assert_eq!(part.answered, 3);
    assert_eq!(part.trades.len(), 2);
    // Not even the first fits: it is skipped, and the answer still gets further.
    let tight = answer_traces(&[1, 3], &held, 10);
    assert_eq!(tight.answered, 1);
    assert!(tight.trades.is_empty());
}
