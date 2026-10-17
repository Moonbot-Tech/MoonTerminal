//! Exchange summaries and activity-only ranked core pages over the report's pinned snapshot.

use super::{Page, Row, db, paging, render, report_html, rich_message_fits, sections};
use crate::t;
use moon_core::config::CoreGroup;
use moon_core::db::ReportFilter;

/// Metrics and disjoint rankings shared by the summary and its paged core roster.
#[derive(Clone)]
pub(super) struct Exchange {
    pub(super) cores: usize,
    pub(super) active: usize,
    pub(super) winrate: f64,
    pub(super) best: Vec<Row>,
    pub(super) worst: Vec<Row>,
}

/// Read each core and saved group through the same filter as the exchange total.
pub(super) fn read(
    snap: &rusqlite::Transaction<'_>,
    mut page: Page,
    filter: &ReportFilter,
    cores: &[(u64, String)],
    groups: &[CoreGroup],
    roster_count: usize,
) -> db::ReadResult<Page> {
    let slice = |ids| db::TotalsSlice {
        core_uids: ids,
        date_from: filter.date_from,
        date_to: filter.date_to,
    };
    let mut slices = vec![slice(None)];
    slices.extend(cores.iter().map(|(id, _)| slice(Some(vec![*id]))));
    let mut totals = db::query_totals_sliced(snap, filter, &slices)?.into_iter();
    page.total = totals.next().unwrap_or_default().quotes;
    let mut active: Vec<Row> = cores
        .iter()
        .zip(totals)
        .filter(|(_, total)| total.quotes.orders > 0)
        .map(|((id, name), total)| Row::Line(Some(*id), name.clone(), total.quotes))
        .collect();
    // Compare native results only when every active core has that same exact denomination.
    let currency = active
        .first()
        .and_then(|row| match row.total().totals.as_slice() {
            [part] if row.total().unknown_orders == 0 => Some(part.currency),
            _ => None,
        })
        .filter(|currency| {
            active.iter().all(|row| {
                matches!(row.total().totals.as_slice(),
        [part] if part.currency == *currency && row.total().unknown_orders == 0)
            })
        });
    let all_usdt = active
        .iter()
        .all(|row| row.total().unified_usdt().is_some());
    let score = |row: &Row| {
        let native = if all_usdt { None } else { currency };
        native
            .map(|_| row.total().totals[0].profit)
            .or_else(|| row.total().unified_usdt().map(|value| value.profit))
            .filter(|value| value.is_finite())
    };
    active.sort_by(|left, right| match (score(left), score(right)) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    // Unvalued cores remain in the complete roster, but cannot honestly be ranked best/worst.
    let ranked: Vec<_> = active
        .iter()
        .filter(|row| score(row).is_some())
        .cloned()
        .collect();
    let best_count = ranked.len().div_ceil(2).min(3);
    let worst_count = (ranked.len() - best_count).min(3);
    let (wins, trades) = db::query_report_wins(snap, filter)?;
    page.exchange = Some(Exchange {
        cores: roster_count,
        active: active.len(),
        winrate: if trades == 0 {
            0.0
        } else {
            wins as f64 * 100.0 / trades as f64
        },
        best: ranked.iter().rev().take(best_count).cloned().collect(),
        worst: ranked.iter().take(worst_count).cloned().collect(),
    });
    let rows = if page.request.by_exchange {
        let ids: Vec<_> = active
            .iter()
            .filter_map(|row| match row {
                Row::Line(id, _, _) => *id,
                _ => None,
            })
            .collect();
        let sections = sections::summary_sections(&ids, groups);
        let slices: Vec<_> = sections
            .iter()
            .map(|section| {
                slice(Some(
                    section.members.iter().map(|&index| ids[index]).collect(),
                ))
            })
            .collect();
        let totals = db::query_totals_sliced(snap, filter, &slices)?;
        sections
            .iter()
            .zip(totals)
            .map(|(section, total)| {
                Row::Line(
                    None,
                    section.group.map_or_else(
                        || t!("profit_monitor.group.ungrouped").to_string(),
                        |group| group.name.clone(),
                    ),
                    total.quotes,
                )
            })
            .collect()
    } else {
        active
    };
    let fits = |rows: &[Row]| {
        let mut probe = page.clone();
        probe.rows = rows.to_vec();
        probe.pages = 10_000;
        probe.request.page = 9_999;
        rich_message_fits(&report_html(&probe))
    };
    let size = if !page.request.by_exchange && rows.chunks(10).all(fits) {
        10
    } else {
        paging::fitting_page_size(&rows, |rows| {
            (page.request.by_exchange || rows.len() <= 10) && fits(rows)
        })
    };
    page.pages = rows.len().div_ceil(size).max(1);
    page.request.page = page.request.page.min(page.pages - 1);
    page.rows = rows
        .into_iter()
        .skip(page.request.page * size)
        .take(size)
        .collect();
    Ok(page)
}

/// Longest shared prefix ending at a separator, leaving a nonempty suffix in every name.
pub(super) fn shared_prefix(names: &[&str]) -> String {
    if names.len() < 2 {
        return String::new();
    }
    let first = names[0];
    first
        .char_indices()
        .filter_map(|(index, ch)| (ch == ' ').then_some(index + ch.len_utf8()))
        .rfind(|&end| {
            names
                .iter()
                .all(|name| name.starts_with(&first[..end]) && !name[end..].trim().is_empty())
        })
        .map_or_else(String::new, |end| first[..end].to_string())
}

/// Render the exchange's sections in one table so every numeric column shares its width.
pub(super) fn html(page: &Page) -> String {
    let Some(exchange) = &page.exchange else {
        return String::new();
    };
    let all = !page.request.by_exchange;
    let now = i64::try_from(moon_core::util::time::now_unix_secs()).unwrap_or(i64::MAX);
    let year = moon_core::util::display_time::at(now, page.zone)
        .map_or(i32::MIN, |now| chrono::Datelike::year(&now));
    let mut html = format!(
        "<p><b>{} {}</b> · {}</p>",
        render::exchange_icon(page.request.scope),
        render::escape(page.scope_label.as_deref().unwrap_or_default()),
        if all {
            format!(
                "{} · {} {}/{}",
                render::escape(&t!("telegram.report_cores")),
                render::escape(&t!("telegram.report_page_short")),
                page.request.page + 1,
                page.pages
            )
        } else {
            render::escape(&render::period(page, year))
        }
    );
    if page.basis == moon_core::config::telegram_menu::ReportBasis::Open {
        html.push_str(&format!(
            "<p><i>{}</i></p>",
            render::escape(&t!("report.period_basis.open"))
        ));
    }
    if page.total.orders == 0 {
        html.push_str(&format!(
            "<p>{}</p>",
            render::escape(&t!("telegram.report_empty"))
        ));
    }
    let listed: Vec<_> = if all {
        page.rows.iter().collect()
    } else {
        exchange.best.iter().chain(&exchange.worst).collect()
    };
    let names: Vec<_> = listed.iter().map(|row| name(row)).collect();
    let prefix = shared_prefix(&names);
    let prefix_caption: String = prefix
        .chars()
        .filter(|ch| !ch.is_control())
        .take(200)
        .collect();
    let caption = || {
        if prefix.is_empty() {
            String::new()
        } else {
            format!(
                "<p>{}</p>",
                render::escape(&t!(
                    "telegram.report_hidden_prefix",
                    prefix = prefix_caption.trim_end()
                ))
            )
        }
    };
    if all {
        html.push_str(&caption());
    }
    html.push_str("<table compact>");
    let columns = 1 + page.layout.drawable_columns().count();
    let section = |key: &str| {
        format!(
            "<tr><td colspan=\"{columns}\"><b>{}</b></td></tr>",
            render::escape(&t!(key))
        )
    };
    if all {
        html.push_str(&render::table_header("telegram.report_cores", &page.layout));
        for row in &page.rows {
            html.push_str(&short_row(row, &prefix, page));
        }
    } else {
        html.push_str(&render::named_total_html(
            &page.total,
            &page.layout,
            "telegram.report_exchange_total",
        ));
        let average = average_order_usdt(&page.total)
            .map(|value| format!("{value:.0}$"))
            .unwrap_or_else(|| t!("telegram.report_unvalued").to_string());
        html.push_str(&format!(
            "<tr><td colspan=\"{columns}\">{}</td></tr>",
            render::escape(&t!(
                "telegram.report_exchange_summary",
                cores = exchange.cores,
                active = exchange.active,
                winrate = format!("{:.0}", exchange.winrate),
                average = average
            ))
        ));
        html.push_str(&section("telegram.report_by_groups"));
        for row in &page.rows {
            html.push_str(&render::row_html(row, false, &page.layout));
        }
        html.push_str(&section("telegram.report_best_cores"));
        if !prefix.is_empty() {
            html.push_str(&format!(
                "<tr><td colspan=\"{columns}\">{}</td></tr>",
                render::escape(&t!(
                    "telegram.report_hidden_prefix",
                    prefix = prefix_caption.trim_end()
                ))
            ));
        }
        for row in &exchange.best {
            html.push_str(&short_row(row, &prefix, page));
        }
        html.push_str(&section("telegram.report_worst_cores"));
        for row in &exchange.worst {
            html.push_str(&short_row(row, &prefix, page));
        }
    }
    html.push_str("</table>");
    if all {
        html.push_str(&format!(
            "<p>{}</p>",
            render::escape(&t!("telegram.report_rank_footer"))
        ));
    }
    html
}

/// Convert only the counted entry-spend scope, excluding Funding and invalid entries.
fn average_order_usdt(total: &db::QuoteBreakdown) -> Option<f64> {
    total
        .entry_spend
        .unified()
        .map(|(spent, _)| spent / total.entry_spend.counted_orders as f64)
        .or_else(|| {
            total
                .average_order_return()
                .filter(|average| average.currency == db::QuoteCurrency::usdt())
                .map(|average| average.avg_order)
        })
        .filter(|value| value.is_finite())
}

/// Borrow a row's name without applying the global core-name ellipsis.
fn name(row: &Row) -> &str {
    match row {
        Row::Line(_, name, _) | Row::Repeat(_, name, _) | Row::Group(name, _) => name,
    }
}

/// Remove only the verified shared prefix and retain the existing money-cell renderer.
fn short_row(row: &Row, prefix: &str, page: &Page) -> String {
    render::row_html(
        &Row::Line(
            None,
            name(row)
                .strip_prefix(prefix)
                .unwrap_or(name(row))
                .to_string(),
            row.total().clone(),
        ),
        false,
        &page.layout,
    )
}

#[cfg(test)]
mod tests;
