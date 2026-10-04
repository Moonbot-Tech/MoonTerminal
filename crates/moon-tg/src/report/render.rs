//! The report's rich HTML, its inline keyboard, and Help.

use crate::t;
use moon_core::{
    config::telegram_menu::ReportBasis,
    db::QuoteBreakdown,
    telegram::{
        api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
        report::{ReportRequest, ReportScope},
        runtime::Response,
    },
    util::{display_time, fmt},
};

use super::{Page, Row};
use crate::HostKind;

/// Telegram `sendRichMessage` cap: 32768 UTF-8 characters in the rich message text.
const RICH_MESSAGE_CHAR_LIMIT: usize = 32_768;
/// Telegram `sendRichMessage` cap: 500 blocks, including nested blocks and table rows.
const RICH_MESSAGE_BLOCK_LIMIT: usize = 500;

/// Escape all external text before inserting it into Telegram's restricted rich HTML.
pub(crate) use crate::html::escape;

/// Render complete USDT only; missing or unknown valuation never masquerades as zero.
pub(super) fn profit(total: &QuoteBreakdown) -> String {
    if total.orders == 0 {
        return "0.00$".into();
    }
    total
        .unified_usdt()
        .and_then(|amount| fmt::signed_fixed(amount.profit, 2).map(|value| format!("{}$", value.0)))
        .unwrap_or_else(|| t!("telegram.report_unvalued").to_string())
}

