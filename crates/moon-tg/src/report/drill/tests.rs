//! Synthetic exchange fixtures pin the approved drill-down and navigation states.

use super::*;
use moon_core::config::{
    ReportLayout, telegram_access::TelegramReportAccess, telegram_menu::ReportBasis,
};
use moon_core::telegram::{
    api::ReplyMarkup,
    report::{Period, ReportRequest, ReportScope},
};

/// Build a scoped report with reversed input profit order, idle history and an unauthorized venue.
fn fixture(active: u64, all: bool, page: usize) -> Page {
    fixture_with_groups(active, all, page, None)
}

/// Allow a dense overlapping-group roster to exercise the real message-length probes.
fn fixture_with_groups(
    active: u64,
    all: bool,
    page: usize,
    groups_override: Option<&[CoreGroup]>,
) -> Page {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (
        core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER,
        profitbtc REAL, spentbtc REAL, basecurrency INTEGER, sellreason TEXT);
        INSERT INTO orders_rep VALUES (99,'CORE_PRIVATE',1,150,999,100,1,'Sell'),
        (90,'CORE_IDLE',1,50,500,100,1,'Sell');",
    )
    .unwrap();
    for id in (1..=active).rev() {
        conn.execute(
            "INSERT INTO orders_rep VALUES (?1,?2,1,150,?3,100,1,'Sell')",
            rusqlite::params![
                id as i64,
                format!("EX-XX / CORE_A_{id:02}"),
                id as f64 - 7.0
            ],
        )
        .unwrap();
    }
    let mut request = ReportRequest::new(Period::Today, false);
    request.scope = ReportScope::Venue(moon_core::feed::ExchangeId::new(2));
    request.by_exchange = !all;
    request.page = page;
    let groups = [
        CoreGroup {
            name: "GROUP_A".into(),
            cores: vec![1, 2, 3],
        },
        CoreGroup {
            name: "GROUP_B".into(),
            cores: vec![2, 3, 4],
        },
    ];
    super::super::read_page_with(
        &conn,
        request,
        100,
        200,
        chrono_tz::UTC,
        ReportBasis::Close,
        ReportLayout::default(),
        &Default::default(),
        groups_override.unwrap_or(&groups),
        |_| {
            let mut venues = std::collections::HashMap::new();
            for id in (1..=active).chain([90, 91, 99]) {
                venues.insert(
                    id,
                    moon_core::venue::CoreVenue::identify(if id == 99 { 6 } else { 2 }, "", None),
                );
            }
            (
                venues,
                TelegramReportAccess::Viewer((1..=active).chain([90, 91]).collect()),
            )
        },
    )
    .unwrap()
}

/// Cutting a shared word or removing a complete name would make core identities ambiguous.
#[test]
fn strips_only_a_shared_separator_and_never_empties_a_name() {
    assert_eq!(
        shared_prefix(&["EX-XX / CORE_A", "EX-XX / CORE_B"]),
        "EX-XX / "
    );
    assert_eq!(
        shared_prefix(&["EX-XX ~ CORE_A", "EX-XX ~ CORE_B"]),
        "EX-XX ~ "
    );
    assert_eq!(shared_prefix(&["EX-XX CORE_A", "EX-XX CORE_B"]), "EX-XX ");
    assert_eq!(shared_prefix(&["CORE_A", "CORE_B"]), "");
    assert_eq!(shared_prefix(&["EX-XX", "EX-XX CORE_A"]), "");
    assert_eq!(shared_prefix(&["CORE_AA", "CORE_AB"]), "");
    assert_eq!(shared_prefix(&["CORE_A"]), "");
    assert_eq!(shared_prefix(&["EX-XX / ", "EX-XX / CORE_A"]), "");
    assert_eq!(shared_prefix(&["EX-XX ~ ", "EX-XX ~ CORE_A"]), "");
    assert_eq!(shared_prefix(&["EX-XX / \u{7}", "EX-XX / CORE_A"]), "");
    assert_eq!(shared_prefix(&["EX-XX / ~ ", "EX-XX / CORE_A"]), "");
    assert_eq!(
        shared_prefix(&["Ядро / CORE_A", "Ядро / CORE_B"]),
        "Ядро / "
    );
}

