//! Money presentation must stay honest when valuation or currency identity is incomplete.
use super::Page;
use super::render::{profit, render};

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
        let page = super::read_page_on(
            &conn,
            request,
            100,
            200,
            chrono_tz::UTC,
            &Default::default(),
            |_| (venues.clone(), super::TelegramReportAccess::Viewer(vec![1])),
        )
        .unwrap();
        assert_eq!(page.total.orders, 1);
        assert_eq!(page.total.totals[0].profit, 7.0);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].1.totals[0].profit, 7.0);
        let Response::Rich { html, .. } = render(&page, crate::HostKind::Terminal, true) else {
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
        let viewer_core = ids == vec![1];
        if viewer_core {
            request.scope = moon_core::telegram::report::ReportScope::Venue(
                moon_core::feed::ExchangeId::new(6),
            );
        }
        let page = super::read_page_on(
            &conn,
            request,
            100,
            200,
            chrono_tz::UTC,
            &Default::default(),
            |_| {
                (
                    std::collections::HashMap::from([
                        (1, moon_core::venue::CoreVenue::identify(2, "", None)),
                        (2, moon_core::venue::CoreVenue::identify(6, "", None)),
                    ]),
                    super::TelegramReportAccess::Viewer(ids),
                )
            },
        )
        .unwrap();
        assert_eq!(page.total.orders, 0);
        assert!(page.rows.is_empty());
        if viewer_core {
            assert_eq!(
                page.drilldowns.len(),
                1,
                "the viewer's own exchanges stay on the keyboard"
            );
            assert_ne!(
                page.drilldowns[0].1,
                moon_core::telegram::report::ReportScope::Venue(moon_core::feed::ExchangeId::new(
                    6
                ))
            );
        } else {
            assert!(page.drilldowns.is_empty());
        }
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
    let Response::Rich { html, .. } = render(&page, crate::HostKind::Terminal, true) else {
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
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = super::render::keyboard(&page)
    else {
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
    let _locale = crate::test_locale::force("en");
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
    let Response::Rich { html, .. } = render(&page, crate::HostKind::Terminal, true) else {
        panic!("expected report")
    };
    let coverage = rust_i18n::t!(
        "telegram.report_average_coverage",
        counted = 0,
        excluded = 2
    )
    .to_string();
    assert!(html.contains(&super::render::escape(&coverage)));
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
    assert_eq!(super::render::native(&total), "+47.58 USDT");
    let btc = QuoteBreakdown {
        totals: vec![moon_core::db::QuoteTotal {
            currency: moon_core::db::QuoteCurrency::btc(),
            profit: 0.00001234,
            orders: 1,
        }],
        orders: 1,
        ..Default::default()
    };
    assert_eq!(super::render::native(&btc), "+0.00001234 BTC");
}

/// Idle rows interleaved with zero-profit activity neither take a row nor disappear from totals, and
/// a core list that fits the message is one page, not six-row pages.
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
    let page = super::read_page_on(
        &conn,
        request,
        100,
        200,
        chrono_tz::UTC,
        &Default::default(),
        |rows| {
            rows.sort_by_key(|(id, _)| *id);
            (Default::default(), super::TelegramReportAccess::Owner)
        },
    )
    .unwrap();
    assert_eq!(page.total.orders, 9);
    assert_eq!(page.pages, 1);
    assert_eq!(
        page.rows
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "3", "5", "7", "9", "11", "13", "15", "17"]
    );
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
    let page = super::read_page_on(
        &conn,
        request.clone(),
        100,
        200,
        chrono_tz::UTC,
        &Default::default(),
        |_| (venues.clone(), super::TelegramReportAccess::Owner),
    )
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
    let empty = super::read_page_on(
        &conn,
        scoped,
        100,
        200,
        chrono_tz::UTC,
        &Default::default(),
        |_| (venues, super::TelegramReportAccess::Owner),
    )
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
    let page = super::read_page_on(
        &conn,
        request,
        from,
        to,
        chrono_tz::Europe::Warsaw,
        &Default::default(),
        |_| (Default::default(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert_eq!(page.rows.last().unwrap().0, "2025-01-01");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.total.orders, 1);
    assert_eq!(page.rows.last().unwrap().1.orders, 1);
}

/// A month of daily rows and a handful of exchanges fit one message.
#[test]
fn breakdown_views_render_every_row_until_the_rich_message_limit() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER);").unwrap();
    let mut daily = ReportRequest::dates("2026-01-01", "2026-01-31").unwrap();
    daily.daily = true;
    daily.by_exchange = false;
    let (from, to) = daily.bounds(0, chrono_tz::UTC).unwrap();
    for day in 0..31 {
        let closedate = from + day * 86_400 + 3_600;
        conn.execute(
            "INSERT INTO orders_rep VALUES (1,'Fixture',?1,?2,1,100,0)",
            rusqlite::params![day + 1, closedate],
        )
        .unwrap();
    }
    let page = super::read_page_on(
        &conn,
        daily,
        from,
        to,
        chrono_tz::UTC,
        &Default::default(),
        |_| (Default::default(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert_eq!(page.rows.len(), 31);
    assert_eq!(page.pages, 1);
    let Response::Rich { html, keyboard, .. } = render(&page, crate::HostKind::Terminal, true)
    else {
        panic!("expected rich report")
    };
    assert!(
        super::rich_message_fits(&html),
        "chars={} blocks={}",
        html.chars().count(),
        super::render::rich_message_blocks(&html)
    );
    assert!(!html.contains(&rust_i18n::t!("telegram.report_page").to_string()));
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = keyboard else {
        panic!("expected inline navigation")
    };
    assert!(markup.inline_keyboard.iter().flatten().all(|button| {
        ReportRequest::parse_callback(button.callback_data.as_deref().unwrap())
            .unwrap()
            .page
            == 0
    }));

    conn.execute("DELETE FROM orders_rep", []).unwrap();
    let codes = [2_u8, 3, 4, 5, 6, 7];
    let mut venues = std::collections::HashMap::new();
    for (index, code) in codes.into_iter().enumerate() {
        let id = (index as u64) + 1;
        conn.execute(
            "INSERT INTO orders_rep VALUES (?1,?2,1,150,1,100,0)",
            rusqlite::params![id as i64, format!("core-{id}")],
        )
        .unwrap();
        venues.insert(id, moon_core::venue::CoreVenue::identify(code, "", None));
    }
    let request = ReportRequest::new(Period::Today, false);
    let page = super::read_page_on(
        &conn,
        request,
        100,
        200,
        chrono_tz::UTC,
        &Default::default(),
        |_| (venues.clone(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert_eq!(page.rows.len(), 6);
    assert_eq!(page.pages, 1);
    let Response::Rich { html, keyboard, .. } = render(&page, crate::HostKind::Terminal, true)
    else {
        panic!("expected rich report")
    };
    assert!(
        super::rich_message_fits(&html),
        "chars={} blocks={}",
        html.chars().count(),
        super::render::rich_message_blocks(&html)
    );
    assert!(!html.contains(&rust_i18n::t!("telegram.report_page").to_string()));
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = keyboard else {
        panic!("expected inline navigation")
    };
    assert!(markup.inline_keyboard.iter().flatten().all(|button| {
        !button
            .callback_data
            .as_deref()
            .and_then(ReportRequest::parse_callback)
            .is_some_and(|request| request.page > 0)
    }));
}

/// Half a year of daily rows exceeds Telegram's 500-block cap, so the report pages — with the
/// largest page that fits, not six rows.
#[test]
fn oversized_daily_report_still_pages() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER);").unwrap();
    let mut request = ReportRequest::dates("2026-01-01", "2026-06-30").unwrap();
    request.daily = true;
    request.by_exchange = false;
    let (from, to) = request.bounds(0, chrono_tz::UTC).unwrap();
    let days = ((to - from) / 86_400) + 1;
    for day in 0..days {
        let closedate = from + day * 86_400 + 3_600;
        conn.execute(
            "INSERT INTO orders_rep VALUES (1,'Fixture',?1,?2,1,100,0)",
            rusqlite::params![day + 1, closedate.min(to)],
        )
        .unwrap();
    }
    let page = super::read_page_on(
        &conn,
        request,
        from,
        to,
        chrono_tz::UTC,
        &Default::default(),
        |_| (Default::default(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert!(
        page.pages > 1,
        "expected paging after {} active days, got {} pages / {} rows",
        days,
        page.pages,
        page.rows.len()
    );
    assert!(page.rows.len() > 6, "got {} rows per page", page.rows.len());
    assert!(super::rich_message_fits(&super::report_html(&page)));
    let Response::Rich { html, keyboard, .. } = render(&page, crate::HostKind::Terminal, true)
    else {
        panic!("expected rich report")
    };
    assert!(html.contains(&rust_i18n::t!("telegram.report_page").to_string()));
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = keyboard else {
        panic!("expected inline navigation")
    };
    assert!(markup.inline_keyboard.iter().flatten().any(|button| {
        ReportRequest::parse_callback(button.callback_data.as_deref().unwrap())
            .unwrap()
            .page
            == 1
    }));
}

/// A core roster too long for one message pages with pages far larger than six rows, and every
/// page it makes fits the rich-message caps.
///
/// Mutation: restore the fixed six-row page for cores. The roster then needs 50 pages.
#[test]
fn an_oversized_core_list_pages_with_large_fitting_pages() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER,core_name TEXT,newrecid INTEGER,closedate INTEGER,profitbtc REAL,spentbtc REAL,basecurrency INTEGER);").unwrap();
    for id in 1..=300 {
        conn.execute(
            "INSERT INTO orders_rep VALUES (?1,?2,1,150,1,100,0)",
            rusqlite::params![
                id,
                format!("Desk {id:03} / a long account name of this core")
            ],
        )
        .unwrap();
    }
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = false;
    let mut seen = 0;
    let mut page_index = 0;
    loop {
        request.page = page_index;
        let page = super::read_page_on(
            &conn,
            request.clone(),
            100,
            200,
            chrono_tz::UTC,
            &Default::default(),
            |rows| {
                rows.sort_by_key(|(id, _)| *id);
                (Default::default(), super::TelegramReportAccess::Owner)
            },
        )
        .unwrap();
        assert!(page.pages > 1 && page.pages < 50, "{} pages", page.pages);
        assert!(super::rich_message_fits(&super::report_html(&page)));
        seen += page.rows.len();
        page_index += 1;
        if page_index == page.pages {
            break;
        }
    }
    assert_eq!(
        seen, 300,
        "every core appears exactly once across the pages"
    );
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
    let Response::Rich { html, .. } = render(&page, crate::HostKind::Terminal, true) else {
        panic!("expected rich report")
    };
    assert!(html.replace("&#160;", " ").contains("0.001 BTC"));
    assert!(!html.replace("&#160;", " ").contains("0.00 BTC"));
}

/// The production reader excludes open/emulator/deleted rows, keeps the total global, and clamps a
/// stale page index to the pages that exist.
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
    let first = super::read_page_on(
        &conn,
        request.clone(),
        100,
        200,
        chrono_tz::UTC,
        &Default::default(),
        |rows| {
            rows.sort_by_key(|(id, _)| *id);
            (Default::default(), super::TelegramReportAccess::Owner)
        },
    )
    .unwrap();
    assert_eq!(first.total.orders, 13);
    assert_eq!(first.total.totals[0].profit, 18.0);
    assert_eq!(first.rows.len(), 12);
    assert_eq!(first.pages, 1);
    assert_eq!(first.rows[0].1.orders, 2);
    assert_eq!(first.rows[0].1.totals[0].profit, 7.0);
    request.page = 1;
    let second = super::read_page_on(
        &conn,
        request,
        100,
        200,
        chrono_tz::UTC,
        &Default::default(),
        |rows| {
            rows.sort_by_key(|(id, _)| *id);
            (Default::default(), super::TelegramReportAccess::Owner)
        },
    )
    .unwrap();
    assert_eq!(second.request.page, 0);
    assert_eq!(second.total.orders, 13);
    assert_eq!(second.total.totals[0].profit, 18.0);
    assert_eq!(second.rows.len(), 12);
}
use moon_core::{
    db::{QuoteBreakdown, UsdtTotal, ValuationCoverage},
    telegram::{
        report::{Period, ReportRequest},
        runtime::Response,
    },
};

/// Ignoring the role in rich report or Help navigation would reinstall Status for a viewer.
#[test]
fn station_reports_and_help_limit_navigation_to_the_owner() {
    let page = Page {
        request: ReportRequest::new(Period::Today, false),
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: QuoteBreakdown::default(),
        rows: Vec::new(),
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
    };
    for locale in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let labels = crate::labels::telegram_labels(crate::HostKind::Station);
        for owner in [false, true] {
            for response in [
                render(&page, crate::HostKind::Station, owner),
                super::help("UTC", crate::HostKind::Station, owner),
            ] {
                let Response::Rich { navigation, .. } = response else {
                    panic!("expected rich response")
                };
                let moon_core::telegram::api::ReplyMarkup::Reply(markup) = navigation.1 else {
                    panic!("expected persistent navigation")
                };
                assert_eq!(
                    markup.keyboard.iter().flatten().any(|button| {
                        moon_core::telegram::commands::parse_reply_button(&button.text, &labels)
                            == moon_core::telegram::commands::ParsedCommand::StationStatus
                    }),
                    owner
                );
            }
            let Response::Rich { html, .. } = super::help("UTC", crate::HostKind::Station, owner)
            else {
                panic!("expected help")
            };
            assert_eq!(html.contains("<code>/status</code>"), owner);
        }
    }
}

/// Missing valuation must never display a native BTC subtotal as USDT or invent a zero.
#[test]
fn unvalued_and_unknown_money_is_not_a_usdt_total() {
    let _locale = crate::test_locale::force("en");
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
    let Response::Rich { html, .. } = render(&page, crate::HostKind::Terminal, true) else {
        panic!("expected rich report")
    };
    assert!(html.contains("&lt;b&gt;&amp;"));
    assert!(!html.contains("<b>&xxxx"));
    assert!(html.len() < 10_000);
}

/// Collapsed markup is one row of Exchanges + All cores; expanding shows today's list plus Back.
#[test]
fn collapsed_keyboard_hides_exchanges_until_opened() {
    let _locale = crate::test_locale::force("ru");
    let mut request = ReportRequest::new(Period::Today, false);
    request.window = Some((0, 1));
    let binance =
        moon_core::telegram::report::ReportScope::Venue(moon_core::feed::ExchangeId::new(2));
    let bybit =
        moon_core::telegram::report::ReportScope::Venue(moon_core::feed::ExchangeId::new(6));
    let page = Page {
        request: request.clone(),
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: QuoteBreakdown::default(),
        rows: Vec::new(),
        pages: 1,
        drilldowns: vec![("Binance".into(), binance), ("Bybit".into(), bybit)],
        scope_label: None,
    };
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = super::render::keyboard(&page)
    else {
        panic!("expected inline navigation")
    };
    assert_eq!(
        markup.inline_keyboard.len(),
        1,
        "today has no extra view row"
    );
    assert_eq!(markup.inline_keyboard[0].len(), 2);
    assert!(
        markup.inline_keyboard[0][0].text.starts_with(&format!(
            "{} \u{25be}",
            rust_i18n::t!("telegram.report_exchanges_menu")
        )),
        "{}",
        markup.inline_keyboard[0][0].text
    );
    let opened = ReportRequest::parse_callback(
        markup.inline_keyboard[0][0]
            .callback_data
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert!(opened.exchanges_open);
    assert_eq!(opened.period, Period::Today);
    let cores = ReportRequest::parse_callback(
        markup.inline_keyboard[0][1]
            .callback_data
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert!(!cores.by_exchange);
    assert!(!cores.daily);
    assert!(!cores.exchanges_open);

    let mut expanded = page;
    expanded.request.exchanges_open = true;
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = super::render::keyboard(&expanded)
    else {
        panic!("expected inline navigation")
    };
    assert_eq!(markup.inline_keyboard[0].len(), 2, "two exchanges per row");
    assert!(markup.inline_keyboard[0][0].text.contains("Binance"));
    let picked = ReportRequest::parse_callback(
        markup.inline_keyboard[0][0]
            .callback_data
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert!(
        picked.exchanges_open,
        "picking an exchange must keep the list open"
    );
    assert_eq!(picked.scope, binance);
    let back = markup
        .inline_keyboard
        .iter()
        .flatten()
        .find(|button| {
            button
                .text
                .contains(&rust_i18n::t!("telegram.report_exchanges_back").to_string())
        })
        .expect("expanded list has Back");
    let closed = ReportRequest::parse_callback(back.callback_data.as_deref().unwrap()).unwrap();
    assert!(!closed.exchanges_open);
    assert_eq!(closed.period, Period::Today);
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
        let moon_core::telegram::api::ReplyMarkup::Inline(markup) = super::render::keyboard(&page)
        else {
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

/// A core is one table row: its name keeps both ends, and the full name stays in the details.
///
/// Mutation: restore the full-width name row. The main table then has two rows per core, the
/// "every other line" layout.
#[test]
fn a_core_is_one_row_with_its_full_name_in_details() {
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = false;
    let name = "Sample Desk / ACCOUNT No 11 with a long server name";
    let page = Page {
        request,
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: QuoteBreakdown::default(),
        rows: vec![
            (name.into(), QuoteBreakdown::default()),
            ("Short".into(), QuoteBreakdown::default()),
        ],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
    };
    let Response::Rich { html, .. } = render(&page, crate::HostKind::Terminal, true) else {
        panic!("expected rich report")
    };
    let main = &html[..html.find("<details>").unwrap()];
    // Header, one row per core, the total.
    assert_eq!(main.matches("<tr>").count(), 4);
    assert!(!main.contains("colspan"));
    assert!(main.contains("<tr><td>Sample D…ong server name</td>"));
    assert!(main.contains("<tr><td>Short</td>"));
    assert!(html.contains(&format!("<td colspan=\"2\"><b>{name}</b>")));
}

/// Deletable Help must not own the persistent keyboard; accounting stays collapsed and escaped.
#[test]
fn help_keeps_persistent_navigation_on_a_separate_message() {
    let Response::Rich {
        html,
        keyboard,
        navigation,
    } = super::help("<UTC>", crate::HostKind::Terminal, true)
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

/// A station's bot answers with the terminal off: its Help must not tell the chat to keep a
/// terminal running, nor point it to terminal settings for the Mini App.
#[test]
fn station_help_does_not_ask_for_a_running_terminal() {
    let _locale = crate::test_locale::force("en");
    let text = |host| {
        let Response::Rich { html, .. } = super::help("UTC", host, true) else {
            panic!("expected rich help")
        };
        html
    };
    let terminal = text(crate::HostKind::Terminal);
    let station = text(crate::HostKind::Station);
    assert!(terminal.contains("Keep the terminal running"));
    assert!(!station.contains("Keep the terminal running"));
    assert!(!station.contains("terminal settings"));
    assert!(station.contains("around the clock"));
    assert!(terminal.contains("terminal history"));
    assert!(!station.contains("terminal history"));
    assert!(station.contains("the station's history"));
    assert!(!crate::labels::report_help(crate::HostKind::Station).contains("terminal"));
}

/// A renamed core is listed under its configured name; a core no longer configured keeps the
/// name its rows stored.
#[test]
fn chat_report_names_a_renamed_core_by_its_configured_name() {
    let _locale = crate::test_locale::force("en");
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER);
        INSERT INTO orders_rep VALUES (1,'core-a-old',1,150,7,100,0),(1,'core-a-older',2,160,1,100,0),(2,'core-b-gone',3,150,2,100,0);").unwrap();
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = false;
    let names = moon_core::db::CoreNames::from_pairs([(1, "core-a-renamed")]);
    let page = super::read_page_on(&conn, request, 100, 200, chrono_tz::UTC, &names, |rows| {
        rows.sort_by_key(|(id, _)| *id);
        (Default::default(), super::TelegramReportAccess::Owner)
    })
    .unwrap();
    let labels: Vec<&str> = page.rows.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(labels, ["core-a-renamed", "core-b-gone"]);
    let Response::Rich { html, .. } = render(&page, crate::HostKind::Terminal, true) else {
        panic!("expected report");
    };
    assert!(html.contains("core-a-renamed"));
    assert!(!html.contains("core-a-old"));
}

/// Mini App core rows and trades name a renamed core by its configured name on every trade.
#[test]
fn mini_app_names_a_renamed_core_by_its_configured_name() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, id INTEGER, newrecid INTEGER, coin TEXT, closedate INTEGER, buydate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER, emulator INTEGER);
        INSERT INTO orders_rep VALUES (1,'core-a-old',1,1,'BTC',150,120,7,100,0,0),(1,'core-a-older',2,2,'ETH',160,120,1,100,0,0),(2,'core-b-gone',3,3,'SOL',170,120,2,100,0,0);").unwrap();
    let names = moon_core::db::CoreNames::from_pairs([(1, "core-a-renamed")]);

    let report = super::read_mini_report_on(
        &conn,
        100,
        200,
        chrono_tz::UTC,
        &names,
        Default::default(),
        super::TelegramReportAccess::Owner,
        false,
        |rows| rows.sort_by_key(|(id, _)| *id),
    )
    .unwrap();
    let cores: Vec<&str> = report
        .by_core
        .iter()
        .map(|(_, name, _, _)| name.as_str())
        .collect();
    assert_eq!(cores, ["core-a-renamed", "core-b-gone"]);

    let trades = super::read_mini_trades_on(
        &conn,
        chrono_tz::UTC,
        super::TelegramReportAccess::Owner,
        names,
        10,
    )
    .unwrap();
    let mut named: Vec<(u64, &str)> = trades
        .iter()
        .map(|trade| (trade.core_uid, trade.core_name.as_str()))
        .collect();
    named.sort();
    assert_eq!(
        named,
        [
            (1, "core-a-renamed"),
            (1, "core-a-renamed"),
            (2, "core-b-gone")
        ]
    );
}

/// Substituting exit quantity for bought quantity inflates spot top-ups; a missing entry or
/// conversion must stay absent even though the closed trade remains visible.
#[test]
fn mini_app_trades_keep_safe_entry_inputs_separate_from_exit_quantity() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (
        core_uid INTEGER, core_name TEXT, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER,
        boughtq REAL, quantity REAL, buyprice REAL, sellprice REAL, sellreason TEXT,
        basecurrency INTEGER, profitbtc REAL, spentbtc REAL);
        INSERT INTO orders_rep VALUES
        (2,'Demo',1,'DEMO',100,150,2,9,250,260,'Sell Price',1,20,500),
        (2,'Demo',2,'DEMO',100,160,2,9,NULL,260,'Sell Price',1,20,500),
        (2,'Demo',3,'DEMO',100,170,2,9,250,260,'Sell Price',8,20,500);
        ATTACH ':memory:' AS valuation;
        CREATE TABLE valuation.trade_values (
            source_kind INTEGER, core_uid INTEGER, row_id INTEGER, algorithm_version INTEGER,
            closedate INTEGER, quote_ordinal INTEGER, profit_quote REAL, spent_quote REAL,
            rate_minute_utc INTEGER, rate_usdt REAL, profit_usdt REAL, spent_usdt REAL);
        CREATE TABLE valuation.rates (
            algorithm_version INTEGER, quote_ordinal INTEGER, minute_utc INTEGER,
            provider TEXT, symbol TEXT, orientation INTEGER, leg2_provider TEXT,
            leg2_symbol TEXT, leg2_orientation INTEGER, resolved_minute_utc INTEGER);",
    )
    .unwrap();
    let trades = super::read_mini_trades_on(
        &conn,
        chrono_tz::UTC,
        super::TelegramReportAccess::Owner,
        Default::default(),
        10,
    )
    .unwrap();
    assert_eq!(trades.len(), 3);
    let ordinary = trades.iter().find(|t| t.close_utc == 150).unwrap();
    assert_eq!(ordinary.bought_quantity, Some(2.0));
    assert_eq!(ordinary.quantity, 9.0);
    assert_eq!(ordinary.entry_volume_rate, Some(1.0));
    assert_eq!(ordinary.profit_usdt, Some(20.0));
    assert_eq!(
        trades
            .iter()
            .find(|t| t.close_utc == 160)
            .unwrap()
            .entry_volume_rate,
        None
    );
    assert_eq!(
        trades
            .iter()
            .find(|t| t.close_utc == 170)
            .unwrap()
            .entry_volume_rate,
        None
    );
}

/// A window ending now learns whether any closed row lies past its end, on the production query.
#[test]
fn a_window_ending_now_learns_whether_rows_lie_past_its_end() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, id INTEGER, newrecid INTEGER, coin TEXT, closedate INTEGER, buydate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER, emulator INTEGER);
        INSERT INTO orders_rep VALUES (1,'a',1,1,'BTC',150,120,7,100,0,0),(1,'a',2,2,'ETH',160,120,1,100,0,0),(2,'b',3,3,'SOL',170,120,2,100,0,0);").unwrap();
    let names = moon_core::db::CoreNames::default();
    let read = |to, ends_now| {
        super::read_mini_report_on(
            &conn,
            100,
            to,
            chrono_tz::UTC,
            &names,
            Default::default(),
            super::TelegramReportAccess::Owner,
            ends_now,
            |rows| rows.sort_by_key(|(id, _)| *id),
        )
        .unwrap()
        .rows_after_to
    };
    assert_eq!(
        read(160, true),
        Some(true),
        "the row closed at 170 lies past 160"
    );
    assert_eq!(
        read(170, true),
        Some(false),
        "the edge itself is inside the window"
    );
    assert_eq!(read(200, true), Some(false));
    assert_eq!(read(160, false), None, "a fixed window does not ask");
}

/// Notification reads page past the Mini App cap and keep both rows that share a close time.
#[test]
fn closed_reads_page_past_the_ui_limit_and_keep_equal_close_times() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, core_name TEXT, newrecid INTEGER, coin TEXT,
            buydate INTEGER, closedate INTEGER, channelname TEXT, emulator INTEGER
        )",
    )
    .unwrap();
    let mut insert = conn
        .prepare(
            "INSERT INTO orders_rep
             (core_uid, core_name, newrecid, coin, buydate, closedate, channelname, emulator)
             VALUES (1, 'CoreA', ?1, 'BTC', ?2, ?3, '', 0)",
        )
        .unwrap();
    for close in 1000_i64..=1059 {
        insert
            .execute(rusqlite::params![close, close - 10, close])
            .unwrap();
    }
    insert
        .execute(rusqlite::params![500_i64, 490_i64, 500_i64])
        .unwrap();
    insert
        .execute(rusqlite::params![7001_i64, 1990_i64, 2000_i64])
        .unwrap();
    insert
        .execute(rusqlite::params![7002_i64, 1990_i64, 2000_i64])
        .unwrap();
    insert
        .execute(rusqlite::params![90000_i64, 89990_i64, 90000_i64])
        .unwrap();
    drop(insert);

    let names = moon_core::db::CoreNames::default();
    let since = super::read_closed_since_on(&conn, chrono_tz::UTC, &names, 1000).unwrap();
    let since_ids: std::collections::BTreeSet<i64> =
        since.iter().map(|trade| trade.rec_id).collect();
    let mut expected_since = std::collections::BTreeSet::new();
    for rec in 1000_i64..=1059 {
        expected_since.insert(rec);
    }
    expected_since.insert(7001);
    expected_since.insert(7002);
    expected_since.insert(90000);
    assert_eq!(since_ids, expected_since);
    assert_eq!(
        since.len(),
        expected_since.len(),
        "paging must not repeat a row"
    );
    assert!(since.len() > super::MINI_TRADES_LIMIT);
    assert!(since.len() > super::NOTIFY_READ_PAGE);
    assert_eq!(
        since
            .iter()
            .find(|trade| trade.rec_id == 1000)
            .unwrap()
            .open_utc,
        990,
        "a missing core offset leaves the stored buy time unchanged"
    );

    let day = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
    let next = chrono::NaiveDate::from_ymd_opt(1970, 1, 2).unwrap();
    let first = super::read_day_on(&conn, chrono_tz::UTC, &names, day).unwrap();
    let second = super::read_day_on(&conn, chrono_tz::UTC, &names, next).unwrap();
    let first_ids: std::collections::BTreeSet<i64> =
        first.iter().map(|trade| trade.rec_id).collect();
    let second_ids: std::collections::BTreeSet<i64> =
        second.iter().map(|trade| trade.rec_id).collect();
    let mut expected_day = expected_since.clone();
    expected_day.remove(&90000);
    expected_day.insert(500);
    assert_eq!(first_ids, expected_day);
    assert_eq!(first.len(), expected_day.len());
    assert_eq!(second_ids, std::collections::BTreeSet::from([90000]));
}
