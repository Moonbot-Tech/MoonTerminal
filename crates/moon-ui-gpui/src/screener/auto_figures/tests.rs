//! The auto-figures column: one scan per market, runs that a new press replaces, and the order and
//! filter the column applies.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use moon_chart::auto_figures::scan::{ScanAnswer, ScanFilter, ScanHit, ScanLine, ScanRow};
use moon_core::config::auto_figures::{
    AutoEvent, DetectorParams, EventMarks, RoundGrid, RoundLevelsParams, Take, TfId,
};
use moon_core::market::{ChartCandle, ScreenerRow};
use moon_core::session::CoreId;

use super::super::table::Entry;
use super::{
    Answer, MarketScan, ScanBook, ScanLabels, ScanQuery, ScanSource, ScanTarget, ScanWork,
    cell_text, read_span_ms, scan_targets, sort_by_scan,
};

/// 2024-01-01 00:00 UTC, a Monday and an hour boundary.
const MONDAY_MS: i64 = 1_704_067_200_000;
const HOUR_MS: i64 = 3_600_000;

/// A Screener row of `core` under `provider` for `market`.
fn entry(provider: CoreId, core: CoreId, market: &str) -> Entry {
    Entry {
        row: ScreenerRow {
            provider,
            core,
            market: market.to_string(),
            ..Default::default()
        }
        .into(),
        core_name: "".into(),
        market_uc: market.to_uppercase().into(),
        coin_uc: "".into(),
        venue: Default::default(),
    }
}

/// A scanned answer whose best event is `best` (if any) and whose nearest line is `distance` % away.
fn scanned(best: Option<AutoEvent>, distance: Option<f64>) -> MarketScan {
    let line = distance.map(|distance_pct| ScanLine {
        row_ix: 0,
        tf: TfId::H1,
        price: 1.0,
        distance_pct,
    });
    MarketScan::Scanned(ScanAnswer {
        best: best.map(|event| ScanHit {
            event,
            line: line.expect("an event is on a line"),
        }),
        nearest: line,
        rows_scanned: 1,
        lines: u32::from(line.is_some()),
        ..Default::default()
    })
}

/// Too little cache to say anything.
fn short() -> MarketScan {
    MarketScan::Scanned(ScanAnswer {
        rows_short: 1,
        short_history: true,
        ..Default::default()
    })
}

/// A book of one finished run holding `answers`, all under provider 1.
fn book(answers: &[(&str, MarketScan)]) -> ScanBook {
    let mut book = ScanBook::default();
    let run = book.begin(MONDAY_MS, answers.len());
    let answers: Vec<Answer> = answers
        .iter()
        .map(|(market, scan)| (1, market.to_string(), *scan))
        .collect();
    assert!(book.apply(run, answers));
    book
}

fn labels() -> ScanLabels {
    ScanLabels::new(
        |event| format!("{event:?}"),
        "nothing".into(),
        "short".into(),
        "no cache".into(),
        "unread".into(),
    )
}

/// The column orders by the best event's rank, then by how near the nearest line is, then the
/// markets with a line and no event, those scanned with nothing, and last the ones that could not
/// answer and those not reached — and descending is exactly that, reversed.
///
/// By hand: a squeeze outranks a false break whatever the distance (rank 0 against 1); two false
/// breaks go by |distance| (0.2 before −0.9); a line with no event, then a market with no line,
/// then short history, unread, no cache, and a market the book never saw.
///
/// Breakage: sorting by the signed distance (−0.9 before 0.2), by distance before rank, or letting
/// the statuses sort as "best".
#[test]
fn the_column_sorts_by_event_rank_then_distance() {
    let book = book(&[
        ("NOEVENT", scanned(None, Some(0.1))),
        ("FB_FAR", scanned(Some(AutoEvent::FalseBreak), Some(-0.9))),
        ("UNREAD", MarketScan::Unread),
        ("SQUEEZE", scanned(Some(AutoEvent::Squeeze), Some(3.0))),
        ("NOCACHE", MarketScan::NoCache),
        ("NOTHING", scanned(None, None)),
        ("FB_NEAR", scanned(Some(AutoEvent::FalseBreak), Some(0.2))),
        ("SHORT", short()),
    ]);
    let markets = [
        "NOEVENT", "FB_FAR", "UNSEEN", "UNREAD", "SQUEEZE", "NOCACHE", "NOTHING", "FB_NEAR",
        "SHORT",
    ];
    let entries: Vec<_> = markets.iter().map(|m| entry(1, 1, m)).collect();
    let sorted = |desc: bool| {
        let mut order: Vec<_> = (0..entries.len()).collect();
        sort_by_scan(&book, &entries, &mut order, desc);
        order
            .into_iter()
            .map(|i| entries[i].row.market.clone())
            .collect::<Vec<_>>()
    };
    let ascending = [
        "SQUEEZE", "FB_NEAR", "FB_FAR", "NOEVENT", "NOTHING", "SHORT", "UNREAD", "NOCACHE",
        "UNSEEN",
    ];
    assert_eq!(sorted(false), ascending);
    let mut descending = ascending;
    descending.reverse();
    assert_eq!(sorted(true), descending);
}

