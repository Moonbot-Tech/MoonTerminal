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

use super::format::{
    ColumnFloor, ColumnMetrics, ProfitFloor, ProfitForm, ProfitLen, format_profit,
    plan_profit_column,
};
use super::model::scoped_query_core_ids;
use super::rows::{GroupMode, LiveContext, MonitorRow, RowLabels, fold_total, grouped_rows};
use super::{
    ContextChange, MonitorLayout, MonitorPeriod, MonitorSort, MonitorSortColumn,
    duration_until_period_refresh, monitor_zone, next_sort, retain_last_known_venues, sort_rows,
};

/// Every part printed: the form a column takes whenever the room allows it.
const FULL: ProfitForm = ProfitForm {
    suffix: true,
    ticker: true,
    si: false,
};

/// Column metrics whose glyph advances are exactly one unit, so a width reads as a character count.
///
/// Args:
///     available: Room the column may take.
///     ticker: Length of the currency ticker.
///
/// Returns:
///     Metrics carrying the real Russian headings measured in characters.
fn metrics(available: f32, ticker: usize) -> ColumnMetrics {
    metrics_at(available, ticker)
}

/// Same metrics under a second name, so a test can state that only the room changed.
///
/// Args:
///     available: Room the column may take.
///     ticker: Length of the currency ticker.
///
/// Returns:
///     Metrics carrying the real Russian headings measured in characters.
fn metrics_at(available: f32, ticker: usize) -> ColumnMetrics {
    ColumnMetrics {
        row_char: 1.0,
        total_char: 1.0,
        // "Прибыль" and "Прибыль, USDT", each plus the sort arrow the heading budget reserves.
        heading: 9.0,
        heading_with_unit: 15.0,
        ticker,
        available,
    }
}

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

