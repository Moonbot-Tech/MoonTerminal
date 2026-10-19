//! Optional offline measurements using copied reports and a read-only fake station tape.

use super::super::super::station::{Filled, lacking, prefill_rows};
use super::super::*;
use super::{FakeSources, RestoreAutoload};
use moon_core::market::trade_replay::{
    Coverage,
    trade_cache::{TapeFile, TradeCache},
};
use std::collections::HashSet;

/// Remove only the uniquely created scratch tree, after its cache worker has closed.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    /// Delete only the unique directory created by this test.
    fn drop(&mut self) {
        // Windows workers release their connections asynchronously; leave a busy scratch tree.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Read explicit identities using the existing real-data probe's log announcement syntax.
/// Tape availability cannot identify a core's exchange.
fn copied_exchanges(input: &std::path::Path) -> HashMap<String, String> {
    let manifest = input.join("core-exchanges.json");
    if manifest.exists() {
        return serde_json::from_slice(&std::fs::read(manifest).unwrap())
            .expect("core-exchanges.json must map core UID strings to exchange keys");
    }
    let mut exchanges = HashMap::new();
    for entry in std::fs::read_dir(input.join("logs"))
        .expect("copied identity logs or core-exchanges.json are required")
    {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_file() {
            continue;
        }
        let text =
            std::fs::read_to_string(entry.path()).expect("copied log must be readable UTF-8");
        for line in text
            .lines()
            .filter(|line| line.contains("identity: exchange_code="))
        {
            let Some((_, core)) = line.split_once("core ") else {
                continue;
            };
            let Some(uid) = core
                .split_whitespace()
                .next()
                .and_then(|uid| uid.parse::<u64>().ok())
            else {
                continue;
            };
            let Some((_, identity)) = line.split_once("ExchangeId { code: ") else {
                continue;
            };
            let Some((code, dex)) = identity.split_once(", dex: ") else {
                continue;
            };
            let (Ok(code), Some(dex)) = (
                code.parse::<u8>(),
                dex.split_whitespace()
                    .next()
                    .and_then(|dex| dex.parse::<u32>().ok()),
            ) else {
                continue;
            };
            let key = format!("{code}:{dex:08x}");
            if let Some(previous) = exchanges.insert(uid.to_string(), key.clone()) {
                assert!(previous == key, "copied logs disagree on core {uid}");
            }
        }
    }
    assert!(
        !exchanges.is_empty(),
        "no explicit core exchange identities in copied logs"
    );
    exchanges
}

/// Read copied report history through the normal query and widen a separate disposable cache.
/// Removing a fill leg must reduce measured coverage or leave a required station request unpaid.
#[test]
#[ignore = "requires MOON_TAPE_COPY_DIR and a dedicated test process"]
fn replay_over_data_copy() {
    let input = std::path::PathBuf::from(
        std::env::var_os("MOON_TAPE_COPY_DIR")
            .expect("MOON_TAPE_COPY_DIR must name a quiescent copy of report, strategy and tape databases plus explicit core identities"),
    );
    let scratch = Scratch(std::env::temp_dir().join(format!(
            "moon-tape-replay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir_all(&scratch.0).unwrap();
    assert!(
        moon_core::config::paths::set_data_dir_override(scratch.0.clone()),
        "run this ignored test alone"
    );
    let database_dir = moon_core::config::paths::db_dir();
    for name in [
        "reports.sqlite",
        "reports.sqlite-wal",
        "strategies.sqlite",
        "strategies.sqlite-wal",
        "trades.sqlite",
        "trades.sqlite-wal",
    ] {
        let path = input.join(name);
        if path.exists() {
            std::fs::copy(path, database_dir.join(name)).unwrap();
        }
    }
    assert!(
        database_dir.join("reports.sqlite").is_file(),
        "copied report metadata is required"
    );
    assert!(
        database_dir.join("strategies.sqlite").is_file(),
        "copied strategy kinds are required by the production report query"
    );
    let exchanges = copied_exchanges(&input);
    let _permit = moon_core::db::report_recovery::prepare()
        .expect("the disposable report copy must pass recovery preflight");
    let _restore = RestoreAutoload::take();
    moon_core::market::trade_replay::set_margin_s(600);
    moon_core::market::trade_replay::set_long_position_min(30);
    let started = Instant::now();
    let query = moon_core::db::analytics::Query {
        from: 0,
        to: moon_core::util::time::now_unix_ms_i64() / 1000 + 86_400,
        metric: moon_core::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let deals = moon_core::db::tuner::ticks::read_deals(&query)
        .expect("copied report query")
        .deals;
    assert!(!deals.is_empty(), "no replayable deals in copied reports");
    // Even SQLite's read-only mode may create WAL/SHM sidecars in a writable directory.
    // Open only the second disposable copy so the supplied directory stays byte-for-byte intact.
    let file = TapeFile::open(&database_dir.join("trades.sqlite")).unwrap();
    let conn = rusqlite::Connection::open_with_flags(
        database_dir.join("trades.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let keys: Vec<(String, String)> = conn
        .prepare("SELECT exchange, market FROM packs UNION SELECT exchange, market FROM spans")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut resolver = super::resolver();
    resolver.addresses.clear();
    let cores: HashSet<_> = deals.iter().map(|deal| deal.core_uid).collect();
    resolver
        .source
        .set_provider_map(&cores.iter().map(|&core| (core, core)).collect());
    resolver.source.set_provider_exchanges(
        &cores
            .iter()
            .map(|&core| {
                (
                    core,
                    moon_core::feed::ExchangeId::new(moon_core::fixture::unlisted::CODE),
                )
            })
            .collect(),
    );
    resolver.source.set_core_venues(
        &cores
            .iter()
            .map(|&core| {
                (
                    core,
                    moon_core::venue::CoreVenue::identify(
                        moon_core::fixture::unlisted::CODE,
                        "",
                        Some(moon_core::fixture::unlisted::CAPTION),
                    ),
                )
            })
            .collect(),
    );
    let cache = TradeCache::open_with_ceiling(scratch.0.join("replay.sqlite"), || None).unwrap();
    let mut selected = Vec::new();
    let mut ambiguous = 0;
    let mut unknown_core = 0;
    let mut bytes_180 = 0usize;
    let mut bytes_600 = 0usize;
    for deal in &deals {
        // Preserve the deal's known exchange; tape availability cannot identify a core's venue.
        let Some(exchange) = exchanges.get(&deal.core_uid.to_string()) else {
            unknown_core += 1;
            continue;
        };
        let coin = deal.coin.to_ascii_uppercase();
        let matches: Vec<_> = keys
            .iter()
            .filter(|(key, market)| {
                let market = market.to_ascii_uppercase().replace(['-', '_', '/'], "");
                key == exchange
                    && (market == coin
                        || market == format!("{coin}USDT")
                        || market == format!("{coin}USD"))
            })
            .collect();
        if matches.len() != 1 {
            ambiguous += 1;
            continue;
        }
        let (exchange, market) = matches[0];
        let Some(old) = model_window(deal, 180_000, 30 * 60_000) else {
            continue;
        };
        let Some(new) = model_window(deal, 600_000, 30 * 60_000) else {
            continue;
        };
        resolver.addresses.insert(
            (deal.core_uid, deal.coin.clone()),
            Some(std::sync::Arc::new(
                crate::analytics::tuner::ticks::state::RowAddress {
                    core_uid: deal.core_uid,
                    venue: None,
                    exchange_key: exchange.clone(),
                    market: market.clone(),
                    btc_market: None,
                },
            )),
        );
        for (window, bytes, seed) in [(old, &mut bytes_180, true), (new, &mut bytes_600, false)] {
            for &(from, to) in window.focus_spans().spans() {
                for span in file.spans(exchange, market, from, to).unwrap() {
                    let span = span.unwrap();
                    let (lo, hi) = (from.max(span.from_ms), to.min(span.to_ms));
                    let ticks: Vec<_> = span
                        .ticks
                        .into_iter()
                        .filter(|t| t.time_ms >= lo as f64 && t.time_ms <= hi as f64)
                        .collect();
                    *bytes += moon_core::market::trade_replay::trade_cache::encode_prints(&ticks)
                        .len()
                        * 3
                        / 4;
                    if seed {
                        cache.insert_from_station(exchange, market, lo, hi, ticks);
                    }
                }
            }
        }
        selected.push(deal.clone());
    }
    assert!(
        !selected.is_empty(),
        "no unambiguous tape markets for copied deals"
    );
    assert!(cache.sync(Duration::from_secs(30)));
    let place = |row: &QueuedRow| {
        (
            row.address.exchange_key.clone(),
            row.address.market.clone(),
            row.window,
        )
    };
    let held =
        |exchange: &str, market: &str, from, to| cache.held_spans(exchange, market, from, to);
    let rows: Vec<_> = selected
        .iter()
        .filter_map(|d| {
            let address = resolver.address(d)?;
            resolver.queued_row(d.clone(), address)
        })
        .collect();
    assert_eq!(rows.len(), selected.len());
    let covered = |margin| {
        rows.iter()
            .filter(|row| {
                let window = model_window(&row.deal, margin, 30 * 60_000).unwrap();
                let coverage = Coverage::from_spans(
                    held(
                        &row.address.exchange_key,
                        &row.address.market,
                        window.from_ms,
                        window.to_ms,
                    )
                    .unwrap(),
                );
                coverage.covers(&window.focus_spans())
            })
            .count()
    };
    let before = covered(180_000);
    let needed: HashSet<_> = rows
        .iter()
        .filter(|row| {
            let coverage = Coverage::from_spans(
                held(
                    &row.address.exchange_key,
                    &row.address.market,
                    row.window.from_ms,
                    row.window.to_ms,
                )
                .unwrap(),
            );
            !coverage.covers(&row.window.focus_spans())
        })
        .map(|row| (row.address.exchange_key.clone(), row.address.market.clone()))
        .collect();
    let issued = std::cell::Cell::new(0usize);
    let mut sources = FakeSources {
        deals: selected.clone(),
        from: None,
        venue_rows: 0,
        core_rows: 0,
        prefill: Box::new(|rows, _, _| {
            let (rows, held_count, station_count, _) = prefill_rows(
                rows,
                place,
                held,
                |_, _| false,
                |rows| {
                    let wants = lacking(rows, place, held);
                    issued.set(issued.get() + wants.len());
                    let mut pieces = 0;
                    for want in wants {
                        for (from, to) in want.spans {
                            for span in file.spans(&want.exchange, &want.market, from, to).unwrap()
                            {
                                let span = span.unwrap();
                                cache.insert_from_station(
                                    &want.exchange,
                                    &want.market,
                                    from.max(span.from_ms),
                                    to.min(span.to_ms),
                                    span.ticks,
                                );
                                pieces += 1;
                            }
                        }
                    }
                    assert!(cache.sync(Duration::from_secs(30)));
                    Filled {
                        pieces,
                        ..Default::default()
                    }
                },
            );
            (rows, held_count, station_count)
        }),
    };
    let pass = Pass {
        generation: generation(),
        station: None,
        station_only: true,
        whole_history: true,
    };
    let left = run_pass_with(
        resolver,
        HashMap::new(),
        Default::default(),
        Some(selected.clone()),
        1,
        &pass,
        &mut sources,
    );
    assert_eq!(issued.get(), needed.len());
    assert_eq!((sources.venue_rows, sources.core_rows), (0, 0));
    let after = covered(600_000);
    let mut seams = 0;
    let mut duplicates = 0;
    let mut introduced_duplicates = 0;
    for (exchange, market) in &keys {
        let spans = cache.read(exchange, market, 0, i64::MAX).unwrap();
        let actual = Coverage::from_spans(spans.iter().map(|span| (span.from_ms, span.to_ms)));
        let requested = Coverage::from_spans(
            rows.iter()
                .filter(|row| {
                    row.address.exchange_key == *exchange && row.address.market == *market
                })
                .flat_map(|row| row.window.focus_spans().spans().to_vec()),
        );
        if requested.is_empty() {
            continue;
        }
        let (from, to) = requested.hull().unwrap();
        let original: Vec<_> = file
            .spans(exchange, market, from, to)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let expected = Coverage::from_spans(original.iter().map(|span| (span.from_ms, span.to_ms)))
            .clip(&requested);
        assert!(actual == expected, "copy-held coverage lost at a seam");
        for pair in spans.windows(2) {
            assert!(pair[0].to_ms < pair[1].from_ms, "overlapping filed spans");
            if pair[0].to_ms + 1 == pair[1].from_ms {
                seams += 1;
            }
        }
        let identity = |tick: &moon_core::feed::types::Tick| {
            (
                tick.time_ms.to_bits(),
                tick.price.to_bits(),
                tick.qty.to_bits(),
                format!("{:?}", tick.side),
            )
        };
        let mut expected_prints = HashMap::<_, usize>::new();
        for tick in original
            .iter()
            .flat_map(|span| &span.ticks)
            .filter(|tick| actual.contains_ms(tick.time_ms as i64))
        {
            *expected_prints.entry(identity(tick)).or_default() += 1;
        }
        let mut seen = HashMap::<_, usize>::new();
        for span in spans {
            for tick in span.ticks {
                let count = seen.entry(identity(&tick)).or_default();
                if *count > 0 {
                    duplicates += 1;
                }
                *count += 1;
            }
        }
        introduced_duplicates += seen
            .iter()
            .map(|(key, count)| count.saturating_sub(*expected_prints.get(key).unwrap_or(&0)))
            .sum::<usize>();
        assert!(
            seen == expected_prints,
            "copy print multiplicity changed during fill"
        );
    }
    assert_eq!(introduced_duplicates, 0);
    let cap_mb = std::env::var("MOON_TAPE_CAP_MB")
        .ok()
        .map(|s| s.parse::<f64>().expect("numeric cap MB"))
        .unwrap_or(moon_core::market::trade_replay::trade_cache::TRADES_MAX_MB as f64);
    println!(
        "[replay] deals={} matched={} unmatched_or_ambiguous={} unknown_core={unknown_core} covered_180_before={before} covered_600_after={after} unresolved={} seams_checked={seams} duplicate_prints={duplicates} introduced_duplicates={introduced_duplicates} requests_issued={} requests_needed={} elapsed_ms={} bytes_per_deal_180={} bytes_per_deal_600={} projected_total_mb={:.3} cap_mb={cap_mb} projection=matched_mean_times_all_deals_observed_bytes_only_not_unique_file_size station=local_copy_only",
        deals.len(),
        selected.len(),
        ambiguous,
        left.len(),
        issued.get(),
        needed.len(),
        started.elapsed().as_millis(),
        bytes_180 / selected.len(),
        bytes_600 / selected.len(),
        bytes_600 as f64 / selected.len() as f64 * deals.len() as f64 / 1_048_576.0
    );
}
