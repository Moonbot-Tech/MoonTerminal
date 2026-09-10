//! Money presentation must stay honest when valuation or currency identity is incomplete.
use super::{Page, profit, render};

/// A viewer never receives another client's rows or money, including daily and exchange totals.
#[test]
fn viewer_membership_filters_every_report_view_and_total() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER,core_name TEXT,newrecid INTEGER,closedate INTEGER,profitbtc REAL,spentbtc REAL,basecurrency INTEGER);
        INSERT INTO orders_rep VALUES (1,'Client core',1,150,7,100,0),(2,'Private other client',1,150,900,100,0),(3,'Archived other client',1,150,800,100,0);").unwrap();
    let venues = std::collections::HashMap::from([
        (1, moon_core::venue::CoreVenue::identify(2, "", None)),
        (2, moon_core::venue::CoreVenue::identify(6, "", None)),
    ]);
    for (daily, by_exchange) in [(false, false), (false, true), (true, false)] {
        let mut request = ReportRequest::new(Period::Today, daily);
        request.by_exchange = by_exchange;
        let page = super::read_page_on(&conn, request, 100, 200, chrono_tz::UTC, |_| {
            (venues.clone(), super::TelegramReportAccess::Viewer(vec![1]))
        })
        .unwrap();
        assert_eq!(page.total.orders, 1);
        assert_eq!(page.total.totals[0].profit, 7.0);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].1.totals[0].profit, 7.0);
        let Response::Rich { html, .. } = render(&page) else {
            panic!("expected report");
        };
        assert!(!html.contains("other client"));
        assert!(!html.contains("900"));
        assert!(!html.contains("800"));
    }
}

/// Empty grants, stale core IDs, and another client's old exchange callback all fail closed.
#[test]
fn viewer_empty_or_unavailable_scope_never_becomes_global() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER,core_name TEXT,newrecid INTEGER,closedate INTEGER,profitbtc REAL,spentbtc REAL,basecurrency INTEGER);
        INSERT INTO orders_rep VALUES (1,'Client',1,150,7,100,0),(2,'Private',1,150,900,100,0);").unwrap();
    for ids in [vec![], vec![99], vec![1]] {
        let mut request = ReportRequest::new(Period::Today, false);
        if ids == vec![1] {
            request.scope = moon_core::telegram::report::ReportScope::Venue(
                moon_core::feed::ExchangeId::new(6),
            );
        }
        let page = super::read_page_on(&conn, request, 100, 200, chrono_tz::UTC, |_| {
            (
                std::collections::HashMap::from([
                    (1, moon_core::venue::CoreVenue::identify(2, "", None)),
                    (2, moon_core::venue::CoreVenue::identify(6, "", None)),
                ]),
                super::TelegramReportAccess::Viewer(ids),
            )
        })
        .unwrap();
        assert_eq!(page.total.orders, 0);
        assert!(page.rows.is_empty());
        assert!(page.drilldowns.is_empty());
    }
}

/// A paged report shows the whole-scope total after its rows, never as a headline or page sum.
#[test]
fn full_total_is_the_final_summary_row() {
    let total = QuoteBreakdown {
        orders: 12,
        valuation: Some(ValuationCoverage {
            eligible_orders: 12,
            valued_orders: 12,
            unavailable_orders: 0,
            usdt: Some(UsdtTotal {
                profit: 123.45,
                spent: Some(1200.0),
            }),
        }),
        ..Default::default()
    };
    let page = Page {
        request: ReportRequest::new(Period::Today, false),
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total,
        rows: vec![(
            "Visible row".into(),
            QuoteBreakdown::from_groups([(Some(0), 1.0, 1)]),
        )],
        pages: 2,
        drilldowns: Vec::new(),
        scope_label: None,
    };
    let Response::Rich { html, .. } = render(&page) else {
        panic!("expected report")
    };
    let table_start = html.find("<table").unwrap();
    let table_end = html.find("</table>").unwrap();
    let total_at = html.find("+123.45").unwrap();
    assert!(total_at > html.find("Visible row").unwrap() && total_at < table_end);
    assert!(!html[..table_start].contains("+123.45"));
    assert!(html[table_start..table_end].contains("<b>12</b>"));
    assert_eq!(
        html.matches("<details>").count(),
        1,
        "calculation explanation belongs in Help"
    );
}

