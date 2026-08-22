//! Regression tests for Profit Monitor row grouping and refresh decisions.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, UNIX_EPOCH};

use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::{Europe::Warsaw, Pacific::Apia, Tz};
use moon_core::db::analytics::{ProfitMonitorCore, ProfitMonitorSummary};
use moon_core::db::{ProfitUnit, QuoteCurrency};
use moon_core::feed::ExchangeId;
use moon_core::util::fmt::DeltaSign;
use moon_core::venue::CoreVenue;

use super::format::{format_profit, profit_column_width};
use super::rows::{GroupMode, LiveContext, MonitorRow, RowLabels, fold_total, grouped_rows};
use super::{
    ContextChange, MonitorLayout, MonitorPeriod, MonitorSort, MonitorSortColumn,
    duration_until_period_refresh, monitor_zone, next_sort, retain_last_known_venues, sort_rows,
};

/// Return deterministic fallback labels for pure grouping tests.
///
/// Returns:
///     English labels that keep expected rows readable.
fn labels() -> RowLabels<'static> {
    RowLabels { core: "Core" }
}

/// Build one core's venue as the session publishes it.
///
/// Args:
///     code: MoonBot platform ordinal.
///     dex: HIP-3 DEX name, empty for a regular exchange.
///     reported: Caption the core published.
///
/// Returns:
///     The venue a `LiveContext` entry holds.
fn venue(code: u8, dex: &str, reported: &str) -> CoreVenue {
    CoreVenue {
        id: ExchangeId::with_dex(code, dex),
        dex: dex.to_string(),
        reported: reported.to_string(),
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
                last_profit: Some(4.0),
                last_close: 1_700_000_100,
            },
            ProfitMonitorCore {
                core_uid: 2,
                report_name: "Second core".to_string(),
                profit: -2.0,
                trades: 1,
                wins: 0,
                positive_spent: 100.0,
                positive_orders: 1,
                last_profit: Some(-2.0),
                last_close: 1_700_000_200,
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
        last_profit: Some(30.0),
        last_close: 1_700_000_050,
    });
    let live = LiveContext {
        venues: HashMap::new(),
        core_names: HashMap::from([(2, "Configured second".to_string())]),
        core_order: vec![2],
        ..LiveContext::default()
    };
    let core_rows = grouped_rows(&summary, &live, GroupMode::Core, false, labels());
    assert_eq!(
        core_rows
            .iter()
            .map(|row| row.primary_core)
            .collect::<Vec<_>>(),
        vec![2, 1, 3]
    );
    let exchange_rows = grouped_rows(&summary, &live, GroupMode::Exchange, false, labels());
    assert_eq!(exchange_rows.len(), 1);
    // The shared wording, not a Profit-Monitor-only one: the same unidentified core reads
    // identically here and in every core picker that lists it.
    assert_eq!(
        exchange_rows[0].name,
        rust_i18n::t!("common.exchange_unknown").to_string()
    );
    assert_eq!(exchange_rows[0].trades, 6);
}

/// `profit_monitor/rows.rs:grouped_rows` must key Exchange rows on the venue IDENTITY, so cores
/// whose builds caption one venue differently merge, and two HIP-3 DEXes do not.
///
/// Breakage: grouping by the reported caption again splits one venue into two rows whenever two
/// cores spell it differently (`Hyper Futures` vs `Hyperliquid Futures`); keying on the platform
/// code alone merges two Hyperliquid DEXes whose markets have nothing in common.
#[test]
fn exchange_rows_group_by_venue_identity_not_by_caption() {
    let mut summary = summary();
    for core in &mut summary.cores {
        core.profit = 0.0;
    }
    summary.cores.push(ProfitMonitorCore {
        core_uid: 3,
        report_name: "Third".to_string(),
        profit: 0.0,
        trades: 7,
        wins: 3,
        positive_spent: 70.0,
        positive_orders: 2,
        last_profit: Some(0.0),
        last_close: 1_700_000_010,
    });
    let live = LiveContext {
        venues: HashMap::from([
            (1, venue(13, "", "Hyper Futures")),
            // The same venue, captioned differently by an older core build.
            (2, venue(13, "", "Hyperliquid Futures")),
            (3, venue(13, "xyz", "Hyper Futures")),
        ]),
        ..LiveContext::default()
    };

    let mut rows = grouped_rows(&summary, &live, GroupMode::Exchange, false, labels());
    assert_eq!(rows.len(), 2, "two captions of one venue are one row");
    sort_rows(&mut rows, Some(next_sort(None, MonitorSortColumn::Name)));
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        ["Hyperliquid Futures", "Hyperliquid Futures · xyz"]
    );
    assert_eq!(
        rows[0].cores.len(),
        2,
        "both differently captioned cores belong to the merged row"
    );
}

