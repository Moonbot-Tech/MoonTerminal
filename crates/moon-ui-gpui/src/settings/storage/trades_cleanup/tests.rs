//! The keep map off a fixed set of rows: margins per row, the union of overlapping claims, the
//! name fallback for an offline core, and what counts as unresolved.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro re-exported through the
// parent's `use gpui::*` and shadow `#[test]`.
use std::collections::HashMap;
use std::sync::Arc;

use super::{
    ClaimStamps, Coverage, Inventory, KeepMap, Margins, OWNER_SLACK_S, ReportAxis, ReportStamp,
    TapeCount, TapeOwner, build_keep, count_with_tape, read_tape_owners, stamps, trade_cache,
};

const MARGINS: Margins = Margins {
    margin_ms: 60_000,
    long_position_ms: 5 * 60_000,
};

/// A strategy-kind read failure must leave the archive untouched; complete empty owners may trim.
#[test]
fn failed_owner_metadata_never_applies_a_keep_map() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let cache = trade_cache::TradeCache::open_with_ceiling(":memory:".into(), || None).unwrap();
    cache.insert(
        "x",
        "M",
        1,
        10,
        vec![],
        moon_core::market::trade_replay::TileSource::Venue,
    );
    assert!(cache.sync(std::time::Duration::from_secs(5)));
    let context = super::CleanupContext {
        source: moon_core::market::MarketDataSource::new(moon_core::market::MarketStore::shared(
            0.0,
        )),
        quotes: HashMap::new(),
        axis: ReportAxis::default(),
    };
    assert!(
        context
            .run_with(&cache, &ReportAxis::default(), |_, _, _| anyhow::bail!(
                "strategy kinds read failed"
            ))
            .is_err()
    );
    assert_eq!(cache.held_spans("x", "M", 0, 20).unwrap(), vec![(1, 10)]);
    let result = context
        .run_with(&cache, &ReportAxis::default(), |_, _, _| Ok(vec![]))
        .unwrap();
    assert_eq!(result.report.spans_dropped, 1);
    assert!(cache.held_spans("x", "M", 0, 20).unwrap().is_empty());
}

fn owner(core_uid: u64, coin: &str, buy_ms: i64, close_ms: i64, kind: &str) -> TapeOwner {
    TapeOwner {
        core_uid,
        coin: coin.into(),
        buy: ReportStamp::Millis(buy_ms),
        close: ReportStamp::Millis(close_ms),
        strategy_id: 42,
        sell_reason: "Sell Price".into(),
        buy_set_ms: None,
        kind: kind.into(),
    }
}

fn key(exchange: &str, market: &str) -> (String, String) {
    (exchange.into(), market.into())
}

fn inventory(keys: &[(&str, &str)]) -> Inventory {
    Inventory {
        keys: keys.iter().map(|(e, m)| key(e, m)).collect(),
        range_ms: Some((0, i64::MAX / 2)),
    }
}

fn quotes() -> HashMap<u64, String> {
    HashMap::from([(1, "USDT".to_string()), (2, "USDT".to_string())])
}

fn spans(keep: &KeepMap, k: &(String, String)) -> Vec<(i64, i64)> {
    keep.get(k).map(|c| c.spans().to_vec()).unwrap_or_default()
}

/// A row the catalog names claims its window's focus at the margin, on the market the catalog
/// named.
#[test]
fn a_live_row_claims_at_the_margin() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let ace = owner(1, "ACE", 1_000_000, 1_010_000, "MoonShot");
    let ben = owner(1, "BEN", 5_000_000, 5_010_000, "MoonShot");
    let (keep, preview) = build_keep(
        &inv,
        &[&ace, &ben],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |o| Some(key("4:0", &format!("{}USDT", o.coin))),
    );
    assert_eq!(
        spans(&keep, &key("4:0", "ACEUSDT")),
        vec![(1_000_000 - 60_000, 1_010_000 + 60_000)]
    );
    assert_eq!(
        spans(&keep, &key("4:0", "BENUSDT")),
        vec![(5_000_000 - 60_000, 5_010_000 + 60_000)]
    );
    assert_eq!(
        (preview.claimants, preview.by_name, preview.unresolved),
        (2, 0, 0)
    );
}