/// Inline navigation changes the current report, never duplicates global period selection.
#[test]
fn inline_buttons_keep_the_current_period() {
    let mut request = ReportRequest::new(Period::Yesterday, false);
    request.window = Some((0, 1));
    let page = Page {
        request,
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: QuoteBreakdown::default(),
        rows: Vec::new(),
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
    };
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = super::keyboard(&page) else {
        panic!("expected inline navigation")
    };
    for button in markup.inline_keyboard.into_iter().flatten() {
        let request =
            ReportRequest::parse_callback(button.callback_data.as_deref().unwrap()).unwrap();
        assert_eq!(
            request.period,
            Period::Yesterday,
            "{} changed the period",
            button.text
        );
        assert_eq!(
            request.window, page.request.window,
            "inline navigation must not refresh a preset"
        );
    }
}

/// Even an unavailable average must disclose that all entries were excluded.
#[test]
fn unavailable_average_keeps_nonzero_exclusion_disclosure() {
    let total = QuoteBreakdown::from_groups([(Some(0), 0.1, 2)]);
    assert!(total.average_order_return().is_none());
    let page = Page {
        request: ReportRequest::new(Period::Today, false),
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: total.clone(),
        rows: vec![("Fixture".into(), total)],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
    };
    let Response::Rich { html, .. } = render(&page) else {
        panic!("expected report")
    };
    let coverage = rust_i18n::t!(
        "telegram.report_average_coverage",
        counted = 0,
        excluded = 2
    )
    .to_string();
    assert!(html.contains(&super::escape(&coverage)));
}

/// Dollar-denominated subtotals should not expose eight-decimal replica noise.
#[test]
fn detail_money_uses_readable_currency_precision() {
    let total = QuoteBreakdown {
        totals: vec![moon_core::db::QuoteTotal {
            currency: moon_core::db::QuoteCurrency::usdt(),
            profit: 47.58247420,
            orders: 158,
        }],
        orders: 158,
        ..Default::default()
    };
    assert_eq!(super::native(&total), "+47.58 USDT");
    let btc = QuoteBreakdown {
        totals: vec![moon_core::db::QuoteTotal {
            currency: moon_core::db::QuoteCurrency::btc(),
            profit: 0.00001234,
            orders: 1,
        }],
        orders: 1,
        ..Default::default()
    };
    assert_eq!(super::native(&btc), "+0.00001234 BTC");
}

/// Idle rows interleaved with zero-profit activity must neither consume page slots nor disappear from totals.
#[test]
fn inactive_cores_do_not_consume_page_slots() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER,core_name TEXT,newrecid INTEGER,closedate INTEGER,profitbtc REAL,spentbtc REAL,basecurrency INTEGER);").unwrap();
    for id in 1..=18 {
        conn.execute(
            "INSERT INTO orders_rep VALUES (?1,?2,1,?3,0,100,0)",
            rusqlite::params![id, id.to_string(), if id % 2 == 1 { 150 } else { 99 }],
        )
        .unwrap();
    }
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = false;
    let first = super::read_page_on(&conn, request.clone(), 100, 200, chrono_tz::UTC, |rows| {
        rows.sort_by_key(|(id, _)| *id);
        (Default::default(), super::TelegramReportAccess::Owner)
    })
    .unwrap();
    assert_eq!(first.total.orders, 9);
    assert_eq!(first.rows.len(), 6);
    assert_eq!(first.pages, 2);
    request.page = 1;
    let last = super::read_page_on(&conn, request, 100, 200, chrono_tz::UTC, |rows| {
        rows.sort_by_key(|(id, _)| *id);
        (Default::default(), super::TelegramReportAccess::Owner)
    })
    .unwrap();
    assert_eq!(
        last.rows
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["13", "15", "17"]
    );
    assert_eq!(last.total.orders, 9);
}