/// Native currency subtotals preserve useful evidence even when conversion is incomplete.
pub(super) fn native(total: &QuoteBreakdown) -> String {
    total
        .totals
        .iter()
        .map(|part| {
            format!(
                "{} {}",
                fmt::signed_fixed(part.profit, part.currency.display_decimals())
                    .map(|value| value.0)
                    .unwrap_or_else(|| t!("telegram.report_unvalued").to_string()),
                part.currency.ticker()
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Telegram counts UTF-8 characters and nested blocks on the `sendRichMessage` path.
pub(crate) fn rich_message_fits(html: &str) -> bool {
    html.chars().count() <= RICH_MESSAGE_CHAR_LIMIT
        && rich_message_blocks(html) <= RICH_MESSAGE_BLOCK_LIMIT
}

/// Table rows, paragraphs, captions, details and tables are the blocks this report emits.
pub(super) fn rich_message_blocks(html: &str) -> usize {
    html.matches("<tr").count()
        + html.matches("<p>").count()
        + html.matches("<caption>").count()
        + html.matches("<details").count()
        + html.matches("<table").count()
}

/// Compose a compact headline, three-column table, and optional per-bot accounting details.
///
/// Made to fit a phone screen (LinKvo, 04.10): the view and its period are the table's caption
/// rather than two paragraphs, the totals are its top row rather than a heading row and a total
/// row, and a single day's period names its date once.
///
/// `host` words delivery failures; `navigation` is the chat's reply keyboard.
pub(super) fn render(page: &Page, host: HostKind, navigation: ReplyMarkup) -> Response {
    let html = report_html(page);
    if !rich_message_fits(&html) {
        return Response::Text {
            text: crate::labels::report_delivery_failed(host),
            keyboard: Some(keyboard(page)),
        };
    }
    Response::Rich {
        html,
        keyboard: keyboard(page),
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            navigation,
        ),
    }
}

/// The page's period: `04.10.2026 00:00 — 08:59` inside one day, both dates otherwise.
fn period(page: &Page) -> String {
    let at = |value| display_time::at(value, page.zone);
    match (at(page.from), at(page.to)) {
        (Some(from), Some(to)) if from.date_naive() == to.date_naive() => {
            format!("{} — {}", from.format("%d.%m.%Y %H:%M"), to.format("%H:%M"))
        }
        (from, to) => {
            let stamp = |value: Option<_>| {
                value
                    .map(|v: chrono::DateTime<chrono_tz::Tz>| {
                        v.format("%d.%m.%Y %H:%M").to_string()
                    })
                    .unwrap_or_default()
            };
            format!("{} — {}", stamp(from), stamp(to))
        }
    }
}

/// HTML for one report page; the caller decides whether it fits Telegram's rich-message caps.
pub(super) fn report_html(page: &Page) -> String {
    let heading = if page.request.daily {
        t!("telegram.report_days")
    } else if page.request.by_exchange {
        t!("telegram.report_exchanges")
    } else {
        t!("telegram.report_cores")
    };
    let title = page.scope_label.as_deref().unwrap_or(&heading);
    let mut html = page
        .caption
        .as_deref()
        .map(|caption| format!("<p><b>{}</b></p>", escape(caption)))
        .unwrap_or_default();
    if page.total.orders == 0 {
        html.push_str(&format!("<p>{}</p>", escape(&t!("telegram.report_empty"))));
    }
    let mut table_caption = if page.request.by_exchange && page.scope_label.is_none() {
        escape(&period(page))
    } else {
        format!("<b>{}</b> · {}", escape(title), escape(&period(page)))
    };
    // The terminal's Report wording: the period counts trades by when they opened.
    if page.basis == ReportBasis::Open {
        table_caption.push_str(&format!(
            " · <i>{}</i>",
            escape(&t!("report.period_basis.open"))
        ));
    }
    html.push_str(&format!(
        "<table compact striped><caption>{table_caption}</caption><tr><th>{}</th><th align=\"right\">{}</th><th align=\"right\">{}</th></tr>",
        escape(&t!("telegram.report_total")),
        escape(&profit(&page.total)),
        page.total.orders
    ));
    let by_core = !page.request.by_exchange && !page.request.daily;
    for row in &page.rows {
        match row {
            // Long user-controlled names cannot exhaust the rich-message budget. A core is one
            // row: its name keeps both ends (where the account number usually is), and the full
            // name stays in the details below.
            Row::Line(name, total) | Row::Repeat(name, total) => {
                let label: String = if by_core {
                    compact_label(name)
                } else {
                    plain_label(name)
                };
                html.push_str(&format!(
                    "<tr><td>{}</td><td align=\"right\">{}</td><td align=\"right\">{}</td></tr>",
                    escape(&label),
                    escape(&profit(total)),
                    total.orders
                ));
            }
            Row::Group(name) => html.push_str(&format!(
                "<tr><td colspan=\"3\"><b>{}</b></td></tr>",
                escape(&plain_label(name))
            )),
            Row::Subtotal(name, total) => html.push_str(&format!(
                "<tr><td><i>{}</i></td><td align=\"right\"><b>{}</b></td><td align=\"right\">{}</td></tr>",
                escape(&plain_label(name)),
                escape(&profit(total)),
                total.orders
            )),
        }
    }
    html.push_str("</table>");
    // The details name each core of the page once, however many groups list it: a repeat counts
    // only when its first listing is on another page.
    let mut lines: Vec<(&String, &QuoteBreakdown)> = page
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Line(name, total) => Some((name, total)),
            Row::Repeat(..) | Row::Group(_) | Row::Subtotal(..) => None,
        })
        .collect();
    for row in &page.rows {
        if let Row::Repeat(name, total) = row
            && !lines.iter().any(|(listed, _)| *listed == name)
        {
            lines.push((name, total));
        }
    }
    html.push_str(&format!(
        "<details><summary>{}</summary><table compact><tr><th>PnL</th><th>{}</th></tr>",
        escape(&t!("telegram.report_details")),
        escape(&t!("telegram.report_average"))
    ));
    for &(name, total) in &lines {
        let label = plain_label(name);
        let average = total
            .average_order_return()
            .map(|value| {
                format!(
                    "{} {}",
                    fmt::group_decimal(&if value.avg_order < 1.0 {
                        fmt::adaptive(value.avg_order)
                    } else {
                        fmt::compact(value.avg_order, 2)
                    }),
                    value.currency.ticker()
                )
            })
            .unwrap_or_else(|| t!("telegram.report_unvalued").to_string());
        html.push_str(&format!(
            "<tr><td colspan=\"2\"><b>{}</b></td></tr><tr><td>{}</td><td align=\"right\">{}</td></tr>",
            escape(&label),
            escape(&native(total)).replace(' ', "&#160;").replace(";&#160;", "; "),
            escape(&average).replace(' ', "&#160;")
        ));
    }
    html.push_str("</table>");
    for &(name, total) in &lines {
        let (counted, excluded) = total
            .average_order_return()
            .map(|value| (value.counted, value.excluded))
            .unwrap_or_else(|| {
                let counted = total
                    .entry_spend
                    .counted_orders
                    .clamp(0, total.orders.max(0));
                (counted, total.orders.saturating_sub(counted))
            });
        if excluded > 0 {
            let label = plain_label(name);
            html.push_str(&format!(
                "<p>{}: {}</p>",
                escape(&label),
                escape(&t!(
                    "telegram.report_average_coverage",
                    counted = counted,
                    excluded = excluded
                ))
            ));
        }
    }
    html.push_str("</details>");
    if page.pages > 1 {
        html.push_str(&format!(
            "<p>{} {}/{}</p>",
            escape(&t!("telegram.report_page")),
            page.request.page + 1,
            page.pages
        ));
    }
    html
}

/// Help is disposable rich content; a separate message owns the reply keyboard.
///
/// `host` picks the lines that say what the bot depends on: a terminal must keep running, a
/// station reports around the clock. `owner` controls station command help; `navigation` is the
/// chat's reply keyboard.
pub(crate) fn help(zone: &str, host: HostKind, owner: bool, navigation: ReplyMarkup) -> Response {
    let (limits, mini) = match host {
        HostKind::Terminal => ("telegram.help_limits", "telegram.help_mini"),
        HostKind::Station => ("telegram.help_limits_station", "telegram.help_mini_station"),
    };
    let mut html = format!(
        "<h2>MoonTerminal</h2><p>{}</p><details><summary>{}</summary>",
        escape(&t!("telegram.help_intro")),
        escape(&t!("telegram.help_commands"))
    );
    for (command, key) in [
        ("/hour", "telegram.help_hour"),
        ("/today", "telegram.button_today"),
        ("/yesterday", "telegram.button_yesterday"),
        ("/month", "telegram.button_month"),
        ("/lastmonth", "telegram.button_lastmonth"),
        ("/daily", "telegram.help_daily"),
    ] {
        html.push_str(&format!(
            "<p><code>{command}</code> &#183; {}</p>",
            escape(&t!(key))
        ));
    }
    if host == HostKind::Station && owner {
        html.push_str(&format!(
            "<p><code>/status</code> &#183; {}</p>",
            escape(&t!("telegram.help_status_station"))
        ));
    }
    if owner {
        html.push_str(&format!(
            "<p><code>/settings</code> &#183; {}</p>",
            escape(&t!("telegram.settings.help"))
        ));
        html.push_str(&format!(
            "<p><code>/control</code> &#183; {}</p>",
            escape(&t!("telegram.control.help"))
        ));
    }
    html.push_str(&format!("<p><b>{}</b></p><pre>/report 2026-09-01 2026-09-10</pre><pre>/daily 2026-09-01 2026-09-10</pre><p>{}</p></details>",
        escape(&t!("telegram.help_custom")), escape(&t!(limits, zone = zone))));
    html.push_str(&format!(
        "<details><summary>{}</summary>",
        escape(&t!("telegram.report_calculation"))
    ));
    let scope = match host {
        HostKind::Terminal => "telegram.report_scope",
        HostKind::Station => "telegram.report_scope_station",
    };
    for key in [
        scope,
        "telegram.report_missing_rates",
        "telegram.report_calculation_extra",
    ] {
        html.push_str(&format!("<p>{}</p>", escape(&t!(key))));
    }
    html.push_str(&format!(
        "</details><details><summary>Mini App</summary><p>{}</p></details>",
        escape(&t!(mini))
    ));
    Response::Rich {
        html,
        keyboard: ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(Vec::new())),
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            navigation,
        ),
    }
}

