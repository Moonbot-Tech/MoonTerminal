//! The report's rich HTML, its inline keyboard, and Help.

use crate::t;
use moon_core::{
    config::{
        GroupRowStyle, ReportColumn, ReportLayout, TotalPlace, TotalSeparation,
        telegram_menu::ReportBasis,
    },
    db::{QuoteBreakdown, ValuationPart},
    telegram::{
        api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
        report::{ReportRequest, ReportScope},
        runtime::Response,
    },
    util::{
        display_time,
        fmt::{self, DeltaSign},
    },
};

use super::{Page, Row};
use crate::HostKind;
use chrono::{DateTime, Datelike, Timelike};
use chrono_tz::Tz;

/// Telegram `sendRichMessage` cap: 32768 UTF-8 characters in the rich message text.
const RICH_MESSAGE_CHAR_LIMIT: usize = 32_768;
/// Telegram `sendRichMessage` cap: 500 blocks, including nested blocks and table rows.
const RICH_MESSAGE_BLOCK_LIMIT: usize = 500;

/// Escape all external text before inserting it into Telegram's restricted rich HTML.
pub(crate) use crate::html::escape;

/// Render the result as complete USDT, or, when some trades have no USDT value, as its parts.
///
/// Missing or unknown valuation never masquerades as zero; a trade without a rate no longer hides
/// what the rest made either — see [`profit_cell`]. Only the tests read the text alone.
#[cfg(test)]
pub(super) fn profit(total: &QuoteBreakdown) -> String {
    profit_cell(total).0
}

/// The result text and the sign that colours it.
///
/// A complete USDT total reads `+19.32$`. Otherwise every part of
/// [`QuoteBreakdown::valuation_split`] is stated on its own — the fully valued currencies as one
/// dollar figure, each other currency in its own money (`+200.00 RUB`), unknown currencies as
/// unvalued — joined by `; `. They are never added: there is no rate to add them with. Only a
/// single part has a sign to colour by.
fn profit_cell(total: &QuoteBreakdown) -> (String, Option<DeltaSign>) {
    if total.orders == 0 {
        return ("0.00$".into(), None);
    }
    if let Some((text, sign)) = usdt_text(total) {
        return (text, Some(sign));
    }
    let parts: Vec<_> = total
        .valuation_split()
        .iter()
        .map(|part| match part {
            ValuationPart::Usdt(part) => usdt_text(part),
            ValuationPart::Native(currency, part) => part.totals.first().and_then(|bucket| {
                fmt::signed_fixed(bucket.profit, currency.display_decimals())
                    .map(|(amount, sign)| (format!("{amount}\u{a0}{}", currency.ticker()), sign))
            }),
            ValuationPart::Unknown(_) => None,
        })
        .collect();
    let sign = match parts.as_slice() {
        [Some((_, sign))] => Some(*sign),
        _ => None,
    };
    let text = parts
        .into_iter()
        .map(|part| part.map_or_else(|| t!("telegram.report_unvalued").to_string(), |p| p.0))
        .collect::<Vec<_>>()
        .join("; ");
    (text, sign)
}

/// A complete USDT result as `+19.32$` with its sign, or `None` when it is incomplete.
fn usdt_text(total: &QuoteBreakdown) -> Option<(String, DeltaSign)> {
    total
        .unified_usdt()
        .and_then(|amount| fmt::signed_fixed(amount.profit, 2))
        .map(|(amount, sign)| (format!("{amount}$"), sign))
}

/// The lines a total band takes: one when the total converts to USDT whole, otherwise one per
/// part of [`QuoteBreakdown::valuation_split`] — `Total USDT`, `Total RUB`, … — so every column of
/// each line describes one comparable scope instead of the whole band reading "unvalued".
///
/// Args:
///     total: The band's money.
///     label: The band's localized label, `Total` or `Exchange total`.
///
/// Returns:
///     `(label, money)` per line, in the split's order.
fn total_lines(total: &QuoteBreakdown, label: &str) -> Vec<(String, QuoteBreakdown)> {
    if total.orders == 0 || total.unified_usdt().is_some() {
        return vec![(label.to_string(), total.clone())];
    }
    total
        .valuation_split()
        .into_iter()
        .map(|part| match part {
            ValuationPart::Usdt(part) => (format!("{label} USDT"), part),
            ValuationPart::Native(currency, part) => {
                (format!("{label} {}", currency.ticker()), part)
            }
            ValuationPart::Unknown(orders) => (
                format!("{label} · {}", t!("telegram.report_quote_unknown")),
                QuoteBreakdown {
                    unknown_orders: orders,
                    orders,
                    ..QuoteBreakdown::default()
                },
            ),
        })
        .collect()
}

