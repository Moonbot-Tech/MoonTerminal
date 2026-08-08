//! Regression tests for Profit Monitor row grouping and refresh decisions.

use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::{Europe::Warsaw, Pacific::Apia, Tz};
use moon_core::db::analytics::{ProfitMonitorCore, ProfitMonitorSummary};
use moon_core::db::{ProfitUnit, QuoteCurrency};
use moon_core::util::fmt::DeltaSign;

use super::rows::{GroupMode, LiveContext, RowLabels, grouped_rows};
use super::{
    ContextChange, MonitorLayout, MonitorPeriod, MonitorSort, MonitorSortColumn,
    duration_until_period_refresh, format_profit, monitor_zone, next_sort,
    retain_last_known_exchange_names, sort_rows,
};

/// Return deterministic fallback labels for pure grouping tests.
///
/// Returns:
///     English labels that keep expected rows readable.
fn labels() -> RowLabels<'static> {
    RowLabels {
        core: "Core",
        unknown_exchange: "Unknown exchange",
        spot: "Spot",
    }
}

/// Build two report-core rows whose additive totals are easy to calculate independently.
///
/// Returns:
///     A two-core comparable payload without any unit-specific behavior.
fn summary() -> ProfitMonitorSummary {
    ProfitMonitorSummary {
        cores: vec![
            ProfitMonitorCore {
                core_uid: 1,
                report_name: "First core".to_string(),
                profit: 12.0,
                trades: 3,
                wins: 2,
                positive_spent: 300.0,
                positive_orders: 2,
            },
            ProfitMonitorCore {
                core_uid: 2,
                report_name: "Second core".to_string(),
                profit: -2.0,
                trades: 1,
                wins: 0,
                positive_spent: 100.0,
                positive_orders: 1,
            },
        ],
    }
}

/// `profit_monitor/rows.rs:grouped_rows` must use `core_order` in Core mode and keep historical
/// unknown cores in ascending report order; replacing the rank sort with profit sorting or reversing
/// its UID fallback makes this assertion red and causes rows to jump between refreshes.
#[test]
fn core_mode_preserves_canonical_order_and_unknown_exchange_is_explicit() {
    let mut summary = summary();
    summary.cores.push(ProfitMonitorCore {
        core_uid: 3,
        report_name: "Historical third".to_string(),
        profit: 50.0,
        trades: 2,
        wins: 2,
        positive_spent: 75.0,
        positive_orders: 1,
    });
    let live = LiveContext {
        exchange_names: HashMap::new(),
        core_names: HashMap::from([(2, "Configured second".to_string())]),
        core_order: vec![2],
    };
    let core_rows = grouped_rows(&summary, &live, GroupMode::Core, labels());
    assert_eq!(
        core_rows
            .iter()
            .map(|row| row.primary_core)
            .collect::<Vec<_>>(),
        vec![2, 1, 3]
    );
    let exchange_rows = grouped_rows(&summary, &live, GroupMode::Exchange, labels());
    assert_eq!(exchange_rows.len(), 1);
    assert_eq!(exchange_rows[0].name, "Unknown exchange");
    assert_eq!(exchange_rows[0].trades, 6);
}

/// profit_monitor/rows.rs:grouped_rows must format only the visible exchange label while
/// retaining raw identities for grouping and sorting; replacing sort_name with the display name
/// reorders this equal-profit set, while grouping by display merges Hyper with Hyper Spot.
#[test]
fn exchange_spot_label_preserves_raw_grouping_and_sorting() {
    let mut summary = summary();
    for core in &mut summary.cores {
        core.profit = 0.0;
    }
    summary.cores.push(ProfitMonitorCore {
        core_uid: 3,
        report_name: "Explicit spot".to_string(),
        profit: 0.0,
        trades: 7,
        wins: 3,
        positive_spent: 70.0,
        positive_orders: 2,
    });
    let live = LiveContext {
        exchange_names: HashMap::from([
            (1, "Hyper".to_string()),
            (2, "Hyper Futures".to_string()),
            (3, "Hyper Spot".to_string()),
        ]),
        ..LiveContext::default()
    };

    let mut rows = grouped_rows(&summary, &live, GroupMode::Exchange, labels());
    assert_eq!(
        rows.len(),
        3,
        "equal display labels must not merge raw groups"
    );
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        ["Hyper Spot", "Hyper Futures", "Hyper Spot"]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.sort_name.as_str())
            .collect::<Vec<_>>(),
        ["Hyper", "Hyper Futures", "Hyper Spot"]
    );

    sort_rows(&mut rows, Some(next_sort(None, MonitorSortColumn::Name)));
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        ["Hyper Spot", "Hyper Futures", "Hyper Spot"]
    );
}