/// A user-controlled name without control characters, cut to 200 of them.
fn plain_label(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// Preserve both ends of a long core name; the complete identity remains in details.
fn compact_label(name: &str) -> String {
    let chars: Vec<_> = name.chars().filter(|c| !c.is_control()).collect();
    if chars.len() <= 24 {
        return chars.into_iter().collect();
    }
    format!(
        "{}…{}",
        chars[..8].iter().collect::<String>(),
        chars[chars.len() - 15..].iter().collect::<String>()
    )
}

/// Use venue identity so localized captions and futures variants retain the same brand color.
fn exchange_icon(scope: ReportScope) -> &'static str {
    use moon_core::venue::{Brand, venue};
    let brand = match scope {
        ReportScope::Venue(id) => venue(id.code).map(|v| v.brand),
        _ => None,
    };
    match brand {
        Some(Brand::Binance) => "\u{1f7e1}",
        Some(Brand::Bybit) => "\u{1f535}",
        Some(Brand::Gate | Brand::Hyperliquid) => "\u{1f7e2}",
        Some(Brand::BitGet) => "\u{1f7e0}",
        Some(Brand::Htx) => "\u{1f534}",
        Some(Brand::Okx) => "\u{26ab}",
        None => "\u{26aa}",
    }
}

/// Inline actions affect the current report; global period choices live in the reply keyboard.
pub(super) fn keyboard(page: &Page) -> ReplyMarkup {
    let request = &page.request;
    let button = |key: &str, request: ReportRequest| {
        let icon = match key {
            "telegram.report_back" | "telegram.report_prev" => "\u{2b05}\u{fe0f}",
            "telegram.report_next" => "\u{27a1}\u{fe0f}",
            "telegram.report_all_cores" | "telegram.report_cores_scope" => "\u{1f9e9}",
            "telegram.report_exchanges_back" => "\u{2190}",
            _ => "\u{1f4c5}",
        };
        InlineKeyboardButton::callback(format!("{icon} {}", t!(key)), request.callback())
    };
    let mut rows = Vec::new();
    if request.exchanges_open {
        for chunk in page.drilldowns.chunks(2) {
            rows.push(
                chunk
                    .iter()
                    .map(|(name, scope)| {
                        let mut next = request.clone();
                        next.scope = *scope;
                        next.by_exchange = false;
                        next.daily = false;
                        next.page = 0;
                        next.exchanges_open = true;
                        InlineKeyboardButton::callback(
                            format!("{} {}", exchange_icon(*scope), compact_label(name)),
                            next.callback(),
                        )
                    })
                    .collect(),
            );
        }
        let mut closed = request.clone();
        closed.exchanges_open = false;
        rows.push(vec![button("telegram.report_exchanges_back", closed)]);
    } else {
        let mut open = request.clone();
        open.exchanges_open = true;
        let mut cores = request.clone();
        cores.by_exchange = false;
        cores.daily = false;
        cores.page = 0;
        let menu = t!("telegram.report_exchanges_menu");
        rows.push(vec![
            InlineKeyboardButton::callback(format!("{menu} \u{25be}"), open.callback()),
            button("telegram.report_all_cores", cores),
        ]);
    }
    let mut views = Vec::new();
    for (key, exchanges, daily) in [
        ("telegram.report_back", true, false),
        (
            if request.scope == ReportScope::All {
                "telegram.report_all_cores"
            } else {
                "telegram.report_cores_scope"
            },
            false,
            false,
        ),
        (
            if request.scope == ReportScope::All {
                "telegram.report_by_days"
            } else {
                "telegram.report_days_scope"
            },
            false,
            true,
        ),
    ] {
        // A single day has nothing to split by days.
        if (daily && !request.period.spans_days())
            || (request.by_exchange == exchanges && request.daily == daily)
            // The collapsed top row already carries All cores; do not repeat it below.
            || (!request.exchanges_open && !exchanges && !daily)
        {
            continue;
        }
        let mut next = request.clone();
        next.by_exchange = exchanges;
        next.daily = daily;
        next.page = 0;
        if exchanges {
            next.scope = ReportScope::All;
        }
        views.push(button(key, next));
    }
    // One row while it holds them: every row of buttons is a line the report no longer fits in.
    match rows.last_mut() {
        Some(top) if !request.exchanges_open && top.len() + views.len() <= 3 => top.extend(views),
        _ if !views.is_empty() => rows.push(views),
        _ => {}
    }
    let mut nav = Vec::new();
    if request.page > 0 {
        let mut prev = request.clone();
        prev.page -= 1;
        nav.push(button("telegram.report_prev", prev));
    }
    if request.page + 1 < page.pages {
        let mut next = request.clone();
        next.page += 1;
        nav.push(button("telegram.report_next", next));
    }
    if !nav.is_empty() {
        rows.push(nav);
    }
    ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(rows))
}
