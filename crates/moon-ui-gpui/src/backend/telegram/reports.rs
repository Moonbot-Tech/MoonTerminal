//! Localized rich reports over the same snapshot, time axis, and money reader as Report.
use crate::{Backend, core_order::CoreOrder};
use chrono::Days;
use chrono_tz::Tz;
use gpui::Context;
use moon_core::{
    config::telegram_access::TelegramReportAccess,
    db::{self, QuoteBreakdown, ReportFilter, RowScope},
    telegram::{
        api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
        report::{Period, ReportRequest, ReportScope},
        runtime::Response,
    },
    util::{display_time, fmt},
};
use rust_i18n::t;
use std::sync::mpsc::SyncSender;

/// Short pages keep the native table readable on phones and bound per-request query work.
const PAGE_SIZE: usize = 6;

/// A complete page plus a full-period total, all read in one SQLite snapshot.
struct Page {
    request: ReportRequest,
    from: i64,
    to: i64,
    zone: Tz,
    total: QuoteBreakdown,
    rows: Vec<(String, QuoteBreakdown)>,
    pages: usize,
    drilldowns: Vec<(String, ReportScope)>,
    scope_label: Option<String>,
}

impl Backend {
    /// Read off GPUI and recheck the saved chat authorization before returning any money.
    pub(super) fn telegram_report(
        &mut self,
        chat: i64,
        request: ReportRequest,
        reply: SyncSender<Response>,
        cx: &mut Context<Self>,
    ) {
        let Some(access) = self.config.telegram.report_access(chat) else {
            super::answer(&reply, t!("telegram.refusal").to_string());
            return;
        };
        if matches!(&access, TelegramReportAccess::Viewer(ids) if ids.is_empty()) {
            report_notice(&reply, t!("telegram.access_no_cores").to_string());
            return;
        }
        if self.telegram.report_pending {
            report_notice(&reply, t!("telegram.report_busy").to_string());
            return;
        }
        let zone = crate::chrome::clock::resolved_header_clock_zone(self.header_clock_zone());
        let now = moon_core::util::time::now_unix_secs() as i64;
        let Some((from, to)) = request.bounds(now, zone) else {
            report_notice(&reply, t!("telegram.report_help").to_string());
            return;
        };
        let order = CoreOrder::new(&self.config);
        let venues = self.session.core_venues().clone();
        let read_access = access.clone();
        self.telegram.report_pending = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(
                    async move { read_page(request, from, to, zone, order, venues, read_access) },
                )
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, _| {
                    this.telegram.report_pending = false;
                    if this.config.telegram.report_access(chat).as_ref() != Some(&access) {
                        super::answer(&reply, t!("telegram.refusal").to_string());
                        return;
                    }
                    match result {
                        Ok(page) => {
                            let _ = reply.try_send(render(&page));
                        }
                        Err(_) => report_notice(&reply, t!("telegram.report_failed").to_string()),
                    }
                });
            });
        })
        .detach();
    }
}

/// A report read failure still exposes global navigation, including during first /start.
fn report_notice(reply: &SyncSender<Response>, text: String) {
    let _ = reply.try_send(Response::Text {
        text,
        keyboard: Some(super::navigation_keyboard()),
    });
}

/// Read only visible groups, while the headline always covers the entire requested period.
fn read_page(
    request: ReportRequest,
    from: i64,
    to: i64,
    zone: Tz,
    order: CoreOrder,
    venues: std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
    access: TelegramReportAccess,
) -> db::ReadResult<Page> {
    let conn = db::open_reader()?;
    read_page_on(&conn, request, from, to, zone, |cores| {
        order.sort_by(cores, |(id, _)| *id);
        (venues, access)
    })
}