/// `profit_monitor/mod.rs:context_change` must keep exchange-name changes out of the database
/// reload path; returning `Reload` here makes a reconnect run another full-period SQLite scan even
/// though cached per-core values can be regrouped immediately.
#[test]
fn exchange_only_context_change_regroups_without_database_read() {
    let before = LiveContext::default();
    let after = LiveContext {
        exchange_names: HashMap::from([(1, "Binance".to_string())]),
        ..LiveContext::default()
    };
    assert_eq!(
        super::context_change(&before, &after, false, false),
        ContextChange::Regroup
    );
    assert_eq!(
        super::context_change(&after, &after, true, false),
        ContextChange::Reload {
            restart_clock: false
        }
    );
}

/// `profit_monitor/mod.rs:context_change` must carry a zone change as one inseparable reload and
/// clock-restart plan; treating it as a plain valuation reload leaves the old midnight timer armed
/// after the user changes the header city.
#[test]
fn zone_change_reloads_and_restarts_the_calendar_timer() {
    let context = LiveContext::default();
    assert_eq!(
        super::context_change(&context, &context, false, true),
        ContextChange::Reload {
            restart_clock: true
        }
    );
}

/// `profit_monitor/mod.rs:retain_last_known_exchange_names` must preserve an exchange when its live
/// client temporarily disappears; removing that carry-forward moves the core into Unknown exchange
/// during every reconnect.
#[test]
fn disconnect_keeps_the_last_known_exchange_name() {
    let before = LiveContext {
        exchange_names: HashMap::from([(1, "Binance".to_string())]),
        ..LiveContext::default()
    };
    let retained = retain_last_known_exchange_names(&before, LiveContext::default());
    assert_eq!(retained.exchange_names, before.exchange_names);
}

/// `profit_monitor/rows.rs:GroupMode` must reject the retired `bot` id and default to Core;
/// accepting it again or moving the default to Exchange makes this assertion red and restores a
/// removed button through an old `layout.toml` preference.
#[test]
fn retired_bot_preference_falls_back_to_core() {
    assert_eq!(GroupMode::from_id("bot"), None);
    assert_eq!(GroupMode::default(), GroupMode::Core);
}

/// `profit_monitor/mod.rs:MIN_WINDOW_WIDTH` must remain the independently derived Name-plus-Profit
/// budget, and `MonitorLayout::for_width` must preserve every exact degradation boundary. Restoring
/// the old 390px minimum, keeping Trades always visible, removing a scale multiplier, or shifting a
/// threshold makes the budget or one adjacent pair red and blocks or clips the narrow window.
#[test]
fn responsive_layout_degrades_at_the_documented_boundaries() {
    assert_eq!(
        super::MIN_WINDOW_WIDTH,
        super::MIN_NAME_COLUMN_WIDTH
            + super::PROFIT_COLUMN_WIDTH
            + 2.0 * super::TABLE_HORIZONTAL_PADDING
            + super::TABLE_COLUMN_GAP,
        "the OS minimum must fit Name, Profit, their gap, and both side paddings"
    );
    assert_eq!(
        MonitorLayout::for_width(super::MIN_WINDOW_WIDTH, 1.0),
        MonitorLayout {
            inline_controls: false,
            clock_seconds: false,
            status_label: false,
            trades: false,
            win_rate: false,
            average_order: false,
        },
        "the minimum window width must exercise the tier below Trades instead of restoring 390px"
    );
    assert!(!MonitorLayout::for_width(389.0, 1.0).trades);
    assert!(MonitorLayout::for_width(390.0, 1.0).trades);
    assert_eq!(
        MonitorLayout::for_width(459.0, 1.0),
        MonitorLayout {
            inline_controls: false,
            clock_seconds: false,
            status_label: false,
            trades: true,
            win_rate: false,
            average_order: false,
        }
    );
    assert_eq!(
        MonitorLayout::for_width(460.0, 1.0),
        MonitorLayout {
            inline_controls: true,
            clock_seconds: true,
            status_label: false,
            trades: true,
            win_rate: false,
            average_order: false,
        }
    );
    assert!(!MonitorLayout::for_width(619.0, 1.0).win_rate);
    assert!(MonitorLayout::for_width(620.0, 1.0).win_rate);
    assert!(!MonitorLayout::for_width(699.0, 1.0).status_label);
    assert!(MonitorLayout::for_width(700.0, 1.0).status_label);
    assert_eq!(
        MonitorLayout::for_width(759.0, 1.0),
        MonitorLayout {
            inline_controls: true,
            clock_seconds: true,
            status_label: true,
            trades: true,
            win_rate: true,
            average_order: false,
        }
    );
    assert_eq!(
        MonitorLayout::for_width(760.0, 1.0),
        MonitorLayout {
            inline_controls: true,
            clock_seconds: true,
            status_label: true,
            trades: true,
            win_rate: true,
            average_order: true,
        }
    );
    assert_eq!(
        MonitorLayout::for_width(574.9, 1.25),
        MonitorLayout {
            inline_controls: false,
            clock_seconds: false,
            status_label: false,
            trades: true,
            win_rate: false,
            average_order: false,
        }
    );
    assert_eq!(
        MonitorLayout::for_width(575.0, 1.25),
        MonitorLayout {
            inline_controls: true,
            clock_seconds: true,
            status_label: false,
            trades: true,
            win_rate: false,
            average_order: false,
        }
    );
    assert!(!MonitorLayout::for_width(487.4, 1.25).trades);
    assert!(MonitorLayout::for_width(487.5, 1.25).trades);
    assert!(!MonitorLayout::for_width(774.9, 1.25).win_rate);
    assert!(MonitorLayout::for_width(775.0, 1.25).win_rate);
    assert!(!MonitorLayout::for_width(874.9, 1.25).status_label);
    assert!(MonitorLayout::for_width(875.0, 1.25).status_label);
    assert!(!MonitorLayout::for_width(949.9, 1.25).average_order);
    assert!(MonitorLayout::for_width(950.0, 1.25).average_order);
}