/// Two rows whose claims overlap are one stretch: the ground between them is kept.
#[test]
fn overlapping_claims_are_one_stretch() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let first = owner(1, "ACE", 1_000_000, 1_010_000, "");
    let second = owner(1, "ACE", 1_012_000, 1_020_000, "");
    let apart = owner(1, "ACE", 2_000_000, 2_001_000, "");
    let (keep, _) = build_keep(
        &inv,
        &[&first, &second, &apart],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |_| Some(key("4:0", "ACEUSDT")),
    );
    assert_eq!(
        spans(&keep, &key("4:0", "ACEUSDT")),
        vec![(940_000, 1_080_000), (1_940_000, 2_061_000)]
    );
}

/// A row that carries its entry order's creation claims from there, as the tuner fetches it
/// (`model_window_at`) — for every kind the tuner runs on, a model or none; one whose creation
/// and position together outrun the long-position threshold claims from its fill, as before.
#[test]
fn a_row_with_its_orders_creation_claims_from_the_creation() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let mut hook = owner(1, "ACE", 1_000_000, 1_010_000, "MoonHook");
    hook.buy_set_ms = Some(1_000_000 - 120_000);
    let (keep, _) = build_keep(
        &inv,
        &[&hook],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |_| Some(key("4:0", "ACEUSDT")),
    );
    assert_eq!(
        spans(&keep, &key("4:0", "ACEUSDT")),
        vec![(1_000_000 - 120_000 - 60_000, 1_010_000 + 60_000)]
    );
    let mut long = owner(1, "ACE", 1_000_000, 1_000_000 + 4 * 60_000, "MoonShot");
    long.buy_set_ms = Some(1_000_000 - 120_000);
    let (keep, _) = build_keep(
        &inv,
        &[&long],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |_| Some(key("4:0", "ACEUSDT")),
    );
    assert_eq!(
        spans(&keep, &key("4:0", "ACEUSDT"))[0].0,
        1_000_000 - 60_000,
        "creation to close outruns five minutes: the window opens at the fill"
    );
}

/// A position held longer than five minutes claims its two ends, the margin on both sides of
/// each, not its middle.
#[test]
fn a_long_position_claims_its_two_ends() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let long = owner(1, "ACE", 1_000_000, 1_000_000 + 3_600_000, "MoonShot");
    let (keep, _) = build_keep(
        &inv,
        &[&long],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |_| Some(key("4:0", "ACEUSDT")),
    );
    assert_eq!(
        spans(&keep, &key("4:0", "ACEUSDT")),
        vec![
            (1_000_000 - 60_000, 1_000_000 + 60_000),
            (4_600_000 - 60_000, 4_600_000 + 60_000)
        ]
    );
}

/// With the core not connected, the coin's spellings claim every market of the file they
/// name — on every exchange that spells it so.
#[test]
fn an_offline_core_claims_by_name_on_every_matching_market() {
    let inv = inventory(&[
        ("4:0", "ACEUSDT"),
        ("9:0", "ACE_USDT"),
        ("15:0", "ACE-USDT-SWAP"),
        ("4:0", "BENUSDT"),
    ]);
    let ace = owner(2, "ACE", 1_000_000, 1_010_000, "");
    let (keep, preview) = build_keep(
        &inv,
        &[&ace],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |_| None,
    );
    let mut claimed: Vec<&(String, String)> = keep.keys().collect();
    claimed.sort();
    assert_eq!(
        claimed,
        vec![
            &key("15:0", "ACE-USDT-SWAP"),
            &key("4:0", "ACEUSDT"),
            &key("9:0", "ACE_USDT")
        ]
    );
    assert_eq!((preview.by_name, preview.unresolved), (1, 0));
}

/// A row of an old replica that stores the full market name matches it as it is.
#[test]
fn a_stored_full_market_name_matches_itself() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let (keep, _) = build_keep(
        &inv,
        &[&owner(2, "aceusdt", 1_000_000, 1_010_000, "")],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |_| None,
    );
    assert_eq!(keep.len(), 1);
}

/// A row that names no market of the file claims nothing and is counted.
#[test]
fn a_row_off_the_file_is_unresolved() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let (keep, preview) = build_keep(
        &inv,
        &[&owner(2, "ZZZ", 1_000_000, 1_010_000, "")],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |_| None,
    );
    assert!(keep.is_empty());
    assert_eq!(
        (preview.claimants, preview.by_name, preview.unresolved),
        (1, 0, 1)
    );
}