/// Borrowing the legacy whole-scope spend would include excluded Funding or invalid entry rows.
#[test]
fn dollar_average_uses_only_the_completely_valued_counted_entries() {
    let mut total = db::QuoteBreakdown::from_groups([(Some(0), 0.2, 3)]);
    total.entry_spend = db::EntrySpend {
        counted_orders: 2,
        valued_orders: 2,
        usdt_spent: 80.0 + 120.0,
        usdt_profit: 10.0,
        ..Default::default()
    };
    total.valuation = Some(db::ValuationCoverage {
        eligible_orders: 3,
        valued_orders: 3,
        unavailable_orders: 0,
        usdt: Some(db::UsdtTotal {
            profit: 20.0,
            spent: Some(9999.0),
        }),
    });
    assert_eq!(average_order_usdt(&total), Some(100.0));
    total.entry_spend.valued_orders = 1;
    assert_eq!(
        average_order_usdt(&total),
        None,
        "partial dollar conversion stays unavailable"
    );
}

/// The hidden-prefix caption is user text too; escaping only the shortened rows admits markup.
#[test]
fn hidden_prefix_captions_escape_markup_and_remove_control_characters() {
    let _locale = crate::test_locale::force("en");
    let mut page = fixture(2, false, 0);
    let exchange = page.exchange.as_mut().unwrap();
    for (row, suffix) in exchange
        .best
        .iter_mut()
        .chain(&mut exchange.worst)
        .zip(["CORE_A", "CORE_B"])
    {
        if let Row::Line(_, name, _) = row {
            *name = format!("<b>&\u{7} EX-XX / {suffix}");
        }
    }
    let html = report_html(&page);
    assert!(html.contains("shared name prefix hidden: &lt;b&gt;&amp; EX-XX /"));
    assert!(!html.contains('\u{7}'));
    assert!(!html.contains("<b>&"));
}

/// Removing the activity filter, venue grant or group overlap handling changes these independent sums.
#[test]
fn summary_matches_the_mockup_sections_and_keeps_totals_independent_of_overlapping_groups() {
    let _locale = crate::test_locale::force("ru");
    let page = fixture(12, false, 0);
    assert_eq!(page.total.orders, 12);
    assert_eq!(page.total.totals[0].profit, -6.0);
    let exchange = page.exchange.as_ref().unwrap();
    assert_eq!((exchange.cores, exchange.active), (14, 12));
    assert!((exchange.winrate - 5.0 / 12.0 * 100.0).abs() < 1e-9);
    assert_eq!(page.rows[0].total().totals[0].profit, -15.0);
    assert_eq!(page.rows[1].total().totals[0].profit, -12.0);
    let html = report_html(&page);
    for text in [
        "Итого по бирже",
        "14 ядер · со сделками 12 · winrate 42% · ср. ордер 100$",
        "По группам",
        "Лучшие ядра",
        "Худшие ядра",
        "общая часть имён скрыта: EX-XX /",
        "CORE_A_12",
        "CORE_A_01",
    ] {
        assert!(html.contains(text), "{text}: {html}");
    }
    assert_eq!(
        html.matches("<table").count(),
        1,
        "stacked sections share numeric widths"
    );
    assert_eq!(html.matches("общая часть имён скрыта").count(), 1);
    assert!(!html.contains("CORE_PRIVATE"));
    assert!(!html.contains("CORE_IDLE"));
    let ReplyMarkup::Inline(keyboard) = render::keyboard(&page) else {
        panic!("inline")
    };
    assert_eq!(
        keyboard.inline_keyboard[0][0].text,
        "Все ядра биржи · 12 со сделками"
    );
    assert_eq!(keyboard.inline_keyboard[1][0].text, "← Назад");
    assert_eq!(keyboard.inline_keyboard[1][1].text, "Другая биржа ▾");
    for button in keyboard.inline_keyboard.iter().flatten() {
        let data = button.callback_data.as_deref().unwrap();
        assert!(data.len() <= 64);
        let next = ReportRequest::parse_callback(data).unwrap();
        assert_eq!(next.window, Some((100, 200)));
        assert!(!next.select_view, "one-off drill-down never saves a mode");
    }
}

/// A naive pair of take(3) calls lists the same core twice when fewer than six traded.
#[test]
fn smaller_rankings_are_disjoint_and_keep_the_real_extremes() {
    for count in 0..=7 {
        let page = fixture(count, false, 0);
        let exchange = page.exchange.unwrap();
        let names: Vec<_> = exchange
            .best
            .iter()
            .chain(&exchange.worst)
            .map(name)
            .collect();
        let unique: std::collections::HashSet<_> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len());
        assert_eq!(names.len(), (count as usize).min(6));
        if count > 0 {
            assert_eq!(
                name(&exchange.best[0]),
                format!("EX-XX / CORE_A_{count:02}")
            );
        }
        if count > 1 {
            assert_eq!(name(&exchange.worst[0]), "EX-XX / CORE_A_01");
        }
    }
}