/// `profit_monitor/mod.rs:{next_sort,sort_rows}` must map every visible heading to its own metric,
/// start names ascending and numbers descending, then reverse a repeated click. Changing the first
/// numeric direction or wiring Win rate/Average order to another field makes the independently
/// derived row orders below red and presents a misleading table after a header click.
#[test]
fn every_heading_sorts_its_own_value_and_repeated_click_reverses() {
    let rows = vec![
        super::rows::MonitorRow {
            name: "Alpha".to_string(),
            sort_name: "Alpha".to_string(),
            profit: 5.0,
            trades: 4,
            wins: 1,
            positive_spent: 60.0,
            positive_orders: 2,
            primary_core: 1,
        },
        super::rows::MonitorRow {
            name: "Beta".to_string(),
            sort_name: "Beta".to_string(),
            profit: 12.0,
            trades: 2,
            wins: 2,
            positive_spent: 10.0,
            positive_orders: 1,
            primary_core: 2,
        },
        super::rows::MonitorRow {
            name: "Gamma".to_string(),
            sort_name: "Gamma".to_string(),
            profit: -3.0,
            trades: 8,
            wins: 4,
            positive_spent: 120.0,
            positive_orders: 3,
            primary_core: 3,
        },
    ];
    let names_for = |column| {
        let mut ordered = rows.clone();
        sort_rows(&mut ordered, Some(next_sort(None, column)));
        ordered.into_iter().map(|row| row.name).collect::<Vec<_>>()
    };

    assert_eq!(
        names_for(MonitorSortColumn::Name),
        ["Alpha", "Beta", "Gamma"]
    );
    assert_eq!(
        names_for(MonitorSortColumn::Profit),
        ["Beta", "Alpha", "Gamma"]
    );
    assert_eq!(
        names_for(MonitorSortColumn::Trades),
        ["Gamma", "Alpha", "Beta"]
    );
    assert_eq!(
        names_for(MonitorSortColumn::WinRate),
        ["Beta", "Gamma", "Alpha"]
    );
    assert_eq!(
        names_for(MonitorSortColumn::AverageOrder),
        ["Gamma", "Alpha", "Beta"]
    );

    let descending = next_sort(None, MonitorSortColumn::Trades);
    assert_eq!(
        descending,
        MonitorSort {
            column: MonitorSortColumn::Trades,
            descending: true,
        }
    );
    assert_eq!(
        next_sort(Some(descending), MonitorSortColumn::Trades),
        MonitorSort {
            column: MonitorSortColumn::Trades,
            descending: false,
        }
    );
}

/// `profit_monitor/mod.rs:MonitorPeriod::range_at` must derive Today from the selected header-clock
/// zone; replacing `display_time::day_start` with UTC excludes a HyperLiquid close at 01:01
/// Warsaw from the user's Today report.
#[test]
fn today_includes_the_early_warsaw_hour() {
    let now = Utc.with_ymd_and_hms(2026, 8, 5, 18, 16, 57).unwrap();
    let hyperliquid_close = Utc.with_ymd_and_hms(2026, 8, 4, 23, 1, 18).unwrap();
    let (from, to) = MonitorPeriod::Today.range_at(now, Warsaw);

    assert_eq!((from, to), (1_785_880_800, 1_785_967_200));
    assert!(from <= hyperliquid_close.timestamp() && hyperliquid_close.timestamp() < to);
    assert_ne!(
        (from, to),
        MonitorPeriod::Today.range_at(now, Tz::UTC),
        "the selected city must change the calendar query bounds"
    );
}