/// State one column per part, the way [`profit_cell`] states the result.
///
/// A row that does not convert to USDT whole — a core trading USDT and RUB — would otherwise put
/// a split result beside an "unvalued" average and volume. Each part's cell is computed over that
/// part alone and the texts are joined by `; `; several parts have no single sign to colour by.
/// A total band never reaches the join: [`total_lines`] already hands it one part per line.
///
/// Args:
///     total: The row's money.
///     cell: The column's renderer for one comparable scope.
fn per_part(
    total: &QuoteBreakdown,
    cell: fn(&QuoteBreakdown) -> (String, Option<DeltaSign>),
) -> (String, Option<DeltaSign>) {
    let parts = total_lines(total, "");
    if parts.len() < 2 {
        return cell(total);
    }
    let text = parts
        .iter()
        .map(|(_, part)| cell(part).0)
        .collect::<Vec<_>>()
        .join("; ");
    (text, None)
}

/// Average-order return as a signed percent, or unvalued.
fn average_cell(total: &QuoteBreakdown) -> (String, Option<DeltaSign>) {
    let value = total
        .average_order_return()
        .and_then(|value| fmt::signed_pct(value.pct, 1));
    (
        value.as_ref().map_or_else(
            || t!("telegram.report_unvalued").to_string(),
            |(text, _)| text.clone(),
        ),
        value.map(|(_, sign)| sign),
    )
}