/// The stamps: one claim on true UTC, lifted by the core's offset — the clock the trade window,
/// the capture and the tuner's deal read all fetch on — and never the core's raw clock.
#[test]
fn stamps_claim_the_lifted_clock_only() {
    let mut row = owner(1, "ACE", 1_000_000, 1_010_000, "");
    row.buy_set_ms = Some(990_000);
    let claim = |buy_ms, close_ms, buy_set_ms| ClaimStamps {
        buy_ms,
        close_ms,
        buy_set_ms,
    };
    assert_eq!(
        stamps(&ReportAxis::default(), &row),
        claim(1_000_000, 1_010_000, Some(990_000))
    );
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![moon_core::db::OffsetSegment {
                from_utc: 0,
                offset_secs: 3,
            }],
        )]),
        chrono_tz::UTC,
    );
    let (buy, close) = axis.stamp_pair_to_utc_ms(row.buy, row.close, 1);
    assert_ne!((buy, close), (1_000_000, 1_010_000));
    // The creation moves with the entry's clock.
    assert_eq!(
        stamps(&axis, &row),
        claim(buy, close, Some(990_000 + (buy - 1_000_000)))
    );
    let seconds = TapeOwner {
        buy: ReportStamp::Seconds(1_000),
        close: ReportStamp::Seconds(1_010),
        buy_set_ms: None,
        ..row
    };
    assert_eq!(
        stamps(&ReportAxis::default(), &seconds),
        claim(1_000_000, 1_010_000, None)
    );
}

/// The replica is read as far past the file's range as a claim reaches — the margin and an entry
/// order's longest replayed wait — rounded up to whole seconds.
#[test]
fn the_reach_is_the_margin_and_the_orders_wait_in_whole_seconds() {
    assert_eq!(MARGINS.reach_s(), 661);
    assert_eq!(
        Margins {
            margin_ms: 7_200_000,
            long_position_ms: 60_000
        }
        .reach_s(),
        7_801
    );
}

/// Count the deals of `rows` with tape, the file holding `held` on every market; also answers how
/// many held reads were made.
fn count(
    inv: &Inventory,
    rows: &[&TapeOwner],
    axis: &ReportAxis,
    live: impl FnMut(&TapeOwner) -> Option<(String, String)>,
    held: &[(i64, i64)],
) -> (TapeCount, usize) {
    let mut reads = 0;
    let counted = count_with_tape(
        inv,
        rows,
        std::slice::from_ref(axis),
        &quotes(),
        MARGINS,
        live,
        |_| {
            reads += 1;
            Ok(Some(Coverage::from_spans(held.iter().copied())))
        },
    )
    .expect("counted");
    (counted, reads)
}

/// A missing market reply must mark both counts as lower bounds and be read only once.
#[test]
fn timed_out_markets_make_tape_counts_lower_bounds() {
    let inv = inventory(&[("4:0", "ACEUSDT"), ("9:0", "ACE_USDT")]);
    let first = owner(2, "ACE", 1_000_000, 4_600_000, "MoonShot");
    let second = owner(2, "ACE", 5_000_000, 5_010_000, "MoonShot");
    let mut reads = 0;
    let got = count_with_tape(
        &inv,
        &[&first, &second],
        &[ReportAxis::default()],
        &quotes(),
        MARGINS,
        |_| None,
        |key| {
            reads += 1;
            Ok((key.0 == "4:0").then(|| Coverage::from_spans([(0, 10_000_000)])))
        },
    )
    .unwrap();
    assert_eq!(got.with_tape, 2);
    assert_eq!(got.long, 1);
    assert_eq!(got.incomplete_markets, 1);
    assert_eq!(reads, 2);
}

/// Tape inside a deal's margin only — before its entry, after its exit — makes it a deal with
/// tape; tape past the margin does not.
#[test]
fn tape_in_the_margin_alone_counts_the_deal() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let ace = owner(1, "ACE", 1_000_000, 1_010_000, "MoonShot");
    let live = |_: &TapeOwner| Some(key("4:0", "ACEUSDT"));
    let axis = ReportAxis::default();
    for (held, counted) in [
        ((1_000_000 - 60_000, 1_000_000 - 55_000), 1),
        ((1_010_000 + 59_000, 1_010_000 + 60_000), 1),
        ((1_010_000 + 60_001, 1_200_000), 0),
        ((800_000, 1_000_000 - 60_001), 0),
    ] {
        let (got, _) = count(&inv, &[&ace], &axis, live, &[held]);
        assert_eq!(got.with_tape, counted, "held {held:?}");
        assert_eq!(got.long, 0);
    }
}