/// `profit_monitor/mod.rs:monitor_zone` must share the header clock's exact-IANA policy; restricting
/// it to the curated city table makes this assertion red and sends a Detroit system profile back
/// to UTC calendar bounds after restart.
#[test]
fn monitor_zone_matches_the_visible_header_exact_iana_policy() {
    assert_eq!(monitor_zone(Some("Europe/Warsaw")), Warsaw);
    assert_eq!(monitor_zone(Some("America/Detroit")), Tz::America__Detroit);
    assert_eq!(monitor_zone(None), Tz::UTC);
}

/// `display_time::day_start` must advance across a fully skipped civil date; restoring a short gap
/// probe makes this assertion red and can drop a historical monitor day at a dateline jump.
#[test]
fn skipped_civil_date_advances_to_its_first_real_instant() {
    let skipped = NaiveDate::from_ymd_opt(2011, 12, 30).unwrap();
    assert_eq!(
        moon_core::util::display_time::day_start(skipped, Apia),
        Some(1_325_239_200)
    );
}

/// Replacing `MonitorPeriod::range_at`'s existing-day step with forward-clamping `day_start`
/// makes Apia Yesterday empty on December 31 instead of selecting December 29.
#[test]
fn yesterday_uses_the_previous_existing_day_across_a_dateline_skip() {
    let now = Utc.with_ymd_and_hms(2011, 12, 30, 12, 0, 0).unwrap();

    assert_eq!(
        MonitorPeriod::Yesterday.range_at(now, Apia),
        (1_325_152_800, 1_325_239_200)
    );
}

/// `profit_monitor/mod.rs:MonitorPeriod::range_at` must resolve both local midnights through the
/// IANA zone; adding a fixed 86,400-second day makes these independent transition lengths red and
/// shifts one edge of Profit Monitor Today around Warsaw daylight-saving changes.
#[test]
fn warsaw_calendar_days_follow_both_dst_transitions() {
    let spring = Utc.with_ymd_and_hms(2026, 3, 29, 12, 0, 0).unwrap();
    let autumn = Utc.with_ymd_and_hms(2026, 10, 25, 12, 0, 0).unwrap();
    let (spring_from, spring_to) = MonitorPeriod::Today.range_at(spring, Warsaw);
    let (autumn_from, autumn_to) = MonitorPeriod::Today.range_at(autumn, Warsaw);

    assert_eq!(spring_to - spring_from, 23 * 60 * 60);
    assert_eq!(autumn_to - autumn_from, 25 * 60 * 60);
}

/// `profit_monitor/mod.rs:duration_until_period_refresh` must arm calendar presets at the next
/// selected-city midnight; restoring the UTC-day timer makes the Warsaw wait red and leaves Today
/// stale for two hours after its query boundary moves in summer.
#[test]
fn clock_refresh_uses_minute_local_midnight_or_no_timer_by_period() {
    assert_eq!(
        duration_until_period_refresh(
            MonitorPeriod::Hour,
            Warsaw,
            UNIX_EPOCH + Duration::from_millis(123_456)
        ),
        Some(Duration::from_millis(56_544))
    );
    assert_eq!(
        duration_until_period_refresh(
            MonitorPeriod::Year,
            Warsaw,
            UNIX_EPOCH + Duration::from_secs(1_785_953_817)
        ),
        Some(Duration::from_secs(13_383)),
        "calendar presets must wait for Warsaw midnight instead of UTC midnight"
    );
    assert_eq!(
        duration_until_period_refresh(MonitorPeriod::All, Warsaw, UNIX_EPOCH),
        None
    );
}

/// `profit_monitor/mod.rs:format_profit` must preserve the exact unit suffix and return the sign
/// from the same unit-aware rounding that produced the visible amount. Reclassifying the raw value
/// makes the first assertion a red `+0 USDT` loss, while dropping either suffix makes the operator
/// compare amounts without knowing whether they are money or percent.
#[test]
fn formatted_profit_keeps_unit_and_rounded_sign_coupled() {
    assert_eq!(
        format_profit(-0.001, Some(ProfitUnit::Quote(QuoteCurrency::usdt()))),
        ("+0 USDT".to_string(), DeltaSign::Zero)
    );
    assert_eq!(
        format_profit(12.5, Some(ProfitUnit::Percent)),
        ("+12.5%".to_string(), DeltaSign::Positive)
    );
}
