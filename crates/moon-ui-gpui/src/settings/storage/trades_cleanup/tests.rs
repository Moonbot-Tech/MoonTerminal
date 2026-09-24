//! The keep map off a fixed set of rows: margins per row, the union of overlapping claims, the
//! name fallback for an offline core, and what counts as unresolved.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro re-exported through the
// parent's `use gpui::*` and shadow `#[test]`.
use std::collections::HashMap;
use std::sync::Arc;

use super::{
    ClaimStamps, Inventory, KeepMap, Margins, OWNER_SLACK_S, ReportAxis, ReportStamp, TapeOwner,
    build_keep, read_tape_owners, stamps, trade_cache,
};

const MARGINS: Margins = Margins {
    margin_ms: 60_000,
    long_position_ms: 5 * 60_000,
};

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
        &ReportAxis::default(),
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
        &ReportAxis::default(),
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
        &ReportAxis::default(),
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
        &ReportAxis::default(),
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
        &ReportAxis::default(),
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
        &ReportAxis::default(),
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
        &ReportAxis::default(),
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
        &ReportAxis::default(),
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

/// The stamps: one claim when the core's clock is not offset, two when it is — the lifted pair
/// the window and the capture use, and the raw one the tuner's fetch uses.
#[test]
fn stamps_claim_both_clocks_when_they_differ() {
    let mut row = owner(1, "ACE", 1_000_000, 1_010_000, "");
    row.buy_set_ms = Some(990_000);
    let claim = |buy_ms, close_ms, buy_set_ms| ClaimStamps {
        buy_ms,
        close_ms,
        buy_set_ms,
    };
    assert_eq!(
        stamps(&ReportAxis::default(), &row),
        vec![claim(1_000_000, 1_010_000, Some(990_000))]
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
        vec![
            claim(buy, close, Some(990_000 + (buy - 1_000_000))),
            claim(1_000_000, 1_010_000, Some(990_000))
        ]
    );
    let seconds = TapeOwner {
        buy: ReportStamp::Seconds(1_000),
        close: ReportStamp::Seconds(1_010),
        buy_set_ms: None,
        ..row
    };
    assert_eq!(
        stamps(&ReportAxis::default(), &seconds),
        vec![claim(1_000_000, 1_010_000, None)]
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

/// The observation channel for the whole pass short of the live catalog: point
/// `MOON_TRADES_CLEANUP_COPY` at a directory holding a COPY of a `data/` folder
/// (`reports.sqlite`, `strategies.sqlite`, `trades.sqlite`, with their WAL files), and this
/// runs the count and then the apply against that copy, printing what each found —
/// the numbers the tab would show, off a real replica and a real file. Every core resolves by
/// name (no session here), so the by-name count equals the claimants. Ignored: it needs the
/// copy, and it installs the process-wide data-dir override, so it runs alone:
/// `cargo test -p moon-ui-gpui --target x86_64-pc-windows-msvc -- --ignored --nocapture
/// probe_a_copied_data_dir`.
#[test]
#[ignore]
fn probe_a_copied_data_dir() {
    let Ok(dir) = std::env::var("MOON_TRADES_CLEANUP_COPY") else {
        eprintln!("MOON_TRADES_CLEANUP_COPY not set; nothing probed");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
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
    let cache = trade_cache::maintenance_handle().expect("cache worker");
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
    for apply in [false, true] {
        let claimants: Vec<&TapeOwner> = owners.iter().filter(|o| o.is_tunable()).collect();
        let started = std::time::Instant::now();
        let (keep, preview) = build_keep(
            &inventory,
            &claimants,
            &ReportAxis::default(),
            &quotes,
            margins,
            |_| None,
        );
        let report = cache
            .trim(Arc::new(keep), apply)
            .expect("answered")
            .expect("trim");
        println!(
            "[probe] apply={apply}: claimants {} (by name {}, unresolved {}); spans {} total / {} dropped / {} cut; prints dropped {}; bytes {} of {}; {} ms",
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
        if apply {
            size("after apply + vacuum");
            let again = cache
                .trim(Arc::new(KeepMap::new()), false)
                .expect("answered")
                .expect("recount");
            println!(
                "[probe] file now holds {} span(s), {} bytes",
                again.spans_total, again.bytes_total
            );
        }
    }
}
