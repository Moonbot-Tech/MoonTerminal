//! Localized rich reports over the same snapshot, time axis, and money reader as Report.
use chrono::Days;
use chrono_tz::Tz;
use moon_core::session::core_order::{self, CoreOrder};
use moon_core::{
    config::telegram_access::TelegramReportAccess,
    db::{self, QuoteBreakdown, ReportFilter, RowScope},
    telegram::{
        report::{ReportRequest, ReportScope},
        runtime::Response,
    },
    util::display_time,
};
use rust_i18n::t;
use std::sync::mpsc::SyncSender;

use crate::TgHost;
use crate::labels::{answer, navigation_keyboard, section_label};

mod render;

pub(crate) use render::help;
use render::{render, report_html, rich_message_fits};

/// Core lists are unbounded, so they still page; breakdown views try to show every row first.
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

/// Read off the owner thread and recheck the saved chat authorization before returning any money.
pub(crate) fn telegram_report(
    host: &mut dyn TgHost,
    chat: i64,
    request: ReportRequest,
    reply: SyncSender<Response>,
) {
    let Some(access) = host.config().telegram.report_access(chat) else {
        answer(&reply, t!("telegram.refusal").to_string());
        return;
    };
    let owner = access == TelegramReportAccess::Owner;
    if matches!(&access, TelegramReportAccess::Viewer(ids) if ids.is_empty()) {
        report_notice(
            &reply,
            t!("telegram.access_no_cores").to_string(),
            host.kind(),
            owner,
        );
        return;
    }
    if host.state().report_pending {
        report_notice(
            &reply,
            t!("telegram.report_busy").to_string(),
            host.kind(),
            owner,
        );
        return;
    }
    let zone = host.report_zone();
    let now = moon_core::util::time::now_unix_secs() as i64;
    let Some((from, to)) = request.bounds(now, zone) else {
        report_notice(
            &reply,
            crate::labels::report_help(host.kind()),
            host.kind(),
            owner,
        );
        return;
    };
    let order = CoreOrder::new(host.config());
    let names = db::CoreNames::from_servers(&host.config().servers);
    let venues = host.session().core_venues().clone();
    let read_access = access.clone();
    host.state_mut().report_pending = true;
    host.spawn(Box::new(move || {
        let result = read_page(request, from, to, zone, order, names, venues, read_access);
        Box::new(move |host: &mut dyn TgHost| {
            host.state_mut().report_pending = false;
            if host.config().telegram.report_access(chat).as_ref() != Some(&access) {
                answer(&reply, t!("telegram.refusal").to_string());
                return;
            }
            match result {
                Ok(page) => {
                    let _ = reply.try_send(render(&page, host.kind(), owner));
                }
                Err(_) => report_notice(
                    &reply,
                    t!("telegram.report_failed").to_string(),
                    host.kind(),
                    owner,
                ),
            }
        })
    }));
}

/// A report notice exposes navigation limited to the admission grant, including on /start.
fn report_notice(reply: &SyncSender<Response>, text: String, host: crate::HostKind, owner: bool) {
    let _ = reply.try_send(Response::Text {
        text,
        keyboard: Some(navigation_keyboard(host, owner)),
    });
}