/// The approach filter passes a market whose nearest line is within X % on either side, and no
/// market without a scanned answer; the event filters go by the best event alone.
///
/// Breakage: the filter reading the signed distance (−0.4 fails any positive X), passing a market
/// with no line or no answer, or `Any` hiding the unanswered rows.
#[test]
fn the_filter_reads_the_distance_and_the_best_event() {
    let book = Rc::new(book(&[
        ("NEAR_BELOW", scanned(Some(AutoEvent::Breakout), Some(-0.4))),
        ("NEAR_ABOVE", scanned(None, Some(0.5))),
        ("FAR", scanned(Some(AutoEvent::Squeeze), Some(0.6))),
        ("NOLINE", scanned(None, None)),
        ("SHORT", short()),
        ("NOCACHE", MarketScan::NoCache),
    ]));
    let markets = [
        "NEAR_BELOW",
        "NEAR_ABOVE",
        "FAR",
        "NOLINE",
        "SHORT",
        "NOCACHE",
        "UNSEEN",
    ];
    let passing = |filter: ScanFilter| {
        let query = ScanQuery::new(book.clone(), filter);
        markets
            .iter()
            .filter(|m| query.admits(&entry(1, 1, m)))
            .copied()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        passing(ScanFilter::Approach { max_pct: 0.5 }),
        ["NEAR_BELOW", "NEAR_ABOVE"]
    );
    assert_eq!(
        passing(ScanFilter::Approach { max_pct: 0.39 }),
        Vec::<&str>::new()
    );
    assert_eq!(passing(ScanFilter::Breakout), ["NEAR_BELOW"]);
    assert_eq!(passing(ScanFilter::Squeeze), ["FAR"]);
    assert_eq!(passing(ScanFilter::Any), markets);
}

/// Before the first press an event filter shows nothing (the window then asks for a press), and
/// `Any` shows everything.
///
/// Breakage: a filter that passes every row until a scan exists — "Squeeze" would list every coin
/// as one.
#[test]
fn an_event_filter_before_the_first_press_shows_nothing() {
    let empty = Rc::new(ScanBook::default());
    let row = entry(1, 1, "BTCUSDT");
    assert!(!ScanQuery::new(empty.clone(), ScanFilter::Squeeze).admits(&row));
    assert!(ScanQuery::new(empty, ScanFilter::Any).admits(&row));
}

/// A source that counts reads, with markets on two exchange keys.
#[derive(Default)]
struct Fake {
    /// (core, market) → exchange key; a pair missing here has no cache address.
    exchanges: HashMap<(CoreId, String), String>,
    /// (exchange, market) → its 5-minute rows; a pair missing here is a read that did not happen.
    bars: HashMap<(String, String), Vec<ChartCandle>>,
    reads: RefCell<Vec<(String, String, i64, i64)>>,
}

impl ScanSource for Fake {
    fn exchange(&self, core: CoreId, market: &str) -> Option<String> {
        self.exchanges.get(&(core, market.to_string())).cloned()
    }