/// Exchange aggregation keeps zero-profit activity, drops idle groups, and never broadens a missing scope.
#[test]
fn exchanges_group_real_identities_and_filter_idle_groups_before_paging() {
    use moon_core::{feed::ExchangeId, telegram::report::ReportScope, venue::CoreVenue};
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER);
    INSERT INTO orders_rep VALUES (1,'Misleading Bybit name',1,150,10,100,0),(2,'Second',1,150,-10,100,0),(3,'Idle',1,99,1000,100,0),(4,'Archive',1,150,7,100,0);").unwrap();
    let venues = std::collections::HashMap::from([
        (1, CoreVenue::identify(2, "", None)),
        (2, CoreVenue::identify(2, "", None)),
        (3, CoreVenue::identify(6, "", None)),
    ]);
    let request = ReportRequest::new(Period::Today, false);
    let page = super::read_page_on(&conn, request.clone(), 100, 200, chrono_tz::UTC, |_| {
        (venues.clone(), super::TelegramReportAccess::Owner)
    })
    .unwrap();
    assert_eq!(page.total.orders, 3);
    assert_eq!(page.rows.len(), 2);
    let zero = page
        .rows
        .iter()
        .find(|(_, total)| total.orders == 2)
        .unwrap();
    assert_eq!(zero.1.totals[0].profit, 0.0);
    assert_eq!(page.drilldowns.len(), 2);
    let mut scoped = request;
    scoped.scope = ReportScope::Venue(ExchangeId::new(13));
    scoped.by_exchange = false;
    let empty = super::read_page_on(&conn, scoped, 100, 200, chrono_tz::UTC, |_| {
        (venues, super::TelegramReportAccess::Owner)
    })
    .unwrap();
    assert_eq!(empty.total.orders, 0);
    assert!(empty.rows.is_empty());
}

/// Reinterpreting a frozen leap-year window in UTC+1 must retain its last partial day.
#[test]
fn daily_pages_include_partial_day_after_zone_change() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut request = ReportRequest::dates("2024-01-01", "2024-12-31").unwrap();
    let (from, to) = request.bounds(0, chrono_tz::UTC).unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER);").unwrap();
    conn.execute(
        "INSERT INTO orders_rep VALUES (1,'Fixture',1,?1,1,100,0)",
        [to],
    )
    .unwrap();
    request.daily = true;
    request.page = 36;
    let page = super::read_page_on(&conn, request, from, to, chrono_tz::Europe::Warsaw, |_| {
        (Default::default(), super::TelegramReportAccess::Owner)
    })
    .unwrap();
    assert_eq!(page.rows.last().unwrap().0, "2025-01-01");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.total.orders, 1);
    assert_eq!(page.rows.last().unwrap().1.orders, 1);
}

/// Small native averages must retain significant digits and exclude uncounted entries.
#[test]
fn native_average_keeps_small_btc_amount_visible() {
    let mut total = QuoteBreakdown::from_groups([(Some(0), 0.0001, 2)]);
    total.entry_spend = moon_core::db::EntrySpend {
        totals: vec![moon_core::db::QuoteSpend {
            currency: moon_core::db::QuoteCurrency::btc(),
            spent: 0.001,
            profit: 0.0001,
            orders: 1,
        }],
        counted_orders: 1,
        ..Default::default()
    };
    let page = Page {
        request: ReportRequest::new(Period::Today, false),
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: total.clone(),
        rows: vec![("Fixture".into(), total)],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
    };
    let Response::Rich { html, .. } = render(&page) else {
        panic!("expected rich report")
    };
    assert!(html.replace("&#160;", " ").contains("0.001 BTC"));
    assert!(!html.replace("&#160;", " ").contains("0.00 BTC"));
}

/// The production reader excludes open/emulator/deleted rows and keeps page totals global.
#[test]
fn report_reader_preserves_filters_and_full_total_across_pages() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (
        core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER, buydate INTEGER,
        profitbtc REAL, spentbtc REAL, basecurrency INTEGER, emulator INTEGER, deleted INTEGER
    );
    INSERT INTO orders_rep VALUES (1,'First',1,100,50,10,100,0,0,0),
      (1,'First',2,200,50,-3,100,0,0,0),
      (1,'First',3,0,50,1000,100,0,0,0),
      (1,'First',4,150,50,1000,100,0,1,0),
      (1,'First',5,150,50,1000,100,0,0,1),
      (1,'First',6,201,50,1000,100,0,0,0);",
    )
    .unwrap();
    for id in 2..=12 {
        conn.execute(
            "INSERT INTO orders_rep VALUES (?1,'Other',1,150,50,1,100,0,0,0)",
            [id],
        )
        .unwrap();
    }
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = false;
    let first = super::read_page_on(&conn, request.clone(), 100, 200, chrono_tz::UTC, |rows| {
        rows.sort_by_key(|(id, _)| *id);
        (Default::default(), super::TelegramReportAccess::Owner)
    })
    .unwrap();
    assert_eq!(first.total.orders, 13);
    assert_eq!(first.total.totals[0].profit, 18.0);
    assert_eq!(first.rows.len(), 6);
    assert_eq!(first.rows[0].1.orders, 2);
    assert_eq!(first.rows[0].1.totals[0].profit, 7.0);
    request.page = 1;
    let second = super::read_page_on(&conn, request, 100, 200, chrono_tz::UTC, |rows| {
        rows.sort_by_key(|(id, _)| *id);
        (Default::default(), super::TelegramReportAccess::Owner)
    })
    .unwrap();
    assert_eq!(second.total.orders, 13);
    assert_eq!(second.total.totals[0].profit, 18.0);
    assert_eq!(second.rows.len(), 6);
}
use moon_core::{
    db::{QuoteBreakdown, UsdtTotal, ValuationCoverage},
    telegram::{
        report::{Period, ReportRequest},
        runtime::Response,
    },
};