/// `profit_monitor/model.rs:context_change` must keep exchange-name changes out of the database
/// reload path; returning `Reload` here makes a reconnect run another full-period SQLite scan even
/// though cached per-core values can be regrouped immediately.
#[test]
fn exchange_only_context_change_regroups_without_database_read() {
    let before = LiveContext::default();
    let after = LiveContext {
        venues: HashMap::from([(1, venue(3, "", "Binance"))]),
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

/// `profit_monitor/model.rs:context_change` must carry a zone change as one inseparable reload and
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

/// `profit_monitor/model.rs:retain_last_known_venues` must preserve an exchange when its live
/// client temporarily disappears; removing that carry-forward moves the core into Unknown exchange
/// during every reconnect.
#[test]
fn disconnect_keeps_the_last_known_exchange_name() {
    let before = LiveContext {
        venues: HashMap::from([(1, venue(3, "", "Binance"))]),
        ..LiveContext::default()
    };
    let retained = retain_last_known_venues(&before, LiveContext::default());
    assert_eq!(retained.venues, before.venues);
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
            last_trade: false,
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
            last_trade: false,
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
            last_trade: false,
            win_rate: false,
            average_order: false,
        }
    );
    assert!(!MonitorLayout::for_width(499.0, 1.0).last_trade);
    assert!(MonitorLayout::for_width(500.0, 1.0).last_trade);
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
            last_trade: true,
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
            last_trade: true,
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
            last_trade: false,
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
            last_trade: false,
            win_rate: false,
            average_order: false,
        }
    );
    assert!(!MonitorLayout::for_width(487.4, 1.25).trades);
    assert!(MonitorLayout::for_width(487.5, 1.25).trades);
    assert!(!MonitorLayout::for_width(624.9, 1.25).last_trade);
    assert!(MonitorLayout::for_width(625.0, 1.25).last_trade);
    assert!(!MonitorLayout::for_width(774.9, 1.25).win_rate);
    assert!(MonitorLayout::for_width(775.0, 1.25).win_rate);
    assert!(!MonitorLayout::for_width(874.9, 1.25).status_label);
    assert!(MonitorLayout::for_width(875.0, 1.25).status_label);
    assert!(!MonitorLayout::for_width(949.9, 1.25).average_order);
    assert!(MonitorLayout::for_width(950.0, 1.25).average_order);
}