/// With the core's clock offset, tape the lifted claim reaches counts the deal, tape only the raw
/// clock would reach does not, and a deal whose tape spans both is still one deal.
#[test]
fn an_offset_clock_counts_a_deal_once() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let ace = owner(1, "ACE", 1_000_000, 1_010_000, "MoonShot");
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![moon_core::db::OffsetSegment {
                from_utc: 0,
                offset_secs: 3,
            }],
        )]),
        chrono_tz::UTC,
    );
    let (buy, close) = axis.stamp_pair_to_utc_ms(ace.buy, ace.close, 1);
    let lifted_only = if buy < 1_000_000 {
        (buy - 60_000, buy - 60_000)
    } else {
        (close + 60_000, close + 60_000)
    };
    let live = |_: &TapeOwner| Some(key("4:0", "ACEUSDT"));
    assert_eq!(
        count(&inv, &[&ace], &axis, live, &[lifted_only])
            .0
            .with_tape,
        1
    );
    assert_eq!(
        count(&inv, &[&ace], &ReportAxis::default(), live, &[lifted_only])
            .0
            .with_tape,
        0,
        "without the offset nothing reaches that print"
    );
    // Past the lifted claim's end (its close plus the margin), inside the raw one's.
    let raw_only = (1_069_000, 1_069_000);
    assert!(close + MARGINS.margin_ms < raw_only.0 && raw_only.0 <= 1_010_000 + MARGINS.margin_ms);
    assert_eq!(
        count(&inv, &[&ace], &axis, live, &[raw_only]).0.with_tape,
        0,
        "the core's raw clock claims no tape once its offset is measured"
    );
    assert_eq!(
        count(&inv, &[&ace], &axis, live, &[(0, 10_000_000)]).0,
        TapeCount {
            with_tape: 1,
            long: 0,
            incomplete_markets: 0
        }
    );
}

/// An offline core the session holds no offset for, while the replica stores one: the pass
/// claims both the tuner's lifted window and the session's unlifted one, so neither path's tape
/// is cut as the other's excess, and the deal still counts once.
#[test]
fn the_replicas_and_the_sessions_clocks_both_claim() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let ace = owner(1, "ACE", 1_000_000, 1_010_000, "MoonShot");
    let persisted = ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![moon_core::db::OffsetSegment {
                from_utc: 0,
                offset_secs: 3,
            }],
        )]),
        chrono_tz::UTC,
    );
    let context = super::CleanupContext {
        source: moon_core::market::MarketDataSource::new(moon_core::market::MarketStore::shared(
            0.0,
        )),
        quotes: HashMap::new(),
        axis: ReportAxis::default(),
    };
    let axes = context.axes(&persisted);
    assert_eq!(axes.len(), 2);
    assert_eq!(
        super::CleanupContext {
            axis: persisted.clone(),
            ..context
        }
        .axes(&persisted)
        .len(),
        1
    );
    let live = |_: &TapeOwner| Some(key("4:0", "ACEUSDT"));
    // The lift moves this core 3 s earlier: the first print is inside the lifted claim only, before
    // the unlifted one starts; the second inside the unlifted claim only, past the lifted one's end.
    for print in [(937_000, 937_000), (1_069_000, 1_069_000)] {
        let got = count_with_tape(&inv, &[&ace], &axes, &quotes(), MARGINS, live, |_| {
            Ok(Some(Coverage::from_spans([print])))
        })
        .unwrap();
        assert_eq!(got.with_tape, 1, "print {print:?}");
    }
    let got = count_with_tape(&inv, &[&ace], &axes, &quotes(), MARGINS, live, |_| {
        Ok(Some(Coverage::from_spans([(0, 10_000_000)])))
    })
    .unwrap();
    assert_eq!(got.with_tape, 1, "one deal however many clocks reach it");
}

/// A long position's claim is its two ends: tape in its middle does not count it, tape at an end
/// counts it as long.
#[test]
fn a_long_split_window_counts_only_at_its_ends() {
    let inv = inventory(&[("4:0", "ACEUSDT")]);
    let long = owner(1, "ACE", 1_000_000, 1_000_000 + 3_600_000, "MoonShot");
    let live = |_: &TapeOwner| Some(key("4:0", "ACEUSDT"));
    let axis = ReportAxis::default();
    assert_eq!(
        count(&inv, &[&long], &axis, live, &[(2_000_000, 3_000_000)]).0,
        TapeCount::default()
    );
    assert_eq!(
        count(&inv, &[&long], &axis, live, &[(4_600_500, 4_600_600)]).0,
        TapeCount {
            with_tape: 1,
            long: 1,
            incomplete_markets: 0
        }
    );
}