/// Missing valuation must never display a native BTC subtotal as USDT or invent a zero.
#[test]
fn unvalued_and_unknown_money_is_not_a_usdt_total() {
    let total = QuoteBreakdown::from_groups([(Some(0), 2.0, 1)]);
    assert_eq!(profit(&total), rust_i18n::t!("telegram.report_unvalued"));
    let total = QuoteBreakdown::from_groups([(None, 2.0, 1)]).with_valuation(ValuationCoverage {
        eligible_orders: 0,
        valued_orders: 0,
        unavailable_orders: 0,
        usdt: Some(UsdtTotal {
            profit: 2.0,
            spent: None,
        }),
    });
    assert!(total.unified_usdt().is_none());
    assert_eq!(profit(&total), rust_i18n::t!("telegram.report_unvalued"));
}

/// Bot names are data, including markup, newlines, and excessive text.
#[test]
fn rich_report_escapes_names_and_bounds_long_labels() {
    let total = QuoteBreakdown::default();
    let page = Page {
        request: ReportRequest::new(Period::Today, false),
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: total.clone(),
        rows: vec![(format!("<b>&{}", "x".repeat(50_000)), total)],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
    };
    let Response::Rich { html, .. } = render(&page) else {
        panic!("expected rich report")
    };
    assert!(html.contains("&lt;b&gt;&amp;"));
    assert!(!html.contains("<b>&xxxx"));
    assert!(html.len() < 10_000);
}

/// Single-day navigation omits daily aggregation while longer periods still expose it.
#[test]
fn today_omits_daily_navigation() {
    for (period, expected) in [(Period::Today, false), (Period::Month, true)] {
        let page = Page {
            request: ReportRequest::new(period, false),
            from: 0,
            to: 1,
            zone: chrono_tz::UTC,
            total: QuoteBreakdown::default(),
            rows: Vec::new(),
            pages: 1,
            drilldowns: Vec::new(),
            scope_label: None,
        };
        let moon_core::telegram::api::ReplyMarkup::Inline(markup) = super::keyboard(&page) else {
            panic!("expected inline navigation")
        };
        assert!(markup.inline_keyboard.iter().all(|row| !row.is_empty()));
        assert_eq!(
            markup.inline_keyboard.iter().flatten().any(|button| {
                ReportRequest::parse_callback(button.callback_data.as_deref().unwrap())
                    .unwrap()
                    .daily
            }),
            expected
        );
    }
}

/// Long core identities get the full table width rather than an ambiguous middle truncation.
#[test]
fn core_names_span_the_money_columns() {
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = false;
    let name = "VLTR$18 / SUB ACC No 11 with a long server name";
    let page = Page {
        request,
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: QuoteBreakdown::default(),
        rows: vec![(name.into(), QuoteBreakdown::default())],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
    };
    let Response::Rich { html, .. } = render(&page) else {
        panic!("expected rich report")
    };
    assert!(html.contains(&format!("<td colspan=\"3\"><b>{name}</b>")));
    assert!(html.contains(&format!("<td colspan=\"2\"><b>{name}</b>")));
}

/// Deletable Help must not own the persistent keyboard; accounting stays collapsed and escaped.
#[test]
fn help_keeps_persistent_navigation_on_a_separate_message() {
    let Response::Rich {
        html,
        keyboard,
        navigation,
    } = super::help("<UTC>")
    else {
        panic!("expected rich help")
    };
    assert_eq!(html.matches("<details>").count(), 3);
    assert!(!html.contains("<details open"));
    assert!(html.contains("&lt;UTC&gt;"));
    assert!(matches!(
        navigation.1,
        moon_core::telegram::api::ReplyMarkup::Reply(_)
    ));
    assert!(!navigation.0.is_empty());
    assert!(matches!(
        keyboard,
        moon_core::telegram::api::ReplyMarkup::Inline(_)
    ));
}