/// Traded volume in whole dollars, or in its one native currency when the scope is complete.
fn volume_cell(total: &QuoteBreakdown) -> (String, Option<DeltaSign>) {
    (
        crate::notify::render::whole_dollar_volume(total.traded_volume.usdt)
            .or_else(|| match total.traded_volume.totals.as_slice() {
                [bucket]
                    if total.traded_volume.usdt.is_none()
                        && total.traded_volume.unknown_orders == 0
                        && total.traded_volume.reconstructed_orders
                            == total.traded_volume.eligible_orders
                        && total.traded_volume.eligible_orders > 0 =>
                {
                    crate::notify::render::native_volume_text(bucket.currency, bucket.amount)
                }
                _ => None,
            })
            .unwrap_or_else(|| t!("telegram.report_unvalued").to_string()),
        None,
    )
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

/// Compose a preview title paragraph, configured table, and per-core accounting details.
/// The saved layout controls numeric columns, total placement and group emphasis.
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

/// The page's period, as short as it stays unambiguous: `05.10 · 00:00—05:59` inside one day,
/// `04.10` for a whole day, `01.10 — 05.10 05:59` across days, where a bound on midnight or on
/// the day's last second names only its date. The year shows only for a period outside `year`.
pub(super) fn period(page: &Page, year: i32) -> String {
    let at = |value| display_time::at(value, page.zone);
    let (Some(from), Some(to)) = (at(page.from), at(page.to)) else {
        let stamp = |value: Option<DateTime<Tz>>| {
            value
                .map(|v| v.format("%d.%m.%Y %H:%M").to_string())
                .unwrap_or_default()
        };
        return format!("{} — {}", stamp(at(page.from)), stamp(at(page.to)));
    };
    let date = if from.year() == year && to.year() == year {
        "%d.%m"
    } else {
        "%d.%m.%Y"
    };
    let starts_day = from.num_seconds_from_midnight() == 0;
    // The inclusive end of a whole day is its last second.
    let ends_day = to.num_seconds_from_midnight() == 86_399;
    if from.date_naive() == to.date_naive() {
        if starts_day && ends_day {
            return from.format(date).to_string();
        }
        return format!(
            "{} · {}—{}",
            from.format(date),
            from.format("%H:%M"),
            to.format("%H:%M")
        );
    }
    let bound = |value: DateTime<Tz>, whole: bool| {
        if whole {
            value.format(date).to_string()
        } else {
            format!("{} {}", value.format(date), value.format("%H:%M"))
        }
    };
    format!("{} — {}", bound(from, starts_day), bound(to, ends_day))
}

/// HTML beginning with the report title and period so Telegram can preview every report page.
/// Manual titles start with localized "Report"; automatic captions keep their own title.
/// Missing groups retain their selected view and show an explanation above the totals table.
/// Saved columns, group styles and whole-scope total placement govern the following table.
/// The caller decides whether it fits Telegram's rich-message caps.
pub(super) fn report_html(page: &Page) -> String {
    if page.exchange.is_some() {
        return super::drill::html(page);
    }
    let heading = if page.request.daily {
        t!("telegram.report_days")
    } else if page.request.by_exchange {
        t!("telegram.report_exchanges")
    } else if page.request.by_group {
        t!("telegram.report_groups")
    } else {
        t!("telegram.report_cores")
    };
    let title = page.scope_label.as_deref().unwrap_or(&heading);
    let mut html = String::new();
    // An unreadable clock names the year rather than guessing it.
    let now = i64::try_from(moon_core::util::time::now_unix_secs()).unwrap_or(i64::MAX);
    let year = display_time::at(now, page.zone).map_or(i32::MIN, |now| now.year());
    let period = period(page, year);
    let mut preview_title = match &page.caption {
        Some(auto) => format!(
            "<b>{}</b> · {} {}",
            escape(&auto.title),
            escape(&period),
            escape(&auto.zone)
        ),
        None if page.request.by_exchange && page.scope_label.is_none() => format!(
            "<b>{}</b> · {}",
            escape(&t!("telegram.report_title")),
            escape(&period)
        ),
        None => format!(
            "<b>{} · {}</b> · {}",
            escape(&t!("telegram.report_title")),
            escape(title),
            escape(&period)
        ),
    };
    // The terminal's Report wording: the period counts trades by when they opened.
    if page.basis == ReportBasis::Open {
        preview_title.push_str(&format!(
            " · <i>{}</i>",
            escape(&t!("report.period_basis.open"))
        ));
    }
    html.push_str(&format!("<p>{preview_title}</p>"));
    if page.groups_missing {
        html.push_str(&format!(
            "<p>{}</p>",
            escape(&t!("telegram.report_groups_missing"))
        ));
    }
    if page.total.orders == 0 {
        html.push_str(&format!("<p>{}</p>", escape(&t!("telegram.report_empty"))));
        if page.rows.is_empty() && !page.groups_missing {
            return html;
        }
    }
    html.push_str(&format!(
        "<table compact><tr><th align=\"left\">{}</th>",
        escape(&if page.request.daily {
            t!("telegram.report_date")
        } else {
            heading
        })
    ));
    for label in column_headers(&page.layout) {
        html.push_str(&format!("<th align=\"right\">{}</th>", escape(&label)));
    }
    html.push_str("</tr>");
    let by_core = !page.request.by_exchange && !page.request.by_group && !page.request.daily;
    for row in ordered_rows(page) {
        match row {
            OrderedRow::Data(row) => html.push_str(&row_html(row, by_core, &page.layout)),
            OrderedRow::Total => html.push_str(&total_html(&page.total, &page.layout)),
            OrderedRow::Spacer => html.push_str(&format!(
                "<tr><td colspan=\"{}\">&#160;</td></tr>",
                1 + page.layout.drawable_columns().count()
            )),
        }
    }
    html.push_str("</table>");
    if page.groups_missing {
        return html;
    }
    // The details name each core of the page once, however many groups list it: a repeat counts
    // only when its first listing is on another page. Core UIDs distinguish reused names.
    let mut lines: Vec<(Option<u64>, &String, &QuoteBreakdown)> = page
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Line(uid, name, total) => Some((*uid, name, total)),
            Row::Repeat(..) | Row::Group(..) => None,
        })
        .collect();
    for row in &page.rows {
        if let Row::Repeat(uid, name, total) = row
            && !lines.iter().any(|(listed, _, _)| listed == uid)
        {
            lines.push((*uid, name, total));
        }
    }
    html.push_str(&format!(
        "<details><summary>{}</summary><table compact><tr><th>PnL</th><th>{}</th></tr>",
        escape(&t!("telegram.report_details")),
        escape(&t!("telegram.report_average"))
    ));
    for &(_, name, total) in &lines {
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
    for &(_, name, total) in &lines {
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

/// One configured row: plain cells for cores, exchanges and days; a band or bold group row.
///
/// A core occupies one main-table row with both ends of its long name retained.
/// Details retain up to 200 non-control characters; the caller checks the whole message budget.
pub(super) fn row_html(row: &Row, by_core: bool, layout: &ReportLayout) -> String {
    let (name, total) = row_label(row, by_core);
    let (cell, lead) = if matches!(row, Row::Group(..)) && layout.group_row == GroupRowStyle::Band {
        ("th", " align=\"right\"")
    } else {
        ("td", "")
    };
    let bold = matches!(row, Row::Group(..)) && layout.group_row == GroupRowStyle::BoldLeft;
    let name = escape(&name);
    let name = if bold { format!("<b>{name}</b>") } else { name };
    let mut html = format!("<tr><{cell}{lead}>{name}</{cell}>");
    html.push_str(&value_cells(total, layout, cell, bold, false));
    html.push_str("</tr>");
    html
}

/// Draw the full-period total as a shaded band distinct from group headers.
fn total_html(total: &QuoteBreakdown, layout: &ReportLayout) -> String {
    named_total_html(total, layout, "telegram.report_total")
}

/// Render a total band with a scope-specific localized label, one row per [`total_lines`] line.
pub(super) fn named_total_html(total: &QuoteBreakdown, layout: &ReportLayout, key: &str) -> String {
    total_lines(total, &t!(key))
        .iter()
        .map(|(label, total)| {
            format!(
                "<tr><th align=\"left\"><b>{}</b></th>{}</tr>",
                escape(label),
                value_cells(total, layout, "th", true, true)
            )
        })
        .collect()
}

/// Emit shared typed column values as escaped HTML in their saved order.
fn value_cells(
    total: &QuoteBreakdown,
    layout: &ReportLayout,
    cell: &str,
    bold: bool,
    total_row: bool,
) -> String {
    column_values(total, layout, total_row)
        .into_iter()
        .map(|mut span| {
            // An empty cell stays empty rather than carrying a bare `<b></b>`.
            span.bold = bold && !span.text.is_empty();
            format!("<{cell} align=\"right\">{}</{cell}>", span.html())
        })
        .collect()
}

/// Share column formatting with previews; native volume requires a complete single-currency scope.
///
/// `total_row` blanks the native-currency cell: the report's total mixes cores of different
/// quote currencies, and a list of per-currency sums there is not a total the reader can use.
fn column_values(
    total: &QuoteBreakdown,
    layout: &ReportLayout,
    total_row: bool,
) -> Vec<crate::preview::Span> {
    use crate::preview::{Span, Tone};
    layout
        .drawable_columns()
        .filter_map(|column| {
            let (text, sign) = match column {
                ReportColumn::Profit => profit_cell(total),
                ReportColumn::Trades => (total.orders.to_string(), None),
                ReportColumn::Average => per_part(total, average_cell),
                ReportColumn::Volume => per_part(total, volume_cell),
                ReportColumn::Native if total_row => (String::new(), None),
                // One currency colours by its sign; a mixed row lists its sums uncoloured.
                // A row with no trades reads 0 rather than a blank; trades of an unknown
                // currency alone stay unvalued. Amount and ticker never wrap apart.
                ReportColumn::Native => (
                    match (total.totals.is_empty(), total.orders) {
                        (true, 0) => "0".to_string(),
                        (true, _) => t!("telegram.report_unvalued").to_string(),
                        _ => native(total)
                            .replace(' ', "\u{a0}")
                            .replace(";\u{a0}", "; "),
                    },
                    match total.totals.as_slice() {
                        [part] => fmt::signed_fixed(part.profit, part.currency.display_decimals())
                            .map(|(_, sign)| sign),
                        _ => None,
                    },
                ),
                ReportColumn::Other(_) => return None,
            };
            Some(Span {
                text,
                tone: sign.map_or(Tone::Plain, |sign| {
                    sign.pick(Tone::Gain, Tone::Loss, Tone::Plain)
                }),
                ..Span::plain("")
            })
        })
        .collect()
}

/// Localized numeric headers in the same saved-column order as their cells.
fn column_headers(layout: &ReportLayout) -> Vec<String> {
    layout
        .drawable_columns()
        .filter_map(|column| {
            let key = match column {
                ReportColumn::Profit => "telegram.report_profit",
                ReportColumn::Trades => "telegram.report_trades",
                ReportColumn::Average => "telegram.report_average_col",
                ReportColumn::Volume => "telegram.report_volume_col",
                ReportColumn::Native => "telegram.report_native_col",
                ReportColumn::Other(_) => return None,
            };
            Some(t!(key).to_string())
        })
        .collect()
}

/// Reuse saved numeric columns in a scoped table's header.
pub(super) fn table_header(key: &str, layout: &ReportLayout) -> String {
    let mut html = format!("<tr><th align=\"left\">{}</th>", escape(&t!(key)));
    for label in column_headers(layout) {
        html.push_str(&format!("<th align=\"right\">{}</th>", escape(&label)));
    }
    html.push_str("</tr>");
    html
}

/// A structural table row; HTML and native rendering consume this same placement walk.
enum OrderedRow<'a> {
    Data(&'a Row),
    Total,
    Spacer,
}

/// Place total and optional spacer around the real data/group rows without another layout walk.
fn ordered_rows(page: &Page) -> Vec<OrderedRow<'_>> {
    let mut rows = Vec::new();
    if page.layout.total == TotalPlace::Top {
        rows.push(OrderedRow::Total);
        if page.layout.separation == TotalSeparation::GapBand {
            rows.push(OrderedRow::Spacer);
        }
    }
    rows.extend(page.rows.iter().map(OrderedRow::Data));
    if page.layout.total == TotalPlace::Bottom {
        if page.layout.separation == TotalSeparation::GapBand {
            rows.push(OrderedRow::Spacer);
        }
        rows.push(OrderedRow::Total);
    }
    rows
}

/// Share core-name compaction and group labels with production cells.
fn row_label(row: &Row, by_core: bool) -> (String, &QuoteBreakdown) {
    match row {
        Row::Line(_, name, total) | Row::Repeat(_, name, total) if by_core => {
            (compact_label(name), total)
        }
        Row::Line(_, name, total) | Row::Repeat(_, name, total) | Row::Group(name, total) => {
            (plain_label(name), total)
        }
    }
}

/// Resolve raw typed table cells and bands through the production row/column helpers.
pub(super) fn preview_table(page: &Page) -> crate::preview::PreviewTable {
    use crate::preview::{PreviewRow, PreviewRowKind, PreviewTable, Span};
    let heading = if page.request.daily {
        t!("telegram.report_days")
    } else if page.request.by_exchange {
        t!("telegram.report_exchanges")
    } else if page.request.by_group {
        t!("telegram.report_groups")
    } else {
        t!("telegram.report_cores")
    };
    let title = page.scope_label.as_deref().unwrap_or(&heading).to_string();
    let mut header = vec![if page.request.daily {
        t!("telegram.report_date").to_string()
    } else {
        heading.to_string()
    }];
    header.extend(column_headers(&page.layout));
    let by_core = !page.request.by_exchange && !page.request.by_group && !page.request.daily;
    let preview_row = |kind: PreviewRowKind,
                       name: String,
                       total: &QuoteBreakdown,
                       band: bool,
                       first_right: bool,
                       bold: bool| {
        let mut cells = vec![Span::plain(name)];
        cells.extend(column_values(
            total,
            &page.layout,
            kind == PreviewRowKind::Total,
        ));
        for cell in &mut cells {
            cell.bold = bold;
        }
        PreviewRow {
            kind,
            cells,
            band,
            first_right,
        }
    };
    let rows = ordered_rows(page)
        .into_iter()
        .flat_map(|row| match row {
            OrderedRow::Spacer => vec![PreviewRow {
                kind: PreviewRowKind::Spacer,
                cells: vec![],
                band: false,
                first_right: false,
            }],
            // The same lines the sent report draws for its total band.
            OrderedRow::Total => total_lines(&page.total, &t!("telegram.report_total"))
                .into_iter()
                .map(|(label, total)| {
                    preview_row(PreviewRowKind::Total, label, &total, true, false, true)
                })
                .collect(),
            OrderedRow::Data(row) => {
                let (name, total) = row_label(row, by_core);
                let group = matches!(row, Row::Group(..));
                let band = group && page.layout.group_row == GroupRowStyle::Band;
                let kind = if group {
                    PreviewRowKind::Group
                } else {
                    PreviewRowKind::Data
                };
                vec![preview_row(kind, name, total, band, band, group)]
            }
        })
        .collect();
    PreviewTable {
        title,
        caption: period(page, crate::preview::sample_clock().year()),
        header,
        rows,
    }
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
pub(super) fn exchange_icon(scope: ReportScope) -> &'static str {
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

/// Persistent mode control shared by the report and the expanded exchange picker.
fn view_switch(request: &ReportRequest) -> InlineKeyboardButton {
    use moon_core::config::telegram_menu::ReportView;
    let current = request.view_mode.unwrap_or(if request.by_exchange {
        ReportView::Exchanges
    } else if request.by_group {
        ReportView::Groups
    } else {
        ReportView::Cores
    });
    let (key, next) = match current {
        ReportView::Exchanges => ("telegram.report_exchanges", ReportView::Groups),
        ReportView::Groups => ("telegram.report_groups", ReportView::Cores),
        ReportView::Cores => ("telegram.report_cores", ReportView::Exchanges),
    };
    let mut switch = request.clone().in_view(next);
    switch.select_view = true;
    switch.scope = ReportScope::All;
    switch.page = 0;
    switch.exchanges_open = false;
    InlineKeyboardButton::callback(
        format!("{} ▾", t!("telegram.report_view_switch", view = t!(key))),
        switch.callback(),
    )
}

/// Inline actions affect the current report; global period choices live in the reply keyboard.
pub(super) fn keyboard(page: &Page) -> ReplyMarkup {
    use moon_core::config::telegram_menu::ReportView;
    let request = &page.request;
    let callback =
        |text: String, next: ReportRequest| InlineKeyboardButton::callback(text, next.callback());
    let mut rows = Vec::new();
    if request.exchanges_open {
        for chunk in page.drilldowns.chunks(2) {
            rows.push(
                chunk
                    .iter()
                    .map(|(name, scope)| {
                        let mut next = request.clone().in_view(ReportView::Exchanges);
                        next.scope = *scope;
                        next.page = 0;
                        next.exchanges_open = false;
                        callback(
                            format!("{} {}", exchange_icon(*scope), compact_label(name)),
                            next,
                        )
                    })
                    .collect(),
            );
        }
        let mut back = request.clone();
        back.exchanges_open = false;
        rows.push(vec![callback(
            format!("← {}", t!("telegram.report_exchanges_back")),
            back,
        )]);
        rows.push(vec![view_switch(request)]);
    } else if page.exchange.is_some() {
        if request.by_exchange {
            let mut all = request.clone().in_view(ReportView::Cores);
            all.page = 0;
            rows.push(vec![callback(
                t!(
                    "telegram.report_exchange_all",
                    active = page.exchange.as_ref().map_or(0, |exchange| exchange.active)
                )
                .to_string(),
                all,
            )]);
            let mut back = request.clone();
            back.scope = ReportScope::All;
            back.follow_view = true;
            back.page = 0;
            let mut other = request.clone();
            other.exchanges_open = true;
            rows.push(vec![
                callback(format!("← {}", t!("telegram.report_exchanges_back")), back),
                callback(format!("{} ▾", t!("telegram.report_other_exchange")), other),
            ]);
        } else {
            let mut nav = Vec::new();
            if request.page > 0 {
                let mut prev = request.clone();
                prev.page -= 1;
                nav.push(callback(
                    format!("‹ {} {}", t!("telegram.report_page_short"), prev.page + 1),
                    prev,
                ));
            }
            if request.page + 1 < page.pages {
                let mut next = request.clone();
                next.page += 1;
                nav.push(callback(
                    format!("{} {} ›", t!("telegram.report_page_short"), next.page + 1),
                    next,
                ));
            }
            if !nav.is_empty() {
                rows.push(nav);
            }
            let mut back = request.clone().in_view(ReportView::Exchanges);
            back.page = 0;
            rows.push(vec![callback(
                format!("← {}", t!("telegram.report_back_exchange")),
                back,
            )]);
        }
    } else {
        let mut open = request.clone();
        open.exchanges_open = true;
        rows.push(vec![
            view_switch(request),
            callback(format!("{} ▾", t!("telegram.report_exchange_menu")), open),
        ]);
        if page.total.orders > 0 || !page.rows.is_empty() {
            let mut cores = request.clone().in_view(ReportView::Cores);
            cores.scope = ReportScope::All;
            cores.page = 0;
            rows.push(vec![callback(
                format!("🧩 {}", t!("telegram.report_all_cores")),
                cores,
            )]);
        }
    }
    // Summary group pages and global tables retain the existing frozen-window paging.
    if !request.exchanges_open && (page.exchange.is_none() || request.by_exchange) {
        let mut nav = Vec::new();
        if request.page > 0 {
            let mut prev = request.clone();
            prev.page -= 1;
            nav.push(callback(format!("← {}", t!("telegram.report_prev")), prev));
        }
        if request.page + 1 < page.pages {
            let mut next = request.clone();
            next.page += 1;
            nav.push(callback(format!("{} →", t!("telegram.report_next")), next));
        }
        if !nav.is_empty() {
            rows.push(nav);
        }
    }
    ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(rows))
}

#[cfg(test)]
mod tests;