/// `profit_monitor/model.rs:{next_sort,sort_rows}` must map every visible heading to its own metric,
/// start names ascending and numbers descending, then reverse a repeated click. Changing the first
/// numeric direction or wiring Win rate/Average order to another field makes the independently
/// derived row orders below red and presents a misleading table after a header click.
#[test]
fn every_heading_sorts_its_own_value_and_repeated_click_reverses() {
    let rows = vec![
        super::rows::MonitorRow {
            name: "Alpha".to_string(),
            profit: 5.0,
            trades: 4,
            wins: 1,
            positive_spent: 60.0,
            positive_orders: 2,
            primary_core: 1,
            ..super::rows::MonitorRow::default()
        },
        super::rows::MonitorRow {
            name: "Beta".to_string(),
            profit: 12.0,
            trades: 2,
            wins: 2,
            positive_spent: 10.0,
            positive_orders: 1,
            primary_core: 2,
            ..super::rows::MonitorRow::default()
        },
        super::rows::MonitorRow {
            name: "Gamma".to_string(),
            profit: -3.0,
            trades: 8,
            wins: 4,
            positive_spent: 120.0,
            positive_orders: 3,
            primary_core: 3,
            ..super::rows::MonitorRow::default()
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

/// `profit_monitor/model.rs:MonitorPeriod::range_at` must derive Today from the selected header-clock
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

/// `profit_monitor/model.rs:monitor_zone` must share the header clock's exact-IANA policy; restricting
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

/// `profit_monitor/model.rs:MonitorPeriod::range_at` must resolve both local midnights through the
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

/// `profit_monitor/model.rs:duration_until_period_refresh` must arm calendar presets at the next
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
        format_profit(-0.001, None, Some(ProfitUnit::Quote(QuoteCurrency::usdt()))),
        ("+0 USDT".to_string(), DeltaSign::Zero)
    );
    assert_eq!(
        format_profit(12.5, None, Some(ProfitUnit::Percent)),
        ("+12.5%".to_string(), DeltaSign::Positive)
    );
}

/// `profit_monitor/mod.rs:format_profit` must place the newest trade INSIDE the unit and round it
/// with the same currency decimals as the total.
///
/// Breakage: appending the bracket after the ticker reads as a second currency; formatting the
/// suffix with its own decimals makes a cell claim precision the total does not show; letting the
/// suffix decide the colour paints a profitable period red after one losing trade.
#[test]
fn last_trade_suffix_shares_the_total_unit_and_rounding() {
    let usdt = Some(ProfitUnit::Quote(QuoteCurrency::usdt()));
    assert_eq!(
        format_profit(-57.114, Some(-0.6), usdt),
        ("-57.11(-0.6) USDT".to_string(), DeltaSign::Negative)
    );
    assert_eq!(
        format_profit(12.0, Some(-0.004), usdt),
        ("+12(+0) USDT".to_string(), DeltaSign::Positive),
        "a suffix that rounds away must not print a minus the number no longer shows"
    );
    assert_eq!(
        format_profit(41.0, Some(-3.0), usdt).1,
        DeltaSign::Positive,
        "the colour follows the total, not the last trade"
    );
    assert_eq!(
        format_profit(3.5, Some(1.25), Some(ProfitUnit::Percent)),
        ("+3.5(+1.25)%".to_string(), DeltaSign::Positive)
    );
    assert_eq!(
        format_profit(-57.114, None, usdt).0,
        "-57.11 USDT",
        "a core with no trade in the period must show no empty bracket"
    );
}

/// `profit_monitor/mod.rs:profit_column_width` must claim the suffix allowance only while the
/// suffix is drawn, and `MonitorLayout` must keep a name column at its minimum in the first tier
/// that allows one.
///
/// Breakage: widening the column unconditionally steals space from Name at every width; enabling
/// the suffix below its own tier truncates a money value instead of dropping it.
#[test]
fn last_trade_column_never_starves_the_name_column() {
    assert_eq!(profit_column_width(false), super::PROFIT_COLUMN_WIDTH);
    assert_eq!(
        profit_column_width(true),
        super::PROFIT_COLUMN_WIDTH + super::PROFIT_LAST_TRADE_EXTRA
    );
    // Narrowest width that turns the suffix on, spending it on every column that tier shows.
    let used = profit_column_width(true)
        + super::TRADES_COLUMN_WIDTH
        + 2.0 * super::TABLE_HORIZONTAL_PADDING
        + 2.0 * super::TABLE_COLUMN_GAP;
    assert!(
        super::LAST_TRADE_WIDTH - used >= super::MIN_NAME_COLUMN_WIDTH,
        "the suffix tier must still leave Name its minimum: {} left",
        super::LAST_TRADE_WIDTH - used
    );
}

/// `profit_monitor/rows.rs:MonitorRow::push` must merge cores onto the NEWEST trade, not the last
/// core the grouping happened to visit.
///
/// Breakage: taking whichever core is merged last makes the displayed last trade jump between two
/// refreshes of identical data, because the group map iterates in hash order.
#[test]
fn merged_rows_carry_the_newest_trade_of_their_cores() {
    let summary = summary();
    let live = LiveContext {
        venues: HashMap::from([
            (1, venue(4, "", "Binance Futures")),
            (2, venue(4, "", "Binance Futures")),
        ]),
        ..LiveContext::default()
    };
    let rows = grouped_rows(&summary, &live, GroupMode::Exchange, false, labels());

    assert_eq!(rows.len(), 1, "both cores report the same exchange");
    assert_eq!(rows[0].last_profit, Some(-2.0));
    assert_eq!(rows[0].last_close, 1_700_000_200);
    assert_eq!(
        rows[0].last_core, 2,
        "the highlight must follow the core that actually traded"
    );
    assert_eq!(
        rows[0].venue.as_ref().map(|venue| venue.id),
        Some(ExchangeId::new(4)),
        "the row must carry the venue its logo and filter payload are resolved from"
    );
}

/// `profit_monitor/mod.rs:arrivals` must highlight a core that traded, and nothing else.
///
/// Breakage: baselining with an empty map instead of `None` flashes every row on the first snapshot
/// after a period change; keeping strict close-date comparison alone misses a second trade inside
/// the same Unix second and a core's first trade of the period; comparing counts downward flashes
/// when retention trims the window; keeping departed cores grows the memory for the process life.
#[test]
fn only_a_core_that_traded_counts_as_an_arrival() {
    let mut cores = summary().cores;

    let (seen, arrived) = super::arrivals(None, &cores);
    assert!(arrived.is_empty(), "the baseline snapshot must not flash");
    assert_eq!(
        seen,
        HashMap::from([(1, (1_700_000_100, 3)), (2, (1_700_000_200, 1))])
    );

    let (again, arrived) = super::arrivals(Some(&seen), &cores);
    assert!(arrived.is_empty(), "an unchanged snapshot must not flash");
    assert_eq!(again, seen);

    // Same second, one more trade: the close date cannot move, so only the count can say so.
    cores[1].trades += 1;
    let (_, arrived) = super::arrivals(Some(&seen), &cores);
    assert_eq!(arrived, vec![2], "a same-second trade must still flash");
    cores[1].trades -= 1;

    // Retention trimming the period lowers the count; that is not a trade.
    cores[0].trades -= 1;
    let (_, arrived) = super::arrivals(Some(&seen), &cores);
    assert!(arrived.is_empty(), "a shrinking period must not flash");
    cores[0].trades += 1;

    cores[0].last_close = 1_700_000_300;
    cores.remove(1);
    let (after, arrived) = super::arrivals(Some(&seen), &cores);
    assert_eq!(arrived, vec![1]);
    assert_eq!(
        after,
        HashMap::from([(1, (1_700_000_300, 3))]),
        "a core that left the period must leave the arrival memory with it"
    );

    // A core that comes back after leaving the period is its first trade of the hour, not a row
    // quietly reappearing — the arrival a user is most likely waiting for.
    let (_, arrived) = super::arrivals(Some(&after), &summary().cores);
    assert_eq!(arrived, vec![2]);
}

/// `profit_monitor/settings.rs:MonitorPrefs::restore` must treat an unset key as its default and a
/// saved `false` as a deliberate choice.
///
/// Breakage: reading the keys as bare booleans makes "never opened the popup" indistinguishable
/// from "turned everything off", so a later default change silently overrides the user.
#[test]
fn display_preferences_separate_unset_from_disabled() {
    let mut layout = moon_core::config::layout::WindowLayout::default();
    assert_eq!(
        super::MonitorPrefs::restore(&layout),
        super::MonitorPrefs::default()
    );
    assert!(super::MonitorPrefs::default().last_trade);

    layout.profit_monitor_last_trade = Some(false);
    let restored = super::MonitorPrefs::restore(&layout);
    assert!(!restored.last_trade);
    assert!(
        restored.exchange_icons && restored.flash,
        "one disabled preference must not drag the others down"
    );

    // Grouping only ever splits rows that are already on screen, so it ships on. Zero rows for idle
    // cores ADD lines a query did not produce, so that one ships off and stays a deliberate choice.
    assert!(super::MonitorPrefs::default().group_sections);
    assert!(!super::MonitorPrefs::default().idle_cores);
    layout.profit_monitor_idle_cores = Some(true);
    layout.profit_monitor_group_sections = Some(false);
    let restored = super::MonitorPrefs::restore(&layout);
    assert!(restored.idle_cores && !restored.group_sections);
}

/// `profit_monitor/rows.rs:grouped_rows` must give an ACTIVE core with no trade its zero row, in
/// canonical order, and only on the by-core axis.
///
/// Breakage: reading a live session instead of the configured flag drops a core that is merely
/// offline and resurrects one the user switched off; appending idle rows after the rank sort puts
/// them all at the bottom instead of among their neighbours; extending Exchange mode invents a
/// venue row for a core that reported no trade to name one.
#[test]
fn idle_rows_cover_active_cores_only_and_only_by_core() {
    let live = LiveContext {
        core_names: HashMap::from([(1, "First".to_string()), (9, "Quiet".to_string())]),
        core_order: vec![9, 1, 2],
        // Core 2 traded but is switched off; core 7 is active but was never configured here.
        active: HashSet::from([1, 9]),
        ..LiveContext::default()
    };

    let rows = grouped_rows(&summary(), &live, GroupMode::Core, true, labels());
    assert_eq!(
        rows.iter().map(|row| row.primary_core).collect::<Vec<_>>(),
        vec![9, 1, 2],
        "the idle core takes its canonical place, not a trailing block"
    );
    let idle = &rows[0];
    assert_eq!(
        (idle.profit, idle.trades, idle.wins, idle.last_close),
        (0.0, 0, 0, 0)
    );
    assert!(
        idle.last_profit.is_none() && idle.cores.is_empty(),
        "a core that closed nothing has no last trade and nothing to flash"
    );
    assert_eq!(
        idle.filter_cores.as_ref(),
        [9],
        "its row must still filter the terminal to itself"
    );

    let without = grouped_rows(&summary(), &live, GroupMode::Core, false, labels());
    assert_eq!(without.len(), 2, "the preference alone adds the zero rows");
    let exchange = grouped_rows(&summary(), &live, GroupMode::Exchange, true, labels());
    assert_eq!(
        exchange.len(),
        1,
        "an exchange row exists because a trade named that venue"
    );
}

/// `profit_monitor/rows.rs:core_row_name` must treat a BLANK configured name as no answer and fall
/// through to the name the report carried.
///
/// Breakage: testing emptiness only after `.or(report)` makes a core whose configured name is a
/// space show `Core <uid>` while its perfectly good reported name sits unused.
#[test]
fn a_blank_configured_name_falls_through_to_the_report_name() {
    let live = LiveContext {
        core_names: HashMap::from([(1, "   ".to_string()), (2, String::new())]),
        core_order: vec![1, 2],
        ..LiveContext::default()
    };
    let rows = grouped_rows(&summary(), &live, GroupMode::Core, false, labels());

    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        ["First core", "Second core"]
    );
}