/// Read only visible groups, while the headline always covers the entire requested period.
#[allow(clippy::too_many_arguments)]
fn read_page(
    request: ReportRequest,
    from: i64,
    to: i64,
    zone: Tz,
    order: CoreOrder,
    names: db::CoreNames,
    venues: std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
    access: TelegramReportAccess,
) -> db::ReadResult<Page> {
    let conn = db::open_reader()?;
    read_page_on(&conn, request, from, to, zone, &names, |cores| {
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
    names: &db::CoreNames,
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
    relabel(&mut cores, names);
    let (venues, access) = order(&mut cores);
    if let TelegramReportAccess::Viewer(allowed) = &access {
        cores.retain(|(id, _)| allowed.contains(id));
    }
    let accessible = cores.clone();
    let scope_label = (request.scope != ReportScope::All).then(|| {
        cores
            .iter()
            .find(|(id, _)| scope_of(venues.get(id)) == request.scope)
            .map(|(id, _)| section_label(venues.get(id)))
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
                return Err(db::ReadFail::failed(
                    db::FailKind::Other,
                    "telegram report calendar range is invalid",
                    moon_core::config::paths::reports_db_path(),
                    "telegram: calendar range",
                    db::FailCode::None,
                ));
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
        for (venue, members) in core_order::exchange_sections(
            cores
                .iter()
                .enumerate()
                .map(|(index, (id, _))| (index, venues.get(id))),
        ) {
            let mut group = filter.clone();
            group.core_uids = members.iter().map(|&index| cores[index].0).collect();
            groups.push((section_label(venue), group));
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
    let drilldowns = exchange_drilldowns(&snap, &accessible, &venues, &filter)?;
    let breakdown = request.daily || request.by_exchange;
    let take =
        |active: &[(String, QuoteBreakdown, Option<ReportScope>)], page: usize, size: usize| {
            active
                .iter()
                .skip(page * size)
                .take(size)
                .map(|(name, total, _)| (name.clone(), total.clone()))
                .collect::<Vec<_>>()
        };
    let (rows, pages) = if breakdown {
        let all = take(&active, 0, active.len().max(1));
        let probe = Page {
            request: request.clone(),
            from,
            to,
            zone,
            total: total.clone(),
            rows: all.clone(),
            pages: 1,
            drilldowns: drilldowns.clone(),
            scope_label: scope_label.clone(),
        };
        if rich_message_fits(&report_html(&probe)) {
            request.page = 0;
            (all, 1)
        } else {
            let pages = active.len().div_ceil(PAGE_SIZE).max(1);
            request.page = request.page.min(pages - 1);
            (take(&active, request.page, PAGE_SIZE), pages)
        }
    } else {
        let pages = active.len().div_ceil(PAGE_SIZE).max(1);
        request.page = request.page.min(pages - 1);
        (take(&active, request.page, PAGE_SIZE), pages)
    };
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

/// One Mini App report: the period total plus every exchange, core, and day on that snapshot.
#[derive(Clone)]
pub(crate) struct MiniReport {
    /// Inclusive window start, unix seconds.
    pub from: i64,
    /// Inclusive window end, unix seconds.
    pub to: i64,
    /// Display zone the day buckets were cut in.
    pub zone: Tz,
    /// Period total over the accessible cores.
    pub total: QuoteBreakdown,
    /// Active exchanges: stable key, caption, money.
    pub by_exchange: Vec<(String, String, QuoteBreakdown)>,
    /// Active cores in Mini App order: id key, caption, exchange section caption, money.
    pub by_core: Vec<(String, String, String, QuoteBreakdown)>,
    /// Every calendar day in the window, including days with no trades.
    pub days: Vec<(String, QuoteBreakdown)>,
    /// Whether the snapshot holds a closed row of the scope past `to`: while it holds none, a
    /// window reaching further on the same day reads exactly these rows. `None` when not asked
    /// (a window with a fixed end) or when the check itself failed.
    pub rows_after_to: Option<bool>,
}

/// Read a Mini App report on one snapshot, the same connection `read_page` uses.
///
/// Owner sees every core (an empty id list). A viewer whose cores do not intersect the
/// snapshot is queried with [`moon_core::config::NO_MATCH_CORE_UID`], so empty does not mean all.
/// Exchange and core rows keep the chat report's activity filter. Days keep every date in the window.
///
/// Args:
///     from: Inclusive window start, unix seconds.
///     to: Inclusive window end, unix seconds.
///     zone: Display zone for the day buckets and the report axis.
///     order: Canonical core order.
///     names: Current configured core names, shown in place of the stored ones.
///     venues: Live venue of each core id.
///     access: Chat grant captured at admission.
///     ends_now: Whether `to` is the instant of the request (Today, Month), so the read also
///         says whether any row lies past it.
///
/// Returns:
///     The four breakdowns, or the database error. A successful empty period is not an error.
#[allow(clippy::too_many_arguments)]
pub(crate) fn read_mini_report(
    from: i64,
    to: i64,
    zone: Tz,
    order: CoreOrder,
    names: &db::CoreNames,
    venues: std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
    access: TelegramReportAccess,
    ends_now: bool,
) -> db::ReadResult<MiniReport> {
    let conn = db::open_reader()?;
    read_mini_report_on(
        &conn,
        from,
        to,
        zone,
        names,
        venues,
        access,
        ends_now,
        |cores| {
            order.sort_by(cores, |(id, _)| *id);
        },
    )
}

/// Connection-injected body of [`read_mini_report`], so fixtures exercise the production query.
#[allow(clippy::too_many_arguments)]
fn read_mini_report_on(
    conn: &rusqlite::Connection,
    from: i64,
    to: i64,
    zone: Tz,
    names: &db::CoreNames,
    venues: std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
    access: TelegramReportAccess,
    ends_now: bool,
    order: impl FnOnce(&mut [(u64, String)]),
) -> db::ReadResult<MiniReport> {
    let snap = db::read_snapshot(conn)?;
    let mut cores = db::distinct_cores(&snap)?;
    relabel(&mut cores, names);
    order(&mut cores);
    if let TelegramReportAccess::Viewer(allowed) = &access {
        cores.retain(|(id, _)| allowed.contains(id));
    }
    let scoped_ids = if access == TelegramReportAccess::Owner {
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
    let mut by_exchange = Vec::new();
    for (venue, members) in core_order::exchange_sections(
        cores
            .iter()
            .enumerate()
            .map(|(index, (id, _))| (index, venues.get(id))),
    ) {
        let mut group = filter.clone();
        group.core_uids = members.iter().map(|&index| cores[index].0).collect();
        let quotes = db::query_totals(&snap, &group)?.quotes;
        if quotes.orders > 0 {
            by_exchange.push((exchange_key(venue), section_label(venue), quotes));
        }
    }
    let mut by_core = Vec::new();
    for (section, (id, name)) in
        crate::mini_app::by_section(cores.clone(), &venues, |(id, _)| *id, |(_, name)| name)
    {
        let mut core = filter.clone();
        core.core_uids = vec![id];
        let quotes = db::query_totals(&snap, &core)?.quotes;
        if quotes.orders > 0 {
            by_core.push((id.to_string(), name, section, quotes));
        }
    }
    let mut days = Vec::new();
    if let (Some(mut date), Some(end)) =
        (display_time::date(from, zone), display_time::date(to, zone))
    {
        let span = (end - date).num_days();
        if !(0..370).contains(&span) {
            return Err(db::ReadFail::failed(
                db::FailKind::Other,
                "telegram report calendar range is invalid",
                moon_core::config::paths::reports_db_path(),
                "telegram: calendar range",
                db::FailCode::None,
            ));
        }
        for _ in 0..=span {
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
                days.push((date.to_string(), db::query_totals(&snap, &day)?.quotes));
            }
            date = next;
        }
    }
    // The complement of the window's upper edge on the same snapshot and axis: `closedate > to`
    // exactly where the window says `closedate <= to`. Asked only for a window ending now, and a
    // failure here is not the report's: it only withholds the answer from later reuse.
    let rows_after_to = ends_now
        .then(|| {
            let mut after = filter.clone();
            after.date_from = Some(to.saturating_add(1));
            after.date_to = None;
            db::query_totals(&snap, &after)
                .ok()
                .map(|totals| totals.quotes.orders > 0)
        })
        .flatten();
    Ok(MiniReport {
        from,
        to,
        zone,
        total,
        by_exchange,
        by_core,
        days,
        rows_after_to,
    })
}

/// Most closed trades the Mini App Trades tab lists.
pub(crate) const MINI_TRADES_LIMIT: usize = 50;

/// One closed trade read for the Mini App, with its dates already on UTC.
pub(crate) struct MiniTrade {
    /// Core that made the trade.
    pub core_uid: u64,
    /// Report row id.
    pub rec_id: i64,
    pub coin: String,
    /// Configured name of the core, or the name stored on the row for a core no longer configured.
    pub core_name: String,
    pub is_short: bool,
    /// Valued profit in USDT, `None` when unvalued.
    pub profit_usdt: Option<f64>,
    /// Profit percent, already x100.
    pub pct: Option<f64>,
    /// Entry time, UTC seconds; `None` when unknown.
    pub buy_utc: Option<i64>,
    /// Close time, UTC seconds.
    pub close_utc: i64,
    pub buy_price: f64,
    pub sell_price: f64,
    pub quantity: f64,
    /// Strategy id; `None` or `0` for a manual trade.
    pub strategy_id: Option<i64>,
    /// Stored `channelname`: the strategy's name when the row was written, empty when absent.
    pub channel_name: String,
}

/// Read the latest closed, non-emulator trades on one snapshot, newest first.
///
/// Scope follows [`read_mini_report`]: the owner reads every core, a viewer only granted cores,
/// and a viewer with no granted core in the snapshot reads [`moon_core::config::NO_MATCH_CORE_UID`].
/// The query over-reads by 25 rows because it sorts by the core-local close date; the rows are then
/// re-sorted on UTC and cut to `limit`.
///
/// Args:
///     zone: Display zone for the report axis.
///     access: Chat grant captured at admission.
///     names: Current configured core names, shown in place of the stored ones.
///     limit: Most trades to return.
///
/// Returns:
///     The trades, or the database error. No closed trades is an empty `Ok`.
pub(crate) fn read_mini_trades(
    zone: Tz,
    access: TelegramReportAccess,
    names: db::CoreNames,
    limit: usize,
) -> db::ReadResult<Vec<MiniTrade>> {
    let conn = db::open_reader()?;
    read_mini_trades_on(&conn, zone, access, names, limit)
}

/// Connection-injected body of [`read_mini_trades`], so fixtures exercise the production query.
fn read_mini_trades_on(
    conn: &rusqlite::Connection,
    zone: Tz,
    access: TelegramReportAccess,
    names: db::CoreNames,
    limit: usize,
) -> db::ReadResult<Vec<MiniTrade>> {
    let snap = db::read_snapshot(conn)?;
    let mut cores = db::distinct_cores(&snap)?;
    if let TelegramReportAccess::Viewer(allowed) = &access {
        cores.retain(|(id, _)| allowed.contains(id));
    }
    let scoped_ids = if access == TelegramReportAccess::Owner {
        Vec::new()
    } else if cores.is_empty() {
        vec![moon_core::config::NO_MATCH_CORE_UID]
    } else {
        cores.iter().map(|(id, _)| *id).collect()
    };
    let filter = ReportFilter {
        core_uids: scoped_ids,
        emulator: Some(false),
        rows: RowScope::Closed,
        axis: db::ReportAxis::load(&snap, zone)?,
        core_names: names,
        ..Default::default()
    };
    let table = db::query_reports(&snap, &filter, "closedate", true, limit + 25)?;
    let index = |name: &str| table.cols.iter().position(|col| col == name);
    let (coin, core_name, is_short, quantity) = (
        index("coin"),
        index("core_name"),
        index("isshort"),
        index("quantity"),
    );
    let (buy_price, sell_price, buy_date, close_date, strategy) = (
        index("buyprice"),
        index("sellprice"),
        index("buydate"),
        index("closedate"),
        index("strategyid"),
    );
    let (rec_id, channel_name) = (index("id"), index("channelname"));
    let (profit, pct) = (
        index(db::VALUATION_PROFIT_COLUMN),
        index(db::PROFIT_PERCENT_COLUMN),
    );
    let mut trades = Vec::with_capacity(table.rows.len());
    for (row_index, row) in table.rows.iter().enumerate() {
        let Some(&core_uid) = table.core_uids.get(row_index) else {
            continue;
        };
        let cell = |ix: Option<usize>| ix.and_then(|ix| row.get(ix));
        let Some(close_local) = cell(close_date)
            .and_then(value_i64)
            .filter(|secs| *secs > 0)
        else {
            continue;
        };
        trades.push(MiniTrade {
            core_uid,
            rec_id: cell(rec_id).and_then(value_i64).unwrap_or_default(),
            coin: cell(coin).map(value_text).unwrap_or_default(),
            core_name: cell(core_name).map(value_text).unwrap_or_default(),
            is_short: cell(is_short).and_then(value_i64).is_some_and(|v| v != 0),
            profit_usdt: cell(profit).and_then(value_f64),
            pct: cell(pct).and_then(value_f64),
            buy_utc: cell(buy_date)
                .and_then(value_i64)
                .filter(|secs| *secs > 0)
                .map(|secs| filter.axis.to_utc(secs, core_uid)),
            close_utc: filter.axis.to_utc(close_local, core_uid),
            buy_price: cell(buy_price).and_then(value_f64).unwrap_or_default(),
            sell_price: cell(sell_price).and_then(value_f64).unwrap_or_default(),
            quantity: cell(quantity).and_then(value_f64).unwrap_or_default(),
            strategy_id: cell(strategy).and_then(value_i64),
            channel_name: cell(channel_name).map(value_text).unwrap_or_default(),
        });
    }
    trades.sort_by_key(|trade| std::cmp::Reverse(trade.close_utc));
    trades.truncate(limit);
    Ok(trades)
}

/// Label each `(uid, stored name)` core with its configured name, as the desktop Report does.
///
/// Args:
///     cores: Cores as `db::distinct_cores` lists them, relabelled in place.
///     names: Current configured core names; an unconfigured uid keeps its stored name.
fn relabel(cores: &mut [(u64, String)], names: &db::CoreNames) {
    for (id, name) in cores {
        *name = names.resolve(*id, name).to_string();
    }
}

/// A stored whole number, accepting a real from an older database.
fn value_i64(value: &rusqlite::types::Value) -> Option<i64> {
    match value {
        rusqlite::types::Value::Integer(i) => Some(*i),
        rusqlite::types::Value::Real(r) => Some(*r as i64),
        _ => None,
    }
}

/// A stored number as `f64`, or `None` for text, blobs and nulls.
fn value_f64(value: &rusqlite::types::Value) -> Option<f64> {
    match value {
        rusqlite::types::Value::Real(r) => Some(*r),
        rusqlite::types::Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// A stored value as text; numbers print in decimal, null is empty.
fn value_text(value: &rusqlite::types::Value) -> String {
    match value {
        rusqlite::types::Value::Text(text) => text.clone(),
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Real(r) => r.to_string(),
        _ => String::new(),
    }
}

/// Stable exchange key shared by the Mini App row and independent of the localized caption.
fn exchange_key(venue: Option<&moon_core::venue::CoreVenue>) -> String {
    match scope_of(venue) {
        moon_core::telegram::report::ReportScope::Venue(id) => {
            format!("{:x}.{:x}", id.code, id.dex)
        }
        moon_core::telegram::report::ReportScope::Unidentified
        | moon_core::telegram::report::ReportScope::All => "unidentified".to_string(),
    }
}

/// Exchange buttons always list every active venue the chat can see, including from a scoped view.
fn exchange_drilldowns(
    snap: &rusqlite::Transaction<'_>,
    cores: &[(u64, String)],
    venues: &std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
    filter: &ReportFilter,
) -> db::ReadResult<Vec<(String, ReportScope)>> {
    let mut drilldowns = Vec::new();
    for (venue, members) in core_order::exchange_sections(
        cores
            .iter()
            .enumerate()
            .map(|(index, (id, _))| (index, venues.get(id))),
    ) {
        let mut group = filter.clone();
        group.core_uids = members.iter().map(|&index| cores[index].0).collect();
        let total = db::query_totals(snap, &group)?.quotes;
        if total.orders > 0 {
            drilldowns.push((section_label(venue), scope_of(venue)));
        }
    }
    Ok(drilldowns)
}

/// Resolve exactly the same venue identity used by the terminal's core lists.
fn scope_of(venue: Option<&moon_core::venue::CoreVenue>) -> ReportScope {
    match core_order::section_of(venue) {
        core_order::ExchangeSection::Unidentified => ReportScope::Unidentified,
        core_order::ExchangeSection::Venue(id) => ReportScope::Venue(id),
    }
}

#[cfg(test)]
mod tests;
