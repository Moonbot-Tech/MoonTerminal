//! Money presentation must stay honest when valuation or currency identity is incomplete.
use super::Page;
use super::render::{profit, render};
use moon_core::config::telegram_menu::ReportBasis;

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
            ReportBasis::Close,
            &Default::default(),
            |_| (venues.clone(), super::TelegramReportAccess::Viewer(vec![1])),
        )
        .unwrap();
        assert_eq!(page.total.orders, 1);
        assert_eq!(page.total.totals[0].profit, 7.0);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].total().totals[0].profit, 7.0);
        let Response::Rich { html, .. } = render(
            &page,
            crate::HostKind::Terminal,
            nav(crate::HostKind::Terminal, true),
        ) else {
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
            ReportBasis::Close,
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

/// A paged report shows the whole-scope total as its table's top row, never as a paragraph above
/// the table or as the page's own sum.
#[test]
fn full_total_is_the_tables_top_row() {
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
        rows: vec![super::Row::Line(
            "Visible row".into(),
            QuoteBreakdown::from_groups([(Some(0), 1.0, 1)]),
        )],
        pages: 2,
        drilldowns: Vec::new(),
        scope_label: None,
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
    };
    let Response::Rich { html, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
        panic!("expected report")
    };
    let table_start = html.find("<table").unwrap();
    let table_end = html.find("</table>").unwrap();
    let total_at = html.find("+123.45").unwrap();
    assert!(total_at > table_start && total_at < html.find("Visible row").unwrap());
    assert!(!html[..table_start].contains("+123.45"));
    assert!(html[table_start..table_end].contains("<th align=\"right\">12</th>"));
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
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
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
        rows: vec![super::Row::Line("Fixture".into(), total)],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
    };
    let Response::Rich { html, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
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
        ReportBasis::Close,
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
        page.rows.iter().map(|row| row.name()).collect::<Vec<_>>(),
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
        ReportBasis::Close,
        &Default::default(),
        |_| (venues.clone(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert_eq!(page.total.orders, 3);
    assert_eq!(page.rows.len(), 2);
    let zero = page
        .rows
        .iter()
        .find(|row| row.total().orders == 2)
        .unwrap();
    assert_eq!(zero.total().totals[0].profit, 0.0);
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
        ReportBasis::Close,
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
        ReportBasis::Close,
        &Default::default(),
        |_| (Default::default(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert_eq!(page.rows.last().unwrap().name(), "2025-01-01");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.total.orders, 1);
    assert_eq!(page.rows.last().unwrap().total().orders, 1);
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
        ReportBasis::Close,
        &Default::default(),
        |_| (Default::default(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert_eq!(page.rows.len(), 31);
    assert_eq!(page.pages, 1);
    let Response::Rich { html, keyboard, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
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
        ReportBasis::Close,
        &Default::default(),
        |_| (venues.clone(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert_eq!(page.rows.len(), 6);
    assert_eq!(page.pages, 1);
    let Response::Rich { html, keyboard, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
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
        ReportBasis::Close,
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
    let Response::Rich { html, keyboard, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
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
            ReportBasis::Close,
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
        rows: vec![super::Row::Line("Fixture".into(), total)],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
    };
    let Response::Rich { html, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
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
        ReportBasis::Close,
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
    assert_eq!(first.rows[0].total().orders, 2);
    assert_eq!(first.rows[0].total().totals[0].profit, 7.0);
    request.page = 1;
    let second = super::read_page_on(
        &conn,
        request,
        100,
        200,
        chrono_tz::UTC,
        ReportBasis::Close,
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
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
    };
    for locale in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let labels = crate::labels::telegram_labels(crate::HostKind::Station);
        for owner in [false, true] {
            for response in [
                render(
                    &page,
                    crate::HostKind::Station,
                    nav(crate::HostKind::Station, owner),
                ),
                super::help(
                    "UTC",
                    crate::HostKind::Station,
                    owner,
                    nav(crate::HostKind::Station, owner),
                ),
            ] {
                let Response::Rich { navigation, .. } = response else {
                    panic!("expected rich response")
                };
                let moon_core::telegram::api::ReplyMarkup::Reply(markup) = navigation.1 else {
                    panic!("expected the reply keyboard")
                };
                assert_eq!(
                    markup.keyboard.iter().flatten().any(|button| {
                        moon_core::telegram::commands::parse_reply_button(&button.text, &labels)
                            == moon_core::telegram::commands::ParsedCommand::StationStatus
                    }),
                    owner
                );
            }
            let Response::Rich { html, .. } = super::help(
                "UTC",
                crate::HostKind::Station,
                owner,
                nav(crate::HostKind::Station, owner),
            ) else {
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
        rows: vec![super::Row::Line(
            format!("<b>&{}", "x".repeat(50_000)),
            total,
        )],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
    };
    let Response::Rich { html, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
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
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
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
            basis: ReportBasis::Close,
            cores: Vec::new(),
            caption: None,
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
            super::Row::Line(name.into(), QuoteBreakdown::default()),
            super::Row::Line("Short".into(), QuoteBreakdown::default()),
        ],
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
    };
    let Response::Rich { html, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
        panic!("expected rich report")
    };
    let main = &html[..html.find("<details>").unwrap()];
    // The totals on top, then one row per core.
    assert_eq!(main.matches("<tr>").count(), 3);
    assert!(!main.contains("colspan"));
    assert!(main.contains("<tr><td>Sample D…ong server name</td>"));
    assert!(main.contains("<tr><td>Short</td>"));
    assert!(html.contains(&format!("<td colspan=\"2\"><b>{name}</b>")));
}

/// Deletable Help must not own the reply keyboard; accounting stays collapsed and escaped.
#[test]
fn help_keeps_the_reply_keyboard_on_a_separate_message() {
    let Response::Rich {
        html,
        keyboard,
        navigation,
    } = super::help(
        "<UTC>",
        crate::HostKind::Terminal,
        true,
        nav(crate::HostKind::Terminal, true),
    )
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
        let Response::Rich { html, .. } = super::help("UTC", host, true, nav(host, true)) else {
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
    let page = super::read_page_on(
        &conn,
        request,
        100,
        200,
        chrono_tz::UTC,
        ReportBasis::Close,
        &names,
        |rows| {
            rows.sort_by_key(|(id, _)| *id);
            (Default::default(), super::TelegramReportAccess::Owner)
        },
    )
    .unwrap();
    let labels: Vec<&str> = page.rows.iter().map(|row| row.name()).collect();
    assert_eq!(labels, ["core-a-renamed", "core-b-gone"]);
    let Response::Rich { html, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
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
            buydate INTEGER, closedate INTEGER, channelname TEXT, emulator INTEGER,
            buyprice REAL, sellprice REAL
        )",
    )
    .unwrap();
    let mut insert = conn
        .prepare(
            "INSERT INTO orders_rep
             (core_uid, core_name, newrecid, coin, buydate, closedate, channelname, emulator,
              buyprice, sellprice)
             VALUES (1, 'CoreA', ?1, 'BTC', ?2, ?3, '', 0, 0.10739, 0.10844)",
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
    let first = since.iter().find(|trade| trade.rec_id == 1000).unwrap();
    assert_eq!(
        first.close_utc, 1000,
        "a missing core offset leaves the stored close time unchanged"
    );
    assert_eq!(
        (first.buy_price, first.sell_price),
        (Some(0.10739), Some(0.10844)),
        "the card's prices come off the row"
    );
}

/// The keyboard a chat of `owner` gets from a bot with the default menu.
fn nav(host: crate::HostKind, owner: bool) -> moon_core::telegram::api::ReplyMarkup {
    crate::labels::navigation_keyboard(host, owner, &moon_core::config::TelegramConfig::default())
}

/// The bot's period basis reaches the query: by open time a trade counts in the period it opened
/// in, and the report says so.
#[test]
fn the_open_basis_counts_trades_by_when_they_opened() {
    let _locale = crate::test_locale::force("en");
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER,core_name TEXT,newrecid INTEGER,buydate INTEGER,closedate INTEGER,profitbtc REAL,spentbtc REAL,basecurrency INTEGER);
        INSERT INTO orders_rep VALUES (1,'Core',1,50,150,7,100,0),(1,'Core',2,120,300,5,100,0);").unwrap();
    let read = |basis| {
        super::read_page_on(
            &conn,
            ReportRequest::new(Period::Today, false),
            100,
            200,
            chrono_tz::UTC,
            basis,
            &Default::default(),
            |_| (Default::default(), super::TelegramReportAccess::Owner),
        )
        .unwrap()
    };
    let close = read(ReportBasis::Close);
    assert_eq!(close.total.orders, 1);
    assert_eq!(close.total.totals[0].profit, 7.0);
    let open = read(ReportBasis::Open);
    assert_eq!(
        open.request.basis,
        Some(ReportBasis::Open),
        "its buttons keep the basis"
    );
    assert_eq!(open.total.orders, 1);
    assert_eq!(open.total.totals[0].profit, 5.0);
    let caption = rust_i18n::t!("report.period_basis.open").to_string();
    let html = |page: &Page| super::render::report_html(page);
    assert!(html(&open).contains(&caption));
    assert!(!html(&close).contains(&caption));
}

/// An automatic report is the button's report over the slot's frozen period, its title and zone
/// heading the table in one line, its buttons carrying the window, the cores it may disclose; a
/// viewer with no cores gets none.
#[test]
fn an_auto_report_reads_the_frozen_slot_with_its_caption() {
    use moon_core::config::AppConfig;
    use moon_core::telegram::report::AutoWindow;
    let _locale = crate::test_locale::force("en");
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER,core_name TEXT,newrecid INTEGER,buydate INTEGER,closedate INTEGER,profitbtc REAL,spentbtc REAL,basecurrency INTEGER);
        INSERT INTO orders_rep VALUES (1,'Core',1,50,150,7,100,0),(1,'Core',2,120,3700,5,100,0);").unwrap();
    let window = AutoWindow {
        at: 3600,
        from: 0,
        to: 3599,
        period: Period::Hour,
    };
    let inputs = super::AutoInputs {
        zone: chrono_tz::UTC,
        basis: ReportBasis::Close,
        view: moon_core::config::telegram_menu::ReportView::Cores,
        order: moon_core::session::core_order::CoreOrder::new(&AppConfig::headless(Vec::new())),
        names: Default::default(),
        venues: Default::default(),
        groups: Vec::new(),
    };
    let caption = |title: &str| super::AutoCaption {
        title: title.into(),
        zone: "UTC".into(),
    };
    let page = super::read_auto_report(
        &conn,
        &window,
        caption("Hourly report"),
        &inputs,
        &super::TelegramReportAccess::Owner,
    )
    .unwrap()
    .expect("an owner always gets a report");
    assert!(
        page.html.starts_with(
            "<table compact><caption><b>Hourly report</b> · 01.01.1970 00:00—00:59 UTC</caption>"
        ),
        "{}",
        page.html
    );
    assert_eq!(page.cores, None, "an owner's report names no core");
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = &page.keyboard else {
        panic!("inline buttons")
    };
    let callbacks: Vec<&str> = markup
        .inline_keyboard
        .iter()
        .flatten()
        .filter_map(|button| button.callback_data.as_deref())
        .collect();
    assert!(!callbacks.is_empty());
    for data in callbacks {
        let request = ReportRequest::parse_callback(data).expect(data);
        assert_eq!(request.window, Some((0, 3599)), "{data}");
    }
    let none = super::read_auto_report(
        &conn,
        &window,
        caption(""),
        &inputs,
        &super::TelegramReportAccess::Viewer(Vec::new()),
    )
    .unwrap();
    assert!(none.is_none());
}

/// A card prints the trade's own money the moment its row lands, so the read must hand back the
/// settled profit, the entry notional and their currency without any valuation; a row the
/// Report's volume gates cannot prove (no close reason) keeps its profit and loses its volume.
#[test]
fn closed_trades_carry_their_own_money() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, core_name TEXT, newrecid INTEGER, coin TEXT, fname TEXT,
            buydate INTEGER, closedate INTEGER, channelname TEXT, emulator INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER, boughtq REAL, buyprice REAL,
            sellprice REAL, sellreason TEXT
        );
        INSERT INTO orders_rep VALUES
            (1, 'CoreA', 1, 'ACE', 'ACEUSDC', 990, 1000, '', 0, 3.3, 150.0, 8, 10.0, 15.0, 15.33,
             'TakeProfit'),
            (1, 'CoreA', 2, 'ETH', 'ETHBTC', 990, 1001, '', 0, 0.00012, 0.01, 0, 0.5, 0.02, 0.0202,
             'TakeProfit'),
            (1, 'CoreA', 3, 'ACE', 'ACEUSDC', 990, 1002, '', 0, -1.0, 150.0, 8, 10.0, 15.0, 14.9,
             NULL);",
    )
    .unwrap();
    let names = moon_core::db::CoreNames::default();
    let rows = super::read_closed_since_on(&conn, chrono_tz::UTC, &names, 0).unwrap();
    assert_eq!(rows.len(), 3);
    let usdc = &rows[0];
    assert_eq!(usdc.quote.map(|q| q.ticker()), Some("USDC"));
    assert_eq!(usdc.profit_native, Some(3.3));
    assert_eq!(usdc.volume_native, Some(150.0));
    let btc = &rows[1];
    assert_eq!(btc.quote.map(|q| q.ticker()), Some("BTC"));
    assert_eq!(btc.profit_native, Some(0.00012));
    assert!((btc.volume_native.unwrap() - 0.01).abs() < 1e-12);
    let unproven = &rows[2];
    assert_eq!(unproven.profit_native, Some(-1.0));
    assert_eq!(unproven.volume_native, None);
}

/// A report fits a phone screen: the view and its period are the table's caption, a single day
/// names its date once, and its buttons are one row in either view.
#[test]
fn a_days_report_is_compact() {
    let _locale = crate::test_locale::force("en");
    for by_exchange in [false, true] {
        let mut request = ReportRequest::new(Period::Today, false);
        request.by_exchange = by_exchange;
        let page = Page {
            request,
            from: 0,
            to: 8 * 3600 + 59 * 60,
            zone: chrono_tz::UTC,
            total: QuoteBreakdown::default(),
            rows: vec![super::Row::Line("Core".into(), QuoteBreakdown::default())],
            pages: 1,
            drilldowns: Vec::new(),
            scope_label: None,
            basis: ReportBasis::Close,
            cores: Vec::new(),
            caption: None,
        };
        let html = super::render::report_html(&page);
        let table = html.find("<table").unwrap();
        assert!(
            html[table..].contains("01.01.1970 00:00—08:59</caption>"),
            "{html}"
        );
        assert!(!html[..table].contains("01.01.1970"), "{html}");
        let moon_core::telegram::api::ReplyMarkup::Inline(markup) = super::render::keyboard(&page)
        else {
            panic!("expected inline navigation")
        };
        assert_eq!(
            markup.inline_keyboard.len(),
            1,
            "{:?}",
            markup.inline_keyboard
        );
    }
}

/// A funding payment is a closed report row, not a trade: the chat gets no card for it.
#[test]
fn funding_is_not_a_trade_card() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, core_name TEXT, newrecid INTEGER, coin TEXT, fname TEXT,
            buydate INTEGER, closedate INTEGER, channelname TEXT, emulator INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER, boughtq REAL, buyprice REAL,
            sellprice REAL, sellreason TEXT
        );
        INSERT INTO orders_rep VALUES
            (1, 'CoreA', 1, 'ACE', 'ACEUSDT', 990, 1000, '', 0, -0.02, 0.0, 1, 0.0, 0.0, 0.0,
             'Funding'),
            (1, 'CoreA', 2, 'ACE', 'ACEUSDT', 990, 1001, '', 0, 3.3, 150.0, 1, 10.0, 15.0, 15.33,
             'TakeProfit');",
    )
    .unwrap();
    let names = moon_core::db::CoreNames::default();
    let rows = super::read_closed_since_on(&conn, chrono_tz::UTC, &names, 0).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.rec_id).collect::<Vec<_>>(),
        vec![2],
        "only the trade becomes a card"
    );
}

/// The view by cores lists its cores under the saved groups, as the Profit monitor does: groups by
/// name, a core in two groups under both, the cores in none last. A group is one header row
/// carrying its total, the database's sum over its cores (a group of one core totals that core);
/// the headline counts every core once. The details name each core once.
#[test]
fn cores_are_listed_under_their_groups() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER,core_name TEXT,newrecid INTEGER,closedate INTEGER,profitbtc REAL,spentbtc REAL,basecurrency INTEGER);
        INSERT INTO orders_rep VALUES (1,'A',1,150,10,100,0),(2,'B',1,150,20,100,0),(3,'C',1,150,40,100,0);").unwrap();
    let groups = vec![
        moon_core::config::CoreGroup {
            name: "margo".into(),
            cores: vec![2],
        },
        moon_core::config::CoreGroup {
            name: "main".into(),
            cores: vec![2, 1],
        },
    ];
    let read = |request: ReportRequest| {
        super::read_page_with(
            &conn,
            request,
            100,
            200,
            chrono_tz::UTC,
            ReportBasis::Close,
            &Default::default(),
            &groups,
            |cores| {
                cores.sort_by_key(|(id, _)| *id);
                (Default::default(), super::TelegramReportAccess::Owner)
            },
        )
        .unwrap()
    };
    let _locale = crate::test_locale::force("en");
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = false;
    let page = read(request.clone());
    let rows: Vec<(String, f64)> = page
        .rows
        .iter()
        .map(|row| {
            let kind = match row {
                super::Row::Line(..) => "",
                super::Row::Repeat(..) => "~ ",
                super::Row::Group(..) => "# ",
            };
            (
                format!("{kind}{}", row.name()),
                row.total().totals[0].profit,
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("# main".to_string(), 30.0),
            ("A".to_string(), 10.0),
            ("B".to_string(), 20.0),
            ("# margo".to_string(), 20.0),
            ("~ B".to_string(), 20.0),
            ("# Ungrouped".to_string(), 40.0),
            ("C".to_string(), 40.0),
        ]
    );
    assert_eq!(page.total.totals[0].profit, 70.0, "each core counted once");
    let Response::Rich { html, keyboard, .. } = render(
        &page,
        crate::HostKind::Terminal,
        nav(crate::HostKind::Terminal, true),
    ) else {
        panic!("expected report");
    };
    assert!(html.contains("<tr><th align=\"right\">main</th>"), "{html}");
    assert!(html.contains("<tr><th align=\"left\">Total</th>"), "{html}");
    assert!(html.contains("<tr><td>A</td>"), "{html}");
    assert!(!html.contains("total</i>"), "no subtotal row: {html}");
    // Stripes would run across the group headers out of step with them.
    assert!(html.contains("<table compact>"), "{html}");
    let details = &html[html.find("<details>").unwrap()..];
    assert_eq!(details.matches("<b>B</b>").count(), 1, "{details}");
    let moon_core::telegram::api::ReplyMarkup::Inline(markup) = keyboard else {
        panic!("expected inline navigation")
    };
    assert!(
        markup.inline_keyboard.iter().flatten().all(|button| {
            !button
                .callback_data
                .as_deref()
                .unwrap_or_default()
                .starts_with("r:g")
        }),
        "no view by groups is offered"
    );
    // The view by exchanges and by days stay flat.
    request.by_exchange = true;
    assert!(
        read(request)
            .rows
            .iter()
            .all(|row| matches!(row, super::Row::Line(..)))
    );
    // One group holding every listed core puts one caption over everything: flat.
    let all = vec![moon_core::config::CoreGroup {
        name: "all".into(),
        cores: vec![1, 2, 3],
    }];
    let mut flat = ReportRequest::new(Period::Today, false);
    flat.by_exchange = false;
    let page = super::read_page_with(
        &conn,
        flat,
        100,
        200,
        chrono_tz::UTC,
        ReportBasis::Close,
        &Default::default(),
        &all,
        |_| (Default::default(), super::TelegramReportAccess::Owner),
    )
    .unwrap();
    assert_eq!(page.rows.len(), 3);
    assert!(
        page.rows
            .iter()
            .all(|row| matches!(row, super::Row::Line(..)))
    );
}