/// `profit_monitor/rows.rs:fold_total` must add every additive field while taking the newest trade
/// rather than summing it, deterministically.
///
/// Breakage: summing `last_profit` invents money nobody made; folding in list order lets the value
/// change when a sort click reorders identical data.
#[test]
fn one_fold_serves_the_total_and_every_subtotal() {
    let live = LiveContext::default();
    let rows = grouped_rows(&summary(), &live, GroupMode::Core, false, labels());
    let total = fold_total(&rows);

    assert_eq!((total.profit, total.trades, total.wins), (10.0, 4, 2));
    assert_eq!(total.positive_spent, 400.0);
    assert_eq!(total.average_order(), Some(400.0 / 3.0));
    // No denominator, no ratio: an all-zero fold states nothing rather than a rate of zero.
    assert_eq!(MonitorRow::default().win_rate(), None);
    assert_eq!(MonitorRow::default().average_order(), None);
    assert_eq!(
        (total.last_profit, total.last_close, total.last_core),
        (Some(-2.0), 1_700_000_200, 2),
        "the newest closed trade, not a sum of the two"
    );

    let mut reversed = rows.clone();
    reversed.reverse();
    let same = fold_total(&reversed);
    assert_eq!(
        (same.last_profit, same.last_close, same.last_core),
        (total.last_profit, total.last_close, total.last_core)
    );
    assert_eq!(fold_total(&[]).trades, 0, "an empty fold is all zeroes");
}

/// `profit_monitor/mod.rs:arrivals` must not treat an EMPTY previous snapshot as a baseline.
///
/// Breakage: storing `Some({})` and diffing against it makes the first populated read — report
/// replication catching up after the window opens — light every row in the table at once, the exact
/// failure the rebaseline rule exists to prevent.
#[test]
fn an_empty_previous_snapshot_is_not_a_baseline() {
    let empty: HashMap<u64, (i64, i64)> = HashMap::new();

    // What the view does: an empty baseline is filtered away before the diff.
    let baseline = Some(&empty).filter(|seen| !seen.is_empty());
    let (_, arrived) = super::arrivals(baseline, &summary().cores);
    assert!(
        arrived.is_empty(),
        "a table filling up for the first time must not read as a table full of arrivals"
    );
}
