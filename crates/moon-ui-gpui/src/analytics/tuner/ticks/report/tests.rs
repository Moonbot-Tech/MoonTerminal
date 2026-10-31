//! Diagnostic identities must not collapse unlisted exchanges into an unknown marker.

use super::{TapeStatus, TicksData, report_row, venue_name};
use moon_core::db::tuner::ticks::diag::{self, ReportInput};
use moon_core::db::tuner::ticks::{Deal, Deltas};

/// Losing the exchange key in diagnostics would make unrelated core venues indistinguishable.
#[test]
fn unlisted_report_uses_the_exchange_code() {
    assert_eq!(
        venue_name(None, moon_core::fixture::unlisted::EXCHANGE_KEY),
        "#200"
    );
    assert_eq!(venue_name(None, "201:00000001"), "#201");
    assert_eq!(venue_name(None, "broken:key"), "broken:key");
    assert_eq!(
        venue_name(moon_core::venue::venue(4), "4:00000000"),
        "Binance Futures"
    );
}

/// Build one anonymous diagnostic row without any account or production data.
fn synthetic_row(uid: i64) -> super::DealRow {
    super::DealRow {
        deal: Deal {
            report_uid: uid,
            core_uid: uid as u64 % 10,
            core_name: "CORE_A".into(),
            strategy_id: 1,
            kind: "MoonShot".into(),
            coin: "XALPHA".into(),
            buy_ms: 1_000,
            close_ms: 2_000,
            buy_price: 100.0,
            sell_price: 101.0,
            spent: 100.0,
            is_short: false,
            sell_reason: String::new(),
            fact_pnl: 1.0,
            sizing: None,
            pnl_pct: false,
            profit: Some(1.0),
            deltas: Deltas::default(),
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
            quotes: None,
            bars: None,
            own_entry: None,
            buy_set_ms: None,
            corridor: None,
            entry_placed: None,
            gap: None,
        },
        tape: TapeStatus::Missing,
        verdict: None,
        address: None,
        ticks: None,
        entry_line: None,
        held: None,
    }
}

/// Measure the report construction and synchronous write formerly paid by the UI click.
#[test]
#[ignore = "scale probe: run by hand with --ignored --nocapture"]
fn scale_probe_diagnostic_report_100k() {
    let data = TicksData {
        rows: (0..100_000).map(synthetic_row).collect::<Vec<_>>().into(),
        ..Default::default()
    };
    let mut samples = Vec::new();
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let input = ReportInput {
            build: "synthetic".into(),
            period_days: Some(1.0),
            without_ms: data.without_ms,
            service: data.service,
            untunable: data.untunable,
            rows: data.rows.iter().map(|row| report_row(row, &data)).collect(),
            cores: Default::default(),
            model: Default::default(),
        };
        let text = diag::render(&input);
        std::hint::black_box(text.len());
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[probe] diagnostic_report_build=100000 median={:.3}ms",
        samples[2]
    );
}