/// Sorting after paging or retaining idle rows would skip results or change the worst-first order.
#[test]
fn exchange_core_pages_have_ten_activity_only_rows_and_return_to_the_summary() {
    let _locale = crate::test_locale::force("ru");
    let first = fixture(12, true, 0);
    let second = fixture(12, true, 1);
    assert_eq!(
        (first.pages, first.rows.len(), second.rows.len()),
        (2, 10, 2)
    );
    assert_eq!(name(&first.rows[0]), "EX-XX / CORE_A_01");
    assert_eq!(name(&first.rows[9]), "EX-XX / CORE_A_10");
    assert_eq!(name(&second.rows[0]), "EX-XX / CORE_A_11");
    assert_eq!(name(&second.rows[1]), "EX-XX / CORE_A_12");
    for page in [&first, &second] {
        let html = report_html(page);
        assert!(rich_message_fits(&html));
        assert!(html.contains("от худшего к лучшему · ядра без сделок не показаны"));
        let ReplyMarkup::Inline(keyboard) = render::keyboard(page) else {
            panic!("inline")
        };
        let back = keyboard.inline_keyboard.last().unwrap().first().unwrap();
        assert_eq!(back.text, "← Назад к бирже");
        let next = ReportRequest::parse_callback(back.callback_data.as_deref().unwrap()).unwrap();
        assert!(next.by_exchange);
        assert_eq!(next.scope, page.request.scope);
        assert!(!next.select_view);
    }
    let ReplyMarkup::Inline(keyboard) = render::keyboard(&first) else {
        panic!("inline")
    };
    assert_eq!(keyboard.inline_keyboard[0][0].text, "Стр. 2 ›");
    let ReplyMarkup::Inline(keyboard) = render::keyboard(&second) else {
        panic!("inline")
    };
    assert_eq!(keyboard.inline_keyboard[0][0].text, "‹ Стр. 1");
}

/// Sizing only the core roster would let a dense saved-group summary exceed Telegram's caps.
#[test]
fn dense_exchange_summaries_page_without_losing_groups() {
    let groups: Vec<_> = (0..100)
        .map(|index| CoreGroup {
            name: format!("GROUP_{index:03}_{}", "&".repeat(200)),
            cores: vec![1],
        })
        .collect();
    let first = fixture_with_groups(1, false, 0, Some(&groups));
    assert!(first.pages > 1);
    let mut shown = 0;
    for index in 0..first.pages {
        let page = fixture_with_groups(1, false, index, Some(&groups));
        assert!(rich_message_fits(&report_html(&page)));
        shown += page.rows.len();
        assert_eq!(
            page.total.orders, 1,
            "group overlap never changes the exchange total"
        );
    }
    assert_eq!(shown, 100);
}

/// Maximum-size periods and venue identities must stay below the Bot API's callback-data cap.
#[test]
fn every_drill_action_preserves_the_maximum_callback_contract() {
    use moon_core::config::telegram_menu::ReportBasis;
    for all in [false, true] {
        let mut page = fixture(12, all, 1);
        page.request.period = ReportRequest::dates("2026-01-01", "2026-12-31")
            .unwrap()
            .period;
        // Clear the fixture window before resolving the calendar period.
        page.request.window = None;
        page.request.window = page.request.bounds(0, chrono_tz::UTC);
        page.request.scope = ReportScope::Venue(moon_core::feed::ExchangeId {
            code: 255,
            dex: u32::MAX,
        });
        page.request.basis = Some(ReportBasis::Open);
        let ReplyMarkup::Inline(markup) = render::keyboard(&page) else {
            panic!("inline")
        };
        for button in markup.inline_keyboard.iter().flatten() {
            let data = button.callback_data.as_deref().unwrap();
            assert!(data.len() <= 64, "{data}");
            let next = ReportRequest::parse_callback(data).unwrap();
            assert_eq!(next.window, page.request.window);
            assert_eq!(next.basis, Some(ReportBasis::Open));
            assert_eq!(next.period, page.request.period);
        }
    }
}