/// Connection-injected reader lets fixtures exercise the exact production query contract.
fn read_page_on(
    conn: &rusqlite::Connection,
    mut request: ReportRequest,
    from: i64,
    to: i64,
    zone: Tz,
    order: impl FnOnce(
        &mut [(u64, String)],
    ) -> (
        std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
        TelegramReportAccess,
    ),
) -> db::ReadResult<Page> {
    request.window = Some((from, to));
    let snap = db::read_snapshot(conn)?;
    let mut cores = db::distinct_cores(&snap)?;
    let (venues, access) = order(&mut cores);
    if let TelegramReportAccess::Viewer(allowed) = &access {
        cores.retain(|(id, _)| allowed.contains(id));
    }
    let scope_label = (request.scope != ReportScope::All).then(|| {
        cores
            .iter()
            .find(|(id, _)| scope_of(venues.get(id)) == request.scope)
            .map(|(id, _)| crate::controls::venue_section_label(venues.get(id)))
            .unwrap_or_else(|| t!("telegram.report_scope_unavailable").to_string())
    });
    if request.scope != ReportScope::All {
        cores.retain(|(id, _)| scope_of(venues.get(id)) == request.scope);
    }
    let scoped_ids = if request.scope == ReportScope::All && access == TelegramReportAccess::Owner {
        Vec::new()
    } else if cores.is_empty() {
        vec![moon_core::config::NO_MATCH_CORE_UID]
    } else {
        cores.iter().map(|(id, _)| *id).collect()
    };
    let filter = ReportFilter {
        core_uids: scoped_ids,
        date_from: Some(from),
        date_to: Some(to),
        emulator: Some(false),
        rows: RowScope::Closed,
        axis: db::ReportAxis::load(&snap, zone)?,
        ..Default::default()
    };
    let total = db::query_totals(&snap, &filter)?.quotes;
    let mut groups = Vec::new();
    let mut group_scopes = Vec::new();
    if request.daily {
        if let (Some(mut date), Some(end)) =
            (display_time::date(from, zone), display_time::date(to, zone))
        {
            // A frozen range can cover additional partial days after a display-zone change.
            // Validate rather than silently dropping dates from a complete-period headline.
            let days = (end - date).num_days();
            if !(0..370).contains(&days) {
                return Err(db::ReadFail::Failed {
                    kind: db::FailKind::Other,
                    msg: "telegram report calendar range is invalid".into(),
                });
            }
            for _ in 0..=days {
                let Some(next) = date.checked_add_days(Days::new(1)) else {
                    break;
                };
                if let (Some(start), Some(stop)) = (
                    display_time::day_start(date, zone),
                    display_time::day_start(next, zone),
                ) {
                    let mut day = filter.clone();
                    day.date_from = Some(start.max(from));
                    day.date_to = Some((stop - 1).min(to));
                    groups.push((date.to_string(), day));
                    group_scopes.push(None);
                }
                date = next;
            }
        }
    } else if request.by_exchange {
        for (venue, members) in crate::core_order::exchange_sections(
            cores
                .iter()
                .enumerate()
                .map(|(index, (id, _))| (index, venues.get(id))),
        ) {
            let mut group = filter.clone();
            group.core_uids = members.iter().map(|&index| cores[index].0).collect();
            groups.push((crate::controls::venue_section_label(venue), group));
            group_scopes.push(Some(scope_of(venue)));
        }
    } else {
        for (id, name) in cores {
            let mut core = filter.clone();
            core.core_uids = vec![id];
            groups.push((name, core));
            group_scopes.push(None);
        }
    }
    // Filter by actual activity before paging, retaining zero-PnL trades and native-only money.
    let mut active = Vec::new();
    for ((name, filter), scope) in groups.into_iter().zip(group_scopes) {
        let total = db::query_totals(&snap, &filter)?.quotes;
        if total.orders > 0 {
            active.push((name, total, scope));
        }
    }
    let pages = active.len().div_ceil(PAGE_SIZE).max(1);
    request.page = request.page.min(pages - 1);
    let mut rows = Vec::new();
    let mut drilldowns = Vec::new();
    for (name, total, scope) in active
        .into_iter()
        .skip(request.page * PAGE_SIZE)
        .take(PAGE_SIZE)
    {
        if let Some(scope) = scope {
            drilldowns.push((name.clone(), scope));
        }
        rows.push((name, total));
    }
    Ok(Page {
        request,
        from,
        to,
        zone,
        total,
        rows,
        pages,
        drilldowns,
        scope_label,
    })
}

/// Escape all external text before inserting it into Telegram's restricted rich HTML.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render complete USDT only; missing or unknown valuation never masquerades as zero.
fn profit(total: &QuoteBreakdown) -> String {
    if total.orders == 0 {
        return "0.00$".into();
    }
    total
        .unified_usdt()
        .and_then(|amount| fmt::signed_fixed(amount.profit, 2).map(|value| format!("{}$", value.0)))
        .unwrap_or_else(|| t!("telegram.report_unvalued").to_string())
}