    fn read_5m(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Option<Vec<ChartCandle>> {
        self.reads
            .borrow_mut()
            .push((exchange.to_string(), market.to_string(), from_ms, to_ms));
        let rows = self.bars.get(&(exchange.to_string(), market.to_string()))?;
        Some(
            rows.iter()
                .filter(|b| (from_ms as f64..=to_ms as f64).contains(&b.t_open_ms))
                .copied()
                .collect(),
        )
    }
}

/// `RoundLevels` on the half grid within 10 % on `tf`: on closes around 3.5 the only line is 3.5.
fn round_row(tf: TfId) -> ScanRow {
    ScanRow {
        row_ix: 0,
        params: DetectorParams::RoundLevels(RoundLevelsParams {
            grid: RoundGrid::Half,
            range_pct: 10.0,
        }),
        tf,
        take: Take::default(),
        marks: EventMarks::default(),
    }
}

/// Twelve flat 5-minute rows per hour from [`MONDAY_MS`], each hour's rows at its close.
fn hourly(closes: &[f32]) -> Vec<ChartCandle> {
    closes
        .iter()
        .enumerate()
        .flat_map(|(h, &c)| {
            (0..12).map(move |j| ChartCandle {
                t_open_ms: (MONDAY_MS + (h as i64 * 12 + j) * 300_000) as f64,
                open: c,
                high: c,
                low: c,
                close: c,
                volume: 1.0,
                quote_volume: 1.0,
            })
        })
        .collect()
}

/// A market shown by several cores is read and scanned ONCE: twice under one provider (two member
/// cores of one exchange), and once more under a second provider on the same exchange key. Every
/// (provider, market) gets the one answer; a market with no address is "no cache" and read never;
/// a read that did not happen is "unread".
///
/// The answer is the scan's own: closes 3.4 ×3 then 3.55 cross the 3.5 level on the last hour — a
/// breakout, its line −1.408 % from the price — read through the window `read_span_ms` gives.
///
/// Breakage: scanning per row instead of per market (three reads of `ACEUSDT`), keying the dedupe
/// by provider alone (two reads), answering only the first provider, or reading the no-cache
/// market.
#[test]
fn a_market_is_scanned_once_whatever_shows_it() {
    let entries = vec![
        entry(10, 10, "ACEUSDT"),
        entry(10, 11, "ACEUSDT"),
        entry(20, 20, "ACEUSDT"),
        entry(10, 10, "NOCACHEUSDT"),
        entry(10, 11, "NOCACHEUSDT"),
        entry(10, 10, "LOSTUSDT"),
    ];
    let targets = scan_targets(&entries);
    assert_eq!(
        targets,
        [
            ScanTarget {
                provider: 10,
                core: 10,
                market: "ACEUSDT".into()
            },
            ScanTarget {
                provider: 20,
                core: 20,
                market: "ACEUSDT".into()
            },
            ScanTarget {
                provider: 10,
                core: 10,
                market: "NOCACHEUSDT".into()
            },
            ScanTarget {
                provider: 10,
                core: 10,
                market: "LOSTUSDT".into()
            },
        ]
    );

    let mut fake = Fake::default();
    for (core, market) in [(10, "ACEUSDT"), (20, "ACEUSDT"), (10, "LOSTUSDT")] {
        fake.exchanges
            .insert((core, market.to_string()), "binance:00000000".to_string());
    }
    fake.bars.insert(
        ("binance:00000000".into(), "ACEUSDT".into()),
        hourly(&[3.4, 3.4, 3.4, 3.55]),
    );
    let now = MONDAY_MS + 4 * HOUR_MS;
    let rows = vec![round_row(TfId::H1)];
    let span = read_span_ms(&rows);
    let mut work = ScanWork::new(targets, rows, now);
    let answers = work.step(&fake, Duration::MAX);
    assert!(work.finished());

    let reads = fake.reads.borrow();
    assert_eq!(
        reads
            .iter()
            .map(|(_, market, ..)| market.as_str())
            .collect::<Vec<_>>(),
        ["ACEUSDT", "LOSTUSDT"]
    );
    assert_eq!((reads[0].2, reads[0].3), (now - span, now));

    let mut book = ScanBook::default();
    let run = book.begin(now, 4);
    assert!(book.apply(run, answers));
    assert_eq!(book.progress(), None, "every pair answered");
    for provider in [10, 20] {
        let Some(MarketScan::Scanned(answer)) = book.get(provider, "ACEUSDT") else {
            panic!("provider {provider} has no answer");
        };
        assert_eq!(answer.best.map(|b| b.event), Some(AutoEvent::Breakout));
        let d = answer.nearest.expect("the 3.5 line").distance_pct;
        assert!((d + 1.408_45).abs() < 1e-4, "{d}");
    }
    assert_eq!(book.get(10, "NOCACHEUSDT"), Some(MarketScan::NoCache));
    assert_eq!(book.get(10, "LOSTUSDT"), Some(MarketScan::Unread));
}

/// A slice with no budget left still scans one market, so a run always moves, and the answers come
/// in slice by slice until the run is finished.
///
/// Breakage: a budget check before the first market (a slow cache would stall the run forever), or
/// `finished` turning true while markets are left.
#[test]
fn every_slice_moves_the_run() {
    let mut fake = Fake::default();
    let mut entries = Vec::new();
    for market in ["AUSDT", "BUSDT", "CUSDT"] {
        fake.exchanges
            .insert((1, market.to_string()), "ex".to_string());
        fake.bars
            .insert(("ex".into(), market.into()), hourly(&[3.4, 3.55]));
        entries.push(entry(1, 1, market));
    }
    let mut work = ScanWork::new(
        scan_targets(&entries),
        vec![round_row(TfId::H1)],
        MONDAY_MS + 2 * HOUR_MS,
    );
    let mut slices = Vec::new();
    while !work.finished() {
        slices.push(work.step(&fake, Duration::ZERO).len());
        assert!(slices.len() <= 3, "a slice that scans nothing");
    }
    assert_eq!(slices, [1, 1, 1]);
}

/// A second press replaces the first run: its earlier answers go, and a slice the old run still
/// delivers afterwards is dropped rather than mixed into the new answers.
///
/// Breakage: `begin` keeping the old answers (the column would show a coin's previous event as
/// this press's), or `apply` taking a slice by provider and market alone.
#[test]
fn a_new_press_drops_the_old_run_and_its_late_slices() {
    let mut book = ScanBook::default();
    assert_eq!(book.progress(), None);
    let first = book.begin(MONDAY_MS, 2);
    assert!(book.apply(
        first,
        vec![(
            1,
            "AUSDT".into(),
            scanned(Some(AutoEvent::Squeeze), Some(0.1))
        )]
    ));
    assert_eq!(book.progress(), Some((1, 2)));

    let second = book.begin(MONDAY_MS + HOUR_MS, 2);
    assert_eq!(book.get(1, "AUSDT"), None, "the old answers went");
    assert_eq!(book.as_of_ms(), Some(MONDAY_MS + HOUR_MS));
    assert!(!book.apply(first, vec![(1, "BUSDT".into(), MarketScan::NoCache)]));
    assert_eq!(book.get(1, "BUSDT"), None);
    assert_eq!(book.progress(), Some((0, 2)));

    assert!(book.apply(
        second,
        vec![
            (1, "AUSDT".into(), MarketScan::NoCache),
            (1, "BUSDT".into(), MarketScan::Unread),
        ]
    ));
    assert_eq!(book.progress(), None);
    assert_eq!(book.get(1, "AUSDT"), Some(MarketScan::NoCache));
}

/// What a cell reads: the event and the signed distance, "nothing" with the distance, the status
/// words, the pending mark while a run is on, and the dash otherwise.
///
/// Breakage: the distance losing its sign, the best event's name taken by the wrong index, or a
/// market the run has not reached shown as "nothing".
#[test]
fn a_cell_reads_the_event_and_the_signed_distance() {
    let l = labels();
    let text = |scan, running| cell_text(scan, running, &l);
    assert_eq!(
        text(
            Some(scanned(Some(AutoEvent::FalseBreak), Some(0.4234))),
            false
        ),
        "FalseBreak +0.42%"
    );
    assert_eq!(
        text(
            Some(scanned(Some(AutoEvent::Confluence), Some(-1.5))),
            false
        ),
        "Confluence -1.50%"
    );
    assert_eq!(
        text(Some(scanned(None, Some(-0.8))), false),
        "nothing -0.80%"
    );
    assert_eq!(text(Some(scanned(None, None)), false), "nothing");
    assert_eq!(text(Some(short()), false), "short");
    assert_eq!(text(Some(MarketScan::NoCache), false), "no cache");
    assert_eq!(text(Some(MarketScan::Unread), false), "unread");
    assert_eq!(text(None, true), "…");
    assert_eq!(text(None, false), "—");
}
