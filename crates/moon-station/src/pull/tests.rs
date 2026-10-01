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
) -> impl FnMut(
    &str,
    &str,
    i64,
    i64,
) -> Result<Box<dyn Iterator<Item = Result<StoredSpan, String>>>, String> {
    move |_, _, from, to| {
        Ok(Box::new(
            spans
                .iter()
                .filter(move |(a, b, _)| *b >= from && *a <= to)
                .map(|&(from_ms, to_ms, times)| {
                    Ok(StoredSpan {
                        from_ms,
                        to_ms,
                        ticks: times.iter().map(|&t| tick(t)).collect(),
                        source: TileSource::Core,
                    })
                }),
        ))
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

/// Collecting the reader before budgeting decodes all 64 spans and breaks the read-count
/// assertions; changing the resume cut loses or duplicates prints in the independent sequence.
#[test]
fn a_paged_pull_reads_only_through_its_cut_and_delivers_every_print_once() {
    let mut from = 0;
    let mut delivered = Vec::new();
    let end = 64 * 1_000 - 1;
    let mut pages = 0;
    loop {
        let reads = std::cell::RefCell::new(Vec::new());
        let page = answer_tape(&[want(&[(from, end)])], 512, |_, _, a, b| {
            Ok((0..64)
                .filter(move |i| i * 1_000 + 999 >= a && i * 1_000 <= b)
                .map(|i| {
                    reads.borrow_mut().push(i);
                    Ok(StoredSpan {
                        from_ms: i * 1_000,
                        to_ms: i * 1_000 + 999,
                        ticks: (0..400).map(|j| tick(i * 1_000 + j / 2)).collect(),
                        source: TileSource::Core,
                    })
                }))
        })
        .unwrap();
        pages += 1;
        delivered.extend(page.pieces.iter().flat_map(times));
        let reads = reads.into_inner();
        assert!(reads.len() <= page.pieces.len() + 1, "read past the cut");
        assert_eq!(
            reads[0],
            from / 1_000,
            "decoded a span before the page start"
        );
        if let Some(resume) = page.resume {
            assert_eq!(*reads.last().unwrap(), resume.from_ms / 1_000);
            assert!(resume.from_ms > from);
            from = resume.from_ms;
        } else {
            break;
        }
    }
    assert!(pages > 1);
    let expected: Vec<_> = (0..64)
        .flat_map(|i| (0..400).map(move |j| i * 1_000 + j / 2))
        .collect();
    assert_eq!(delivered, expected);
}

/// A reproducible synthetic tape workload; noisy values keep its compressed size substantial
/// without using any user data. The expected prints come from this generator, not the reader.
fn synthetic_ticks(span: i64) -> Vec<Tick> {
    let mut state = span as u32 + 1;
    (0..8_000)
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            Tick {
                time_ms: (span * 30_000 + i * 3) as f64,
                price: (state % 1_000_000) as f32 / 100.0,
                qty: (state / 1_000_000 + 1) as f32 / 100.0,
                side: if state & 1 == 0 {
                    Side::Buy
                } else {
                    Side::Sell
                },
            }
        })
        .collect()
}

/// Drive the real page builder against a synthetic file, optionally collecting each whole
/// remainder as the old TapeFile reader did. Count actual decoded spans and prints, not estimates.
fn measured_pull(file: &TapeFile, eager: bool) -> (usize, usize, usize) {
    let spans = std::cell::Cell::new(0usize);
    let prints = std::cell::Cell::new(0usize);
    let mut pages = 0;
    let mut from = 0;
    let mut delivered = Vec::new();
    loop {
        let page = answer_tape(
            &[want(&[(from, 240 * 30_000 - 1)])],
            TAPE_REPLY_BUDGET,
            |exchange, market, a, b| {
                let iter = file
                    .spans(exchange, market, a, b)
                    .map_err(|e| e.to_string())?
                    .map(|span| {
                        let span = span.map_err(|e| e.to_string())?;
                        spans.set(spans.get() + 1);
                        prints.set(prints.get() + span.ticks.len());
                        Ok(span)
                    });
                let iter: Box<dyn Iterator<Item = Result<StoredSpan, String>> + '_> = if eager {
                    Box::new(iter.collect::<Result<Vec<_>, _>>()?.into_iter().map(Ok))
                } else {
                    Box::new(iter)
                };
                Ok(iter)
            },
        )
        .unwrap();
        pages += 1;
        for piece in page.pieces {
            delivered.extend(
                decode_prints(&piece.prints)
                    .unwrap()
                    .into_iter()
                    .map(|t| (t.time_ms as i64, t.price.to_bits(), t.qty.to_bits(), t.side)),
            );
        }
        match page.resume {
            Some(resume) => {
                assert!(resume.from_ms > from);
                from = resume.from_ms;
            }
            None => break,
        }
    }
    let expected: Vec<_> = (0..240)
        .flat_map(synthetic_ticks)
        .map(|t| (t.time_ms as i64, t.price.to_bits(), t.qty.to_bits(), t.side))
        .collect();
    assert_eq!(delivered, expected);
    (pages, spans.get(), prints.get())
}

/// Reverting to eager collection raises full-pull decode work quadratically. Both modes use
/// identical real pagination and must deliver all synthetic prints in order exactly once.
#[test]
#[ignore = "synthetic large-file before/after measurement"]
fn synthetic_tape_paged_decode_measurement() {
    use moon_core::market::trade_replay::trade_cache::TradeCache;
    let path = std::env::temp_dir().join(format!(
        "moon-station-synthetic-{}.sqlite",
        std::process::id()
    ));
    assert!(!path.exists(), "synthetic file must be new");
    let writer = TradeCache::open_with_ceiling(path.clone(), || None).unwrap();
    for span in 0..240 {
        writer.insert(
            "x",
            "M",
            span * 30_000,
            span * 30_000 + 29_999,
            synthetic_ticks(span),
            TileSource::Core,
        );
    }
    assert!(writer.sync(std::time::Duration::from_secs(120)));
    let file = TapeFile::open(&path).unwrap();
    let before = measured_pull(&file, true);
    let after = measured_pull(&file, false);
    println!(
        "synthetic: 240 spans, 1920000 prints; eager pages/spans/prints={before:?}; lazy={after:?}"
    );
    assert_eq!(before.0, after.0);
    assert!(after.1 <= 240 + after.0);
    assert!(before.2 > after.2 * 5);
    drop(file);
    drop(writer);
    // The worker closes asynchronously after its sender disappears; allow that close to finish.
    for _ in 0..100 {
        if std::fs::remove_file(&path).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("synthetic file could not be removed: {}", path.display());
}