/// `profit_monitor/mod.rs:MIN_WINDOW_WIDTH` must keep fitting Name beside a profit column wide
/// enough to print an ordinary amount, and `MonitorLayout::for_width` must preserve every exact
/// degradation boundary. Restoring the old 390px minimum, keeping Trades always visible, removing a
/// scale multiplier, or shifting a threshold makes the budget or one adjacent pair red and blocks
/// or clips the narrow window.
#[test]
fn responsive_layout_degrades_at_the_documented_boundaries() {
    assert!(
        super::MIN_WINDOW_WIDTH
            >= super::MIN_NAME_COLUMN_WIDTH
                + super::PROFIT_MIN_COLUMN_WIDTH
                + 2.0 * super::TABLE_HORIZONTAL_PADDING
                + super::TABLE_COLUMN_GAP,
        "the OS minimum must fit Name, a printable Profit, their gap, and both side paddings"
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

/// `profit_monitor/model.rs:MonitorPeriod::id` must keep the three rolling preset ids that are
/// already stored in `layout.toml`; renaming `m-week` to `m-days-7` makes this assertion red and
/// silently resets users who selected the seven-day view to Today after restart.
#[test]
fn legacy_rolling_period_ids_restore_the_renamed_presets() {
    assert_eq!(
        [
            MonitorPeriod::from_id("m-week"),
            MonitorPeriod::from_id("m-month"),
            MonitorPeriod::from_id("m-year"),
        ],
        [
            Some(MonitorPeriod::Days7),
            Some(MonitorPeriod::Days30),
            Some(MonitorPeriod::Days365),
        ],
        "persisted rolling-period ids must survive the variant rename"
    );
}

/// `profit_monitor/model.rs:MonitorPeriod::range_at` must start CurWeek at Monday on Monday and
/// mid-week; using `num_days_from_sunday()` makes these assertions red and shows users data from
/// the wrong calendar week.
#[test]
fn cur_week_is_monday_anchored_on_and_after_monday() {
    for (now, expected_to) in [
        (
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            1_785_794_400,
        ),
        (
            Utc.with_ymd_and_hms(2026, 8, 5, 18, 16, 57).unwrap(),
            1_785_967_200,
        ),
    ] {
        assert_eq!(
            MonitorPeriod::CurWeek.range_at(now, Warsaw),
            (1_785_708_000, expected_to),
            "CurWeek must begin at the independently pinned Monday boundary"
        );
    }
}

/// `profit_monitor/model.rs:MonitorPeriod::range_at` must start CurYear at January 1 in the
/// selected zone; starting from today's month makes this assertion red and omits earlier annual
/// profit data from the monitor.
#[test]
fn cur_year_is_anchored_to_january_first_in_the_selected_zone() {
    let now = Utc.with_ymd_and_hms(2026, 8, 5, 18, 16, 57).unwrap();

    assert_eq!(
        MonitorPeriod::CurYear.range_at(now, Warsaw),
        (1_767_222_000, 1_785_967_200),
        "CurYear must include every local calendar day from January 1 through today"
    );
}

/// `analytics/profit_monitor/model.rs:MonitorPeriod::range_at` must resolve Last month as the
/// complete previous calendar month; a January carry copied from the non-January arm silently
/// makes the Profit Monitor show January instead of the prior December.
///
/// Independent oracle: expected boundaries are explicit UTC midnights for normal, year-crossing,
/// unequal-month-length, leap-February, and ordinary-February cases rather than values from the
/// production range resolver.
#[test]
fn last_month_range_uses_previous_calendar_month_boundaries() {
    let midnight = |year, month, day| {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("valid UTC midnight")
    };

    for (now, expected, why) in [
        (
            midnight(2024, 6, 15),
            (
                midnight(2024, 5, 1).timestamp(),
                midnight(2024, 6, 1).timestamp(),
            ),
            "mid-year month",
        ),
        (
            midnight(2024, 1, 15),
            (
                midnight(2023, 12, 1).timestamp(),
                midnight(2024, 1, 1).timestamp(),
            ),
            "January rolls into the prior year",
        ),
        (
            midnight(2024, 9, 15),
            (
                midnight(2024, 8, 1).timestamp(),
                midnight(2024, 9, 1).timestamp(),
            ),
            "31-day August precedes a 30-day current month",
        ),
        (
            midnight(2024, 3, 15),
            (
                midnight(2024, 2, 1).timestamp(),
                midnight(2024, 3, 1).timestamp(),
            ),
            "leap-year February ends on the following month start",
        ),
        (
            midnight(2023, 3, 15),
            (
                midnight(2023, 2, 1).timestamp(),
                midnight(2023, 3, 1).timestamp(),
            ),
            "ordinary February has the same month-start boundary shape",
        ),
    ] {
        assert_eq!(
            MonitorPeriod::LastMonth.range_at(now, Tz::UTC),
            expected,
            "{why}"
        );
    }
}

/// `analytics/profit_monitor/model.rs:MonitorPeriod::from_id` must restore the Last month id
/// from the monitor layout; renaming it resets users to Today on the next open.
#[test]
fn last_month_persisted_id_round_trips_to_its_preset() {
    assert_eq!(
        MonitorPeriod::from_id("m-last-month"),
        Some(MonitorPeriod::LastMonth)
    );
}

/// `profit_monitor/model.rs:MonitorPeriod::GROUPS` must remain the complete ordered partition of
/// `ALL`; adding a preset to only one constant makes this assertion red and leaves a restorable
/// period unreachable from the picker or a picker item outside the persisted set.
#[test]
fn period_groups_match_all_presets_exactly_once_in_display_order() {
    let grouped = MonitorPeriod::GROUPS
        .iter()
        .flat_map(|group| group.iter().copied())
        .collect::<Vec<_>>();

    assert_eq!(
        grouped,
        MonitorPeriod::ALL,
        "GROUPS must contain every persisted preset exactly once and in display order"
    );
}

/// `profit_monitor/model.rs:duration_until_period_refresh` must arm every bounded preset at the
/// next selected-city midnight; restoring the UTC-day timer makes the Warsaw wait red and leaves
/// a rolling range stale for two hours after its query boundary moves in summer.
#[test]
fn clock_refresh_uses_local_midnight_or_no_timer_by_period() {
    assert_eq!(
        duration_until_period_refresh(
            MonitorPeriod::Days365,
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
/// makes the first assertion a red `0.00 USDT` loss, while dropping either suffix makes the operator
/// compare amounts without knowing whether they are money or percent.
#[test]
fn formatted_profit_keeps_unit_and_rounded_sign_coupled() {
    assert_eq!(
        format_profit(
            -0.001,
            None,
            Some(ProfitUnit::Quote(QuoteCurrency::usdt())),
            FULL
        ),
        ("0.00 USDT".to_string(), DeltaSign::Zero)
    );
    assert_eq!(
        format_profit(12.5, None, Some(ProfitUnit::Percent), FULL),
        ("+12.50%".to_string(), DeltaSign::Positive)
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
        format_profit(-57.114, Some(-0.6), usdt, FULL),
        ("-57.11(-0.60) USDT".to_string(), DeltaSign::Negative)
    );
    assert_eq!(
        format_profit(12.0, Some(-0.004), usdt, FULL),
        ("+12.00(0.00) USDT".to_string(), DeltaSign::Positive),
        "a suffix that rounds away must not print a minus the number no longer shows"
    );
    assert_eq!(
        format_profit(41.0, Some(-3.0), usdt, FULL).1,
        DeltaSign::Positive,
        "the colour follows the total, not the last trade"
    );
    assert_eq!(
        format_profit(3.5, Some(1.25), Some(ProfitUnit::Percent), FULL),
        ("+3.50(+1.25)%".to_string(), DeltaSign::Positive)
    );
    assert_eq!(
        format_profit(-57.114, None, usdt, FULL).0,
        "-57.11 USDT",
        "a core with no trade in the period must show no empty bracket"
    );
}

/// `profit_monitor/format.rs:plan_profit_column` must spend only what the values need, and give
/// everything above that back to the name column.
///
/// Breakage: sizing the column for a worst case nobody is showing — the fixed 154 units this
/// replaced — truncates a name on every row to reserve digits that are never drawn.
#[test]
fn the_profit_column_is_sized_from_the_values_it_shows() {
    let usdt = Some(ProfitUnit::Quote(QuoteCurrency::usdt()));
    // "+167.21" and "(+3.40)": nineteen characters once " USDT" is added.
    let row = ProfitLen::measure(167.21, Some(3.4), usdt);
    let column = plan_profit_column(row, row, true, &metrics(200.0, 4), ProfitFloor::default());
    assert_eq!(column.form, FULL, "a wide window prints every part");
    assert_eq!(
        column.width, 19.0,
        "the column must claim its content, not the room it was offered"
    );

    // A short amount claims less again, down to the heading it must still show.
    let short = ProfitLen::measure(6.22, None, usdt);
    assert_eq!(
        plan_profit_column(
            short,
            short,
            false,
            &metrics(200.0, 4),
            ProfitFloor::default()
        )
        .width,
        10.0,
        "+6.22 USDT is ten characters"
    );
    let tiny = ProfitLen::measure(0.0, None, usdt);
    assert_eq!(
        plan_profit_column(
            tiny,
            tiny,
            false,
            &metrics(200.0, 4),
            ProfitFloor::default()
        )
        .width,
        9.0,
        "the heading is the floor once the values are shorter than it"
    );
}

/// `profit_monitor/format.rs:candidate_forms` must give up the ticker first, the suffix second and
/// the digits last.
///
/// Breakage: abbreviating before dropping the ticker takes precision away from the figure the
/// window exists to show while a removable label is still printed; truncating instead of degrading
/// turns a money value into a different, plausible number.
#[test]
fn a_narrow_profit_column_drops_the_ticker_before_the_suffix() {
    let usdt = Some(ProfitUnit::Quote(QuoteCurrency::usdt()));
    let row = ProfitLen::measure(167.21, Some(3.4), usdt);
    let plan = |available: f32| {
        plan_profit_column(
            row,
            row,
            true,
            &metrics(available, 4),
            ProfitFloor::default(),
        )
    };

    assert_eq!(plan(19.0).form, FULL, "everything fits at its exact width");
    let no_ticker = plan(18.0);
    assert!(
        no_ticker.form.suffix && !no_ticker.form.ticker && !no_ticker.form.si,
        "the ticker goes first, and the heading takes over naming the unit: {:?}",
        no_ticker.form
    );
    let no_suffix = plan(12.0);
    assert!(
        !no_suffix.form.suffix && no_suffix.form.ticker && !no_suffix.form.si,
        "dropping the suffix buys the ticker back: {:?}",
        no_suffix.form
    );
    assert_eq!(no_suffix.width, 12.0, "+167.21 USDT is twelve characters");

    // Abbreviation is reached only once no plain form fits at all.
    let large = ProfitLen::measure(1234567.89, None, usdt);
    assert_eq!(
        plan_profit_column(
            large,
            large,
            false,
            &metrics(16.0, 4),
            ProfitFloor::default()
        )
        .width,
        16.0,
        "+1234567.89 USDT fits sixteen characters and must be printed in full"
    );
    assert!(
        !plan_profit_column(
            large,
            large,
            false,
            &metrics(12.0, 4),
            ProfitFloor::default()
        )
        .form
        .si,
        "eleven exact characters still fit twelve: the ticker goes, not the digits"
    );
    let abbreviated = plan_profit_column(
        large,
        large,
        false,
        &metrics(10.0, 4),
        ProfitFloor::default(),
    );
    assert!(
        abbreviated.form.si,
        "only when no plain form fits does the amount abbreviate: {:?}",
        abbreviated.form
    );
    assert_eq!(
        format_profit(
            1234567.89,
            None,
            usdt,
            ProfitForm {
                suffix: false,
                ticker: true,
                si: true
            }
        )
        .0,
        "+1.23M USDT"
    );
}

/// `profit_monitor/format.rs:plan_profit_column` must let the VALUES decide the form and treat the
/// heading as a floor only.
///
/// Breakage: charging the longer "Прибыль, USDT" heading against the fit test rejects a form whose
/// digits fit, so a money value abbreviates to buy room for a label that ellipsizes harmlessly —
/// and the terminal then prints different precision in different locales.
#[test]
fn a_long_heading_never_costs_a_money_value_its_digits() {
    let usdt = Some(ProfitUnit::Quote(QuoteCurrency::usdt()));
    let row = ProfitLen::measure(167.21, Some(3.4), usdt);
    let wordy = ColumnMetrics {
        heading_with_unit: 100.0,
        ..metrics(14.0, 4)
    };
    let column = plan_profit_column(row, row, true, &wordy, ProfitFloor::default());
    assert!(
        column.form.suffix && !column.form.ticker && !column.form.si,
        "the values fit without the ticker, so no further rung may be taken: {:?}",
        column.form
    );
    assert_eq!(
        column.width, 14.0,
        "an unfittable heading is clamped, never paid for out of the digits"
    );
}

/// `profit_monitor/format.rs:plan_profit_column` must not climb back up the ladder within one
/// period.
///
/// Breakage: re-deriving the form from each snapshot makes one core crossing a digit boundary strip
/// the ticker from every cell and put it back a refresh later, which is the flicker the ratchet
/// exists to stop.
#[test]
fn the_column_does_not_climb_back_up_the_ladder() {
    let usdt = Some(ProfitUnit::Quote(QuoteCurrency::usdt()));
    let row = ProfitLen::measure(167.21, Some(3.4), usdt);
    let held = plan_profit_column(
        row,
        row,
        true,
        &metrics(200.0, 4),
        ProfitFloor {
            width: 0.0,
            rung: 2,
        },
    );
    assert_eq!(
        held.rung, 2,
        "a released rung is not reclaimed by a wide window"
    );
    assert!(!held.form.suffix && held.form.ticker);
}

/// `profit_monitor/format.rs:ColumnFloor::carried` must release the ratchet as soon as the
/// measurement it was taken under moves.
///
/// Breakage: a ratchet with no release keeps a column degraded after the window that degraded it is
/// widened again — the ticker and the suffix would never come back until the period rolled over —
/// and holds a stale width after the Font slider changes the glyph advance underneath it.
#[test]
fn the_floor_releases_itself_when_the_measurement_moves() {
    let usdt = Some(ProfitUnit::Quote(QuoteCurrency::usdt()));
    let metrics = metrics(120.0, 4);
    let taken = ProfitFloor {
        width: 90.0,
        rung: 2,
    };
    let floor = ColumnFloor {
        unit: usdt,
        available: metrics.available,
        row_char: metrics.row_char,
        floor: taken,
    };
    assert_eq!(
        floor.carried(usdt, &metrics),
        taken,
        "same question, same floor"
    );
    assert_eq!(
        floor.carried(Some(ProfitUnit::Quote(QuoteCurrency::btc())), &metrics),
        ProfitFloor::default(),
        "another currency measures different text"
    );
    assert_eq!(
        floor.carried(usdt, &metrics_at(100.0, 4)),
        ProfitFloor::default(),
        "a resized window must be able to climb back up the ladder"
    );
    assert_eq!(
        floor.carried(
            usdt,
            &ColumnMetrics {
                row_char: 2.0,
                ..metrics
            }
        ),
        ProfitFloor::default(),
        "a different glyph advance makes the stored width mean something else"
    );
}

/// `profit_monitor/format.rs:ProfitLen` must fold every measured line, and `value_width` must
/// charge the footer its own larger type step.
///
/// Breakage: measuring only the visible rows makes the column jump while scrolling; measuring the
/// footer at row size truncates the grand total, which is drawn one step up from the rows.
#[test]
fn the_column_is_measured_across_every_line_and_type_size() {
    let usdt = Some(ProfitUnit::Quote(QuoteCurrency::usdt()));
    let mut rows = ProfitLen::measure(6.22, None, usdt);
    rows.absorb(ProfitLen::measure(-12345.67, None, usdt));
    rows.absorb(ProfitLen::measure(25.81, None, usdt));
    // The widest row, "-12345.67 USDT", is fourteen characters.
    assert_eq!(
        plan_profit_column(
            rows,
            ProfitLen::default(),
            false,
            &metrics(200.0, 4),
            ProfitFloor::default()
        )
        .width,
        14.0
    );

    let total = ProfitLen::measure(167.21, None, usdt);
    let footer = ColumnMetrics {
        total_char: 1.5,
        ..metrics(200.0, 4)
    };
    assert_eq!(
        plan_profit_column(
            ProfitLen::default(),
            total,
            false,
            &footer,
            ProfitFloor::default()
        )
        .width,
        18.0,
        "twelve footer characters at one and a half units each"
    );
}

/// `profit_monitor/format.rs:abbreviated` must round to the unit BEFORE abbreviating, and must
/// refuse to touch anything below the 100,000 SI floor.
///
/// Breakage: abbreviating the raw value makes a `-0.004` that the column prints as zero still
/// arrive coloured as a loss; abbreviating below 100,000 routes through `fmt::adaptive`, which
/// re-rounds to five significant digits and prints NO marker — on an eight-decimal quote that
/// silently replaces the row's number with a different one that looks exact.
#[test]
fn an_abbreviated_amount_keeps_its_rounded_sign_and_its_small_digits() {
    let usdt = Some(ProfitUnit::Quote(QuoteCurrency::usdt()));
    let btc = Some(ProfitUnit::Quote(QuoteCurrency::btc()));
    let si = ProfitForm {
        suffix: false,
        ticker: true,
        si: true,
    };
    assert_eq!(
        format_profit(-0.004, None, usdt, si),
        ("0.00 USDT".to_string(), DeltaSign::Zero)
    );
    assert_eq!(
        format_profit(-2_300_000.0, None, usdt, si),
        ("-2.3M USDT".to_string(), DeltaSign::Negative)
    );
    assert_eq!(
        format_profit(12.345_678_91, None, btc, si).0,
        "+12.34567891 BTC",
        "below the SI floor the abbreviated form prints the exact amount"
    );
    assert_eq!(
        format_profit(99_999.99, None, usdt, si).0,
        "+99999.99 USDT",
        "below one hundred thousand the configured fixed decimals remain exact"
    );

    // The column planner agrees with the text: a wide column keeps the ticker and fixed spelling
    // below the SI floor instead of entering an abbreviation rung.
    let small = ProfitLen::measure(99_999.99, None, usdt);
    let planned = plan_profit_column(
        small,
        small,
        false,
        &metrics(200.0, 4),
        ProfitFloor::default(),
    );
    assert!(!planned.form.si, "a sub-floor column must not select SI");
    assert!(
        planned.form.ticker,
        "a wide column keeps the unit beside its fixed digits"
    );
    assert_eq!(
        format_profit(99_999.99, None, usdt, planned.form),
        ("+99999.99 USDT".to_string(), DeltaSign::Positive),
        "the planner and formatter must preserve the same sign, digits, and unit"
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

    // The run controls SEND COMMANDS to cores, so every one of them ships off: a profit window
    // that grew a Stop button in an update is one mis-click away from stopping a fleet.
    let defaults = super::MonitorPrefs::default();
    assert!(
        !defaults.core_status
            && !defaults.trading_buttons
            && !defaults.auto_buttons
            && !defaults.group_controls
            && !defaults.header_controls
    );
    layout.profit_monitor_core_status = Some(true);
    layout.profit_monitor_auto_buttons = Some(true);
    let restored = super::MonitorPrefs::restore(&layout);
    assert!(restored.core_status && restored.auto_buttons && !restored.trading_buttons);
    assert!(
        !restored.group_controls && !restored.header_controls,
        "the scope modifiers say where the chosen controls ALSO appear, and are never implied"
    );

    // The per-group preference was widened from trading alone to every enabled control, and the
    // key it replaced had shipped saying BOTH "the trading control exists" and "captions carry
    // it" — the old `run_slots` reserved the column for it alone. Carrying over only the modifier
    // would leave that profile with no control and no column: a feature gone in an update.
    let mut legacy = moon_core::config::layout::WindowLayout::default();
    legacy.profit_monitor_group_trading = Some(true);
    let carried = super::MonitorPrefs::restore(&legacy);
    assert!(
        carried.group_controls && carried.trading_buttons,
        "the retired key must carry over as both the control and the scope it was shown in"
    );
    assert!(
        super::run_slots(carried).trading,
        "and the column it needs must still be reserved"
    );
    assert!(
        !carried.auto_buttons && !carried.header_controls,
        "it must not hand out controls it never stood for"
    );

    // The carry-over is written back at once, which is what makes it happen exactly once: the key
    // it came from has no writer in the new model, so an unpersisted migration would re-apply at
    // every launch and undo the first thing the user does with the control it handed them.
    assert!(
        carried.persist_migration(&mut legacy),
        "the carried-over preference must be persisted when it is derived"
    );
    assert_eq!(legacy.profit_monitor_group_controls, Some(true));
    assert_eq!(legacy.profit_monitor_trading_buttons, Some(true));
    assert!(
        !carried.persist_migration(&mut legacy),
        "and only the first time"
    );

    // From here the ordinary rows own both keys, including turning what was carried over back off.
    legacy.profit_monitor_trading_buttons = Some(false);
    legacy.profit_monitor_group_controls = Some(false);
    let after = super::MonitorPrefs::restore(&legacy);
    assert!(
        !after.trading_buttons && !after.group_controls,
        "a choice made after the migration must survive the next restore"
    );
}

/// `profit_monitor/mod.rs:run_slots` must reserve a slot for the CONTROL that fills it and for
/// nothing else, and `name_min_width` must pay for the run column out of the Name column down to
/// its floor.
///
/// The Name column pays until it reaches `NAME_COLUMN_FLOOR`; past that the remainder comes out of
/// the slack `MIN_WINDOW_WIDTH` already holds, which is why the loop below asserts the total rather
/// than the subtraction.
///
/// Breakage: letting a scope modifier reserve a column gives every row an empty slot for a button
/// that lives only in the heading; letting a control fail to reserve one leaves a caption carrying
/// a column the rows beneath it do not. Taking the width from anywhere but Name and that slack
/// raises `MIN_WINDOW_WIDTH`, which is exactly the constraint this column was fitted into.
#[test]
fn the_run_column_is_paid_for_by_the_name_column() {
    let mut prefs = super::MonitorPrefs::default();
    assert!(!super::run_slots(prefs).any(), "off by default");
    assert_eq!(
        super::name_min_width(super::run_slots(prefs)),
        super::MIN_NAME_COLUMN_WIDTH
    );

    // One control, one slot: the two are chosen independently.
    prefs.trading_buttons = true;
    let trading = super::run_slots(prefs);
    assert!(trading.trading && !trading.status && !trading.auto);

    let auto = super::run_slots(super::MonitorPrefs {
        auto_buttons: true,
        ..super::MonitorPrefs::default()
    });
    assert!(auto.auto && !auto.trading);

    // The scope modifiers reserve NOTHING on their own: they only decide which lines fill a slot
    // some control already asked for.
    let modifiers_only = super::run_slots(super::MonitorPrefs {
        group_controls: true,
        header_controls: true,
        ..super::MonitorPrefs::default()
    });
    assert!(
        !modifiers_only.any(),
        "a scope modifier without a control must not widen the column"
    );

    // Every combination still fits the unchanged minimum window width.
    for (status, trading, auto) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (true, true, false),
        (true, true, true),
    ] {
        let slots = super::run_slots(super::MonitorPrefs {
            core_status: status,
            trading_buttons: trading,
            auto_buttons: auto,
            ..super::MonitorPrefs::default()
        });
        let used = super::name_min_width(slots)
            + slots.width()
            + super::PROFIT_MIN_COLUMN_WIDTH
            + 2.0 * super::TABLE_HORIZONTAL_PADDING
            // One gap to the profit column, plus the run column's own gap to the name.
            + 2.0 * super::TABLE_COLUMN_GAP;
        assert!(
            used <= super::MIN_WINDOW_WIDTH,
            "run column {slots:?} pushed the table to {used} past the {} minimum",
            super::MIN_WINDOW_WIDTH
        );
        assert!(
            super::name_min_width(slots) >= super::NAME_COLUMN_FLOOR,
            "the name column must keep its floor"
        );
    }
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

/// `profit_monitor/model.rs:scoped_query_core_ids` must retain only data-only previously seen
/// cores, and keep an unhidden preset unfiltered: inverting its configured-core filter or returning
/// `core_order` when nothing is hidden silently removes real core money from the monitor total.
#[test]
fn a_data_only_core_keeps_its_money_inside_a_scoped_read() {
    let unhidden = LiveContext {
        preset: Some(moon_core::config::WorkspaceMode::AutoTrading),
        core_order: vec![1, 2],
        configured_total: 2,
        configured_core_ids: HashSet::from([1, 2]),
        ..LiveContext::default()
    };
    assert_eq!(
        scoped_query_core_ids(&unhidden, &[99]),
        Vec::<u64>::new(),
        "an unhidden preset must leave the database read unfiltered"
    );

    let hidden = LiveContext {
        preset: Some(moon_core::config::WorkspaceMode::AutoTrading),
        core_order: vec![1],
        configured_total: 2,
        configured_core_ids: HashSet::from([1, 2]),
        ..LiveContext::default()
    };
    assert_eq!(
        scoped_query_core_ids(&hidden, &[99]),
        vec![1, 99],
        "a previously observed core outside config.servers is data-only and must keep its money"
    );
    assert_eq!(
        scoped_query_core_ids(&hidden, &[2]),
        vec![1],
        "a configured-but-hidden core is intentionally excluded by core_order"
    );

    let hides_every_configured_core = LiveContext {
        core_order: Vec::new(),
        configured_total: 2,
        configured_core_ids: HashSet::from([1, 2]),
        ..hidden
    };
    assert_eq!(
        scoped_query_core_ids(&hides_every_configured_core, &[1, 2]),
        vec![moon_core::config::NO_MATCH_CORE_UID],
        "a present scope that resolves empty must fail closed instead of broadening to every core"
    );
}