/// An offline core's coin matching several markets of the file is one deal, and each market is
/// read once however many deals ask for it.
#[test]
fn a_deal_on_duplicate_markets_counts_once_and_reads_each_market_once() {
    let inv = inventory(&[
        ("4:0", "ACEUSDT"),
        ("9:0", "ACE_USDT"),
        ("15:0", "ACE-USDT-SWAP"),
    ]);
    let first = owner(2, "ACE", 1_000_000, 1_010_000, "");
    let second = owner(2, "ACE", 2_000_000, 2_010_000, "");
    let (got, reads) = count(
        &inv,
        &[&first, &second],
        &ReportAxis::default(),
        |_| None,
        &[(0, 10_000_000)],
    );
    assert_eq!(
        got,
        TapeCount {
            with_tape: 2,
            long: 0,
            incomplete_markets: 0
        }
    );
    assert_eq!(reads, 3);
}

/// The observation channel for the whole pass short of the live catalog: point
/// `MOON_TRADES_CLEANUP_COPY` at a directory holding a COPY of a `data/` folder
/// (`reports.sqlite`, `strategies.sqlite`, `trades.sqlite`, with their WAL files), and this
/// applies retention against that copy, printing what the pass removed from the archive.
/// Every core resolves by
/// name (no session here), so the by-name count equals the claimants. Ignored: it needs the
/// copy, and it installs the process-wide data-dir override, so it runs alone:
/// `cargo test -p moon-ui-gpui --target x86_64-pc-windows-msvc -- --ignored --nocapture
/// probe_a_copied_data_dir`.
/// The shared locale/data-dir guard serializes override installation with localized path readers;
/// the one-shot override still requires this probe to run in its own test process.
#[test]
#[ignore]
fn probe_a_copied_data_dir() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let Ok(dir) = std::env::var("MOON_TRADES_CLEANUP_COPY") else {
        eprintln!("MOON_TRADES_CLEANUP_COPY not set; nothing probed");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _locale = crate::test_locale::force("en");
    assert!(
        moon_core::config::paths::set_data_dir_override(dir.clone()),
        "the override must be installed before any path resolves"
    );
    // The replica reader is gated on the process lease the app takes at startup.
    let _permit = moon_core::db::report_recovery::prepare().expect("the replica lease");
    let trades_path = moon_core::config::paths::trades_db_path();
    let size = |what: &str| {
        let bytes = std::fs::metadata(&trades_path)
            .map(|m| m.len())
            .unwrap_or(0);
        println!("[probe] {what}: trades.sqlite is {bytes} bytes");
    };
    size("before");
    let cache = trade_cache::handle().expect("cache worker");
    let inventory = cache.inventory().expect("answered").expect("inventory");
    let (from_ms, to_ms) = inventory.range_ms.expect("a non-empty file");
    println!(
        "[probe] inventory: {} market(s), {from_ms}..{to_ms}",
        inventory.keys.len()
    );
    let margins = Margins::live();
    let slack_s = OWNER_SLACK_S + margins.reach_s();
    let started = std::time::Instant::now();
    let owners = read_tape_owners(
        from_ms.div_euclid(1_000) - slack_s,
        to_ms.div_euclid(1_000) + slack_s,
    )
    .expect("replica read");
    let tunable = owners.iter().filter(|o| o.is_tunable()).count();
    println!(
        "[probe] owners: {} row(s) in range, {tunable} tunable, read in {} ms",
        owners.len(),
        started.elapsed().as_millis()
    );
    let quotes: HashMap<u64, String> = owners
        .iter()
        .map(|o| (o.core_uid, "USDT".to_string()))
        .collect();
    println!(
        "[probe] margin: {} ms, long position from {} ms",
        margins.margin_ms,
        moon_core::market::trade_replay::long_position_ms()
    );
    {
        let claimants: Vec<&TapeOwner> = owners.iter().filter(|o| o.is_tunable()).collect();
        let started = std::time::Instant::now();
        let (keep, preview) = build_keep(
            &inventory,
            &claimants,
            &[ReportAxis::default()],
            &quotes,
            margins,
            |_| None,
        );
        let report = cache.trim(Arc::new(keep)).expect("answered").expect("trim");
        println!(
            "[probe] applied: claimants {} (by name {}, unresolved {}); spans {} total / {} dropped / {} cut; prints dropped {}; bytes {} of {}; {} ms",
            preview.claimants,
            preview.by_name,
            preview.unresolved,
            report.spans_total,
            report.spans_dropped,
            report.spans_cut,
            report.prints_dropped,
            report.bytes_dropped,
            report.bytes_total,
            started.elapsed().as_millis()
        );
        size("after apply + vacuum");
    }
}