/// A page cut inside a group repeats the group's header, and a header that would end a page
/// without its cores moves to the next one.
#[test]
fn a_page_cut_inside_a_group_keeps_its_header() {
    use super::Row::{Group, Line};
    let total = QuoteBreakdown::default;
    let rows = vec![
        Group("a".into(), total()),
        Line("1".into(), total()),
        Line("2".into(), total()),
        Group("b".into(), total()),
        Line("3".into(), total()),
    ];
    let names = |page: Vec<super::Row>| {
        page.iter()
            .map(|row| row.name().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(super::page_rows(&rows, 0, 2)), ["a", "1"]);
    assert_eq!(names(super::page_rows(&rows, 1, 2)), ["a", "1", "2"]);
    // The page [2, b] would end on b's header; b opens the next page instead.
    assert_eq!(names(super::page_rows(&rows, 2, 2)), ["a", "2"]);
    assert_eq!(names(super::page_rows(&rows, 3, 2)), ["b", "3"]);
    assert_eq!(names(super::page_rows(&rows, 4, 2)), ["b", "3"]);
    assert_eq!(names(super::page_rows(&rows, 0, 5)), names(rows.clone()));
    let flat = vec![Line("x".into(), total()), Line("y".into(), total())];
    assert_eq!(names(super::page_rows(&flat, 1, 1)), ["y"]);
}

/// A core listed again on a page whose first listing of it is on another page is in that page's
/// details; listed twice on one page, it is there once.
#[test]
fn a_repeat_alone_on_its_page_keeps_its_details() {
    use super::Row::{Group, Line, Repeat};
    let _locale = crate::test_locale::force("en");
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = false;
    let page = |rows| Page {
        request: request.clone(),
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: QuoteBreakdown::default(),
        rows,
        pages: 2,
        drilldowns: Vec::new(),
        scope_label: None,
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
    };
    let details = |rows| {
        let html = super::report_html(&page(rows));
        html[html.find("<details>").unwrap()..].to_string()
    };
    let alone = details(vec![
        Group("margo".into(), QuoteBreakdown::default()),
        Repeat("Bcore".into(), QuoteBreakdown::default()),
    ]);
    assert_eq!(alone.matches("<b>Bcore</b>").count(), 1, "{alone}");
    let both = details(vec![
        Group("main".into(), QuoteBreakdown::default()),
        Line("Bcore".into(), QuoteBreakdown::default()),
        Group("margo".into(), QuoteBreakdown::default()),
        Repeat("Bcore".into(), QuoteBreakdown::default()),
    ]);
    assert_eq!(both.matches("<b>Bcore</b>").count(), 1, "{both}");
}

/// The period names as little as stays unambiguous: no year inside the current one, no clock on a
/// bound that falls on a day's edge, a whole day as its date alone.
#[test]
fn the_period_is_short() {
    let day = 86_400;
    // 05.10.2026 00:00 UTC.
    let midnight = 1_791_158_400;
    let page = |from: i64, to: i64| Page {
        request: ReportRequest::new(Period::Today, false),
        from,
        to,
        zone: chrono_tz::UTC,
        total: QuoteBreakdown::default(),
        rows: Vec::new(),
        pages: 1,
        drilldowns: Vec::new(),
        scope_label: None,
        basis: ReportBasis::Close,
        cores: Vec::new(),
        caption: None,
    };
    let period = |from, to, year| super::render::period(&page(from, to), year);
    assert_eq!(
        period(midnight, midnight + 6 * 3600 - 1, 2026),
        "05.10 00:00—05:59"
    );
    assert_eq!(period(midnight, midnight + day - 1, 2026), "05.10");
    // Cut 30 s before the day ends, it is not the whole day.
    assert_eq!(
        period(midnight, midnight + day - 31, 2026),
        "05.10 00:00—23:59"
    );
    assert_eq!(
        period(midnight - 4 * day, midnight + 6 * 3600 - 1, 2026),
        "01.10 — 05.10 05:59"
    );
    assert_eq!(
        period(midnight - 4 * day, midnight - 1, 2026),
        "01.10 — 04.10"
    );
    assert_eq!(
        period(midnight + 3600, midnight + day + 3599, 2026),
        "05.10 01:00 — 06.10 00:59"
    );
    // A period outside the current year keeps its year.
    assert_eq!(
        period(midnight, midnight + 6 * 3600 - 1, 2027),
        "05.10.2026 00:00—05:59"
    );
}