/// Native currency subtotals preserve useful evidence even when conversion is incomplete.
fn native(total: &QuoteBreakdown) -> String {
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

/// Compose a compact headline, three-column table, and optional per-bot accounting details.
fn render(page: &Page) -> Response {
    let heading = if page.request.daily {
        t!("telegram.report_days")
    } else if page.request.by_exchange {
        t!("telegram.report_exchanges")
    } else {
        t!("telegram.report_cores")
    };
    let stamp = |value| {
        display_time::at(value, page.zone)
            .map(|v| v.format("%d.%m.%Y %H:%M").to_string())
            .unwrap_or_default()
    };
    let title = page.scope_label.as_deref().unwrap_or(&heading);
    let mut html = if page.request.by_exchange && page.scope_label.is_none() {
        format!("<p>{} — {}</p>", stamp(page.from), stamp(page.to))
    } else {
        format!(
            "<p><b>{}</b></p><p>{} — {}</p>",
            escape(title),
            stamp(page.from),
            stamp(page.to)
        )
    };
    if page.total.orders == 0 {
        html.push_str(&format!("<p>{}</p>", escape(&t!("telegram.report_empty"))));
    }
    html.push_str(&format!(
        "<table compact striped><tr><th>{}</th><th align=\"right\">USDT</th><th align=\"right\">{}</th></tr>",
        escape(&if page.request.daily {
            t!("telegram.report_date")
        } else if page.request.by_exchange {
            t!("telegram.report_exchange")
        } else {
            t!("telegram.report_bot")
        }),
        escape(&t!("telegram.report_trades"))
    ));
    for (name, total) in &page.rows {
        // Long user-controlled names cannot exhaust the rich-message budget.
        let label: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
        if !page.request.by_exchange && !page.request.daily {
            html.push_str(&format!(
                "<tr><td colspan=\"3\"><b>{}</b></td></tr>",
                escape(&label)
            ));
        }
        let label = if !page.request.by_exchange && !page.request.daily {
            String::new()
        } else {
            label
        };
        html.push_str(&format!(
            "<tr><td>{}</td><td align=\"right\">{}</td><td align=\"right\">{}</td></tr>",
            escape(&label),
            escape(&profit(total)),
            total.orders
        ));
    }
    html.push_str(&format!("<tr><td><b>{}</b></td><td align=\"right\"><b>{}</b></td><td align=\"right\"><b>{}</b></td></tr></table>",
        escape(&t!("telegram.report_total")),escape(&profit(&page.total)),page.total.orders));
    html.push_str(&format!(
        "<details><summary>{}</summary><table compact><tr><th>PnL</th><th>{}</th></tr>",
        escape(&t!("telegram.report_details")),
        escape(&t!("telegram.report_average"))
    ));
    for (name, total) in &page.rows {
        let label: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
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
    for (name, total) in &page.rows {
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
            let label: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
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
    if html.chars().count() > 30_000 {
        return Response::Text {
            text: t!("telegram.report_delivery_failed").to_string(),
            keyboard: Some(keyboard(page)),
        };
    }
    Response::Rich {
        html,
        keyboard: keyboard(page),
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            super::navigation_keyboard(),
        ),
    }
}

/// Help is disposable rich content; a separate permanent message owns persistent navigation.
pub(super) fn help(zone: &str) -> Response {
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
    html.push_str(&format!("<p><b>{}</b></p><pre>/report 2026-09-01 2026-09-10</pre><pre>/daily 2026-09-01 2026-09-10</pre><p>{}</p></details>",
        escape(&t!("telegram.help_custom")), escape(&t!("telegram.help_limits", zone = zone))));
    html.push_str(&format!(
        "<details><summary>{}</summary>",
        escape(&t!("telegram.report_calculation"))
    ));
    for key in [
        "telegram.report_scope",
        "telegram.report_missing_rates",
        "telegram.report_calculation_extra",
    ] {
        html.push_str(&format!("<p>{}</p>", escape(&t!(key))));
    }
    html.push_str(&format!(
        "</details><details><summary>Mini App</summary><p>{}</p></details>",
        escape(&t!("telegram.help_mini"))
    ));
    Response::Rich {
        html,
        keyboard: ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(Vec::new())),
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            super::navigation_keyboard(),
        ),
    }
}

/// Resolve exactly the same venue identity used by the terminal's core lists.
fn scope_of(venue: Option<&moon_core::venue::CoreVenue>) -> ReportScope {
    match crate::core_order::section_of(venue) {
        crate::core_order::ExchangeSection::Unidentified => ReportScope::Unidentified,
        crate::core_order::ExchangeSection::Venue(id) => ReportScope::Venue(id),
    }
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
fn keyboard(page: &Page) -> ReplyMarkup {
    let request = &page.request;
    let button = |key: &str, request: ReportRequest| {
        let icon = match key {
            "telegram.report_back" | "telegram.report_prev" => "\u{2b05}\u{fe0f}",
            "telegram.report_next" => "\u{27a1}\u{fe0f}",
            "telegram.report_all_cores" | "telegram.report_cores_scope" => "\u{1f9e9}",
            _ => "\u{1f4c5}",
        };
        InlineKeyboardButton::callback(format!("{icon} {}", t!(key)), request.callback())
    };
    let mut rows = Vec::new();
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
                    InlineKeyboardButton::callback(
                        format!("{} {}", exchange_icon(*scope), compact_label(name)),
                        next.callback(),
                    )
                })
                .collect(),
        );
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
        if (daily && request.period == Period::Today)
            || (request.by_exchange == exchanges && request.daily == daily)
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
    if !views.is_empty() {
        rows.push(views);
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

#[cfg(test)]
mod tests;
