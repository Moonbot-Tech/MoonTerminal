//! Regression tests for Profit Monitor row grouping and refresh decisions.

use std::collections::HashMap;

use moon_core::db::analytics::{ProfitMonitorCore, ProfitMonitorSummary};
use moon_core::db::{ProfitUnit, QuoteCurrency};
use moon_core::util::fmt::DeltaSign;

use super::rows::{GroupMode, LiveContext, RowLabels, grouped_rows};
use super::{
    ContextChange, MonitorPeriod, MonitorSort, MonitorSortColumn, VisibleColumns,
    duration_until_period_refresh, format_profit, next_sort, retain_last_known_exchange_names,
    sort_rows,
};
use std::time::{Duration, UNIX_EPOCH};

/// Return deterministic fallback labels for pure grouping tests.
///
/// Returns:
///     English labels that keep expected rows readable.
fn labels() -> RowLabels<'static> {
    RowLabels {
        core: "Core",
        unknown_exchange: "Unknown exchange",
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
        super::context_change(&before, &after, false),
        ContextChange::Regroup
    );
    assert_eq!(
        super::context_change(&after, &after, true),
        ContextChange::Reload
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

/// `profit_monitor/mod.rs:VisibleColumns::for_width` must drop Average order before Win rate while
/// preserving both exact boundaries; swapping the thresholds makes this boundary-pair assertion
/// red and clips essential Profit/Trades columns in a narrow desktop widget.
#[test]
fn responsive_columns_degrade_in_the_documented_order() {
    assert_eq!(
        VisibleColumns::for_width(619.0),
        VisibleColumns {
            win_rate: false,
            average_order: false,
        }
    );
    assert_eq!(
        VisibleColumns::for_width(620.0),
        VisibleColumns {
            win_rate: true,
            average_order: false,
        }
    );
    assert_eq!(
        VisibleColumns::for_width(760.0),
        VisibleColumns {
            win_rate: true,
            average_order: true,
        }
    );
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
            profit: 5.0,
            trades: 4,
            wins: 1,
            positive_spent: 60.0,
            positive_orders: 2,
            primary_core: 1,
        },
        super::rows::MonitorRow {
            name: "Beta".to_string(),
            profit: 12.0,
            trades: 2,
            wins: 2,
            positive_spent: 10.0,
            positive_orders: 1,
            primary_core: 2,
        },
        super::rows::MonitorRow {
            name: "Gamma".to_string(),
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

/// `profit_monitor/mod.rs:duration_until_period_refresh` must derive every wait from the next
/// UTC boundary; using one fixed minute loop for every preset would rescan a full Year 1,440 times
/// per day, while a process-relative interval would drift after executor delays.
#[test]
fn clock_refresh_uses_minute_midnight_or_no_timer_by_period() {
    assert_eq!(
        duration_until_period_refresh(
            MonitorPeriod::Hour,
            UNIX_EPOCH + Duration::from_millis(123_456)
        ),
        Some(Duration::from_millis(56_544))
    );
    assert_eq!(
        duration_until_period_refresh(
            MonitorPeriod::Year,
            UNIX_EPOCH + Duration::from_secs(43_230)
        ),
        Some(Duration::from_secs(43_170)),
        "calendar presets must wait for UTC midnight instead of polling every minute"
    );
    assert_eq!(
        duration_until_period_refresh(MonitorPeriod::All, UNIX_EPOCH),
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
