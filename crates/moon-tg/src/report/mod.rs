//! Localized rich reports over the same snapshot, time axis, and money reader as Report.
use chrono::Days;
use chrono_tz::Tz;
use moon_core::session::core_order::{self, CoreOrder};
use moon_core::{
    config::{telegram_access::TelegramReportAccess, telegram_menu::ReportBasis},
    db::{self, QuoteBreakdown, ReportFilter, RowScope},
    telegram::{
        report::{ReportRequest, ReportScope},
        runtime::Response,
    },
    util::display_time,
};
use rust_i18n::t;
use std::collections::BTreeSet;
use std::sync::mpsc::SyncSender;

use crate::notify::trades::ClosedTrade;

use crate::TgHost;
use crate::labels::{answer, navigation_keyboard, section_label};
use moon_core::telegram::api::ReplyMarkup;

mod paging;
mod render;

pub(crate) use render::{escape, help, rich_message_fits};
use render::{render, report_html};

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
    /// Which timestamp the period was read on.
    basis: ReportBasis,
    /// Cores the chat may see, whose trades the totals can include.
    cores: Vec<u64>,
    /// A line above the report: an automatic report names itself, its zone and its basis.
    caption: Option<String>,
}

/// Read off the owner thread and recheck the saved chat authorization before returning any money.
///
/// A request whose view the chat did not pick opens in the bot's `report_view`; a request that does
/// not carry its basis yet is read on the bot's `period_basis`.
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
    let bot = &host.config().telegram.bot;
    let request = request.resolve_view(bot.report_view);
    // A page's own buttons carry the basis it was read on; a fresh request takes the bot's.
    let basis = request.basis.unwrap_or(bot.period_basis);
    let navigation = navigation_keyboard(host.kind(), owner, &host.config().telegram);
    if matches!(&access, TelegramReportAccess::Viewer(ids) if ids.is_empty()) {
        report_notice(
            &reply,
            t!("telegram.access_no_cores").to_string(),
            navigation,
        );
        return;
    }
    if host.state().report_pending {
        report_notice(&reply, t!("telegram.report_busy").to_string(), navigation);
        return;
    }
    let zone = host.report_zone();
    let now = moon_core::util::time::now_unix_secs() as i64;
    let Some((from, to)) = request.bounds(now, zone) else {
        report_notice(&reply, crate::labels::report_help(host.kind()), navigation);
        return;
    };
    let order = CoreOrder::new(host.config());
    let names = db::CoreNames::from_servers(&host.config().servers);
    let venues = host.session().core_venues().clone();
    let read_access = access.clone();
    host.state_mut().report_pending = true;
    host.spawn(Box::new(move || {
        let result = read_page(
            request,
            from,
            to,
            zone,
            basis,
            order,
            names,
            venues,
            read_access,
        );
        Box::new(move |host: &mut dyn TgHost| {
            host.state_mut().report_pending = false;
            if host.config().telegram.report_access(chat).as_ref() != Some(&access) {
                answer(&reply, t!("telegram.refusal").to_string());
                return;
            }
            // The navigation as it is now: the menu may have changed during the read.
            let navigation = navigation_keyboard(host.kind(), owner, &host.config().telegram);
            match result {
                Ok(page) => {
                    let _ = reply.try_send(render(&page, host.kind(), navigation));
                }
                Err(_) => {
                    report_notice(&reply, t!("telegram.report_failed").to_string(), navigation)
                }
            }
        })
    }));
}

/// A report notice exposes navigation limited to the admission grant, including on /start.
fn report_notice(reply: &SyncSender<Response>, text: String, navigation: ReplyMarkup) {
    let _ = reply.try_send(Response::Text {
        text,
        keyboard: Some(navigation),
    });
}

/// Read only visible groups, while the headline always covers the entire requested period.
#[allow(clippy::too_many_arguments)]
fn read_page(
    request: ReportRequest,
    from: i64,
    to: i64,
    zone: Tz,
    basis: ReportBasis,
    order: CoreOrder,
    names: db::CoreNames,
    venues: std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
    access: TelegramReportAccess,
) -> db::ReadResult<Page> {
    let conn = db::open_reader()?;
    read_page_on(&conn, request, from, to, zone, basis, &names, |cores| {
        order.sort_by(cores, |(id, _)| *id);
        (venues, access)
    })
}

/// Connection-injected reader lets fixtures exercise the exact production query contract.
#[allow(clippy::too_many_arguments)]
fn read_page_on(
    conn: &rusqlite::Connection,
    mut request: ReportRequest,
    from: i64,
    to: i64,
    zone: Tz,
    basis: ReportBasis,
    names: &db::CoreNames,
    order: impl FnOnce(
        &mut [(u64, String)],
    ) -> (
        std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
        TelegramReportAccess,
    ),
) -> db::ReadResult<Page> {
    request.window = Some((from, to));
    request.basis = Some(basis);
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
        period_basis: basis.period_basis(),
        axis: db::ReportAxis::load(&snap, zone)?,
        ..Default::default()
    };
    let total = db::query_totals(&snap, &filter)?.quotes;
    let mut groups = Vec::new();
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
        }
    } else {
        for (id, name) in cores {
            let mut core = filter.clone();
            core.core_uids = vec![id];
            groups.push((name, core));
        }
    }
    // Filter by actual activity before paging, retaining zero-PnL trades and native-only money.
    let mut active = Vec::new();
    for (name, filter) in groups {
        let total = db::query_totals(&snap, &filter)?.quotes;
        if total.orders > 0 {
            active.push((name, total));
        }
    }
    let drilldowns = exchange_drilldowns(&snap, &accessible, &venues, &filter)?;
    // Every view shows all its rows while the message fits; an oversized one pages with the
    // largest ladder rung that fits (`paging`). A probe of a partial page carries the longest page
    // label it can show, so the page actually rendered is never longer than the one measured.
    let fits = |rows: &[(String, QuoteBreakdown)]| {
        let partial = rows.len() < active.len();
        let mut probe = request.clone();
        probe.page = if partial { 9_999 } else { 0 };
        rich_message_fits(&report_html(&Page {
            request: probe,
            from,
            to,
            zone,
            total: total.clone(),
            rows: rows.to_vec(),
            pages: if partial { 10_000 } else { 1 },
            drilldowns: drilldowns.clone(),
            scope_label: scope_label.clone(),
            basis,
            cores: Vec::new(),
            caption: None,
        }))
    };
    let size = paging::fitting_page_size(&active, fits);
    let pages = active.len().div_ceil(size).max(1);
    request.page = request.page.min(pages - 1);
    let rows: Vec<_> = active
        .iter()
        .skip(request.page * size)
        .take(size)
        .cloned()
        .collect();
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
        basis,
        cores: accessible.iter().map(|(id, _)| *id).collect(),
        caption: None,
    })
}

/// An automatic report ready to queue: the rich HTML, its buttons, the cores it discloses.
pub(crate) struct AutoPage {
    pub(crate) html: String,
    pub(crate) keyboard: ReplyMarkup,
    /// A viewer's report discloses its granted cores. An owner's covers every core the report
    /// database holds, retired ones included, so it names none: `None` is kept while the chat is
    /// the owner and dropped if it becomes a viewer.
    pub(crate) cores: Option<Vec<u64>>,
}

/// What every automatic report of one read shares.
#[derive(Clone)]
pub(crate) struct AutoInputs {
    pub(crate) zone: Tz,
    pub(crate) basis: ReportBasis,
    pub(crate) view: moon_core::config::telegram_menu::ReportView,
    pub(crate) order: CoreOrder,
    pub(crate) names: db::CoreNames,
    pub(crate) venues: std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
}

/// Read one automatic report: the report a button opens, over the slot's frozen period, in the
/// bot's view and basis, with `caption` above it.
///
/// Returns:
///     `None` for a viewer with no cores, which has nothing to see, and for a report that does
///     not fit a rich message even without its caption (logged). The caller spends the slot.
pub(crate) fn read_auto_report(
    conn: &rusqlite::Connection,
    window: &moon_core::telegram::report::AutoWindow,
    caption: String,
    inputs: &AutoInputs,
    access: &TelegramReportAccess,
) -> db::ReadResult<Option<AutoPage>> {
    if matches!(access, TelegramReportAccess::Viewer(ids) if ids.is_empty()) {
        return Ok(None);
    }
    let request = ReportRequest::new(window.period.clone(), false).in_view(inputs.view);
    let mut page = read_page_on(
        conn,
        request,
        window.from,
        window.to,
        inputs.zone,
        inputs.basis,
        &inputs.names,
        |cores| {
            inputs.order.sort_by(cores, |(id, _)| *id);
            (inputs.venues.clone(), access.clone())
        },
    )?;
    page.caption = Some(caption);
    let mut html = report_html(&page);
    // The page was sized without the caption; a report it tips over goes without it.
    if !rich_message_fits(&html) {
        page.caption = None;
        html = report_html(&page);
    }
    if !rich_message_fits(&html) {
        log::warn!("telegram auto report does not fit a rich message; not sent");
        return Ok(None);
    }
    let cores = match access {
        TelegramReportAccess::Owner => None,
        TelegramReportAccess::Viewer(_) => Some(page.cores.clone()),
    };
    Ok(Some(AutoPage {
        html,
        keyboard: render::keyboard(&page),
        cores,
    }))
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
    // Every figure the page shows is one slice of `filter`: the period, each exchange, each core,
    // each day. One sliced read sums them all from a single pass over the period's rows, each
    // exactly what its own `query_totals` would state.
    let whole = |core_uids: Option<Vec<u64>>| db::TotalsSlice {
        core_uids,
        date_from: Some(from),
        date_to: Some(to),
    };
    let mut slices = vec![whole(None)];
    let exchanges = core_order::exchange_sections(
        cores
            .iter()
            .enumerate()
            .map(|(index, (id, _))| (index, venues.get(id))),
    )
    .into_iter()
    .map(|(venue, members)| {
        slices.push(whole(Some(
            members.iter().map(|&index| cores[index].0).collect(),
        )));
        venue
    })
    .collect::<Vec<_>>();
    let sections =
        crate::mini_app::by_section(cores.clone(), &venues, |(id, _)| *id, |(_, name)| name);
    for (_, (id, _)) in &sections {
        slices.push(whole(Some(vec![*id])));
    }
    let mut day_dates = Vec::new();
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
                slices.push(db::TotalsSlice {
                    core_uids: None,
                    date_from: Some(start.max(from)),
                    date_to: Some((stop - 1).min(to)),
                });
                day_dates.push(date.to_string());
            }
            date = next;
        }
    }
    let mut sliced = db::query_totals_sliced(&snap, &filter, &slices)?
        .into_iter()
        .map(|totals| totals.quotes);
    let total = sliced.next().unwrap_or_default();
    let mut by_exchange = Vec::new();
    for (venue, quotes) in exchanges.into_iter().zip(sliced.by_ref()) {
        if quotes.orders > 0 {
            by_exchange.push((exchange_key(venue), section_label(venue), quotes));
        }
    }
    let mut by_core = Vec::new();
    for ((section, (id, name)), quotes) in sections.into_iter().zip(sliced.by_ref()) {
        if quotes.orders > 0 {
            by_core.push((id.to_string(), name, section, quotes));
        }
    }
    let days = day_dates.into_iter().zip(sliced).collect::<Vec<_>>();
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

/// One closed Mini App trade, with UTC dates and the Report-gated entry-volume inputs.
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
    /// Entry quantity, distinct from exit quantity that can include a spot wallet top-up.
    pub bought_quantity: Option<f64>,
    /// Report valuation rate, withheld unless entry notional can safely share the money quote.
    pub entry_volume_rate: Option<f64>,
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

/// Read Mini App rows and safe entry-volume inputs on the same snapshot as their valued profit.
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
    let table = db::query_mini_trades(&snap, &filter, limit + 25)?;
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
    let bought_quantity = index("boughtq");
    let entry_volume_rate = index(db::MINI_ENTRY_VOLUME_RATE_COLUMN);
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
            bought_quantity: cell(bought_quantity).and_then(value_f64),
            entry_volume_rate: cell(entry_volume_rate).and_then(value_f64),
            strategy_id: cell(strategy).and_then(value_i64),
            channel_name: cell(channel_name).map(value_text).unwrap_or_default(),
        });
    }
    trades.sort_by_key(|trade| std::cmp::Reverse(trade.close_utc));
    trades.truncate(limit);
    Ok(trades)
}

/// Closed rows fetched per core on one notification page.
const NOTIFY_READ_PAGE: usize = 32;
/// Largest page before an equal-timestamp stampede steps the window back one second.
const NOTIFY_READ_PAGE_CAP: usize = 1_048_576;

/// Read closed trades with `close_utc >= from_utc`, paging each replica core separately.
///
/// There is no UI row limit. At [`NOTIFY_READ_PAGE_CAP`], a saturated equal-timestamp page
/// steps back one second and can omit additional rows at that timestamp.
/// A window that crosses a core clock-offset change can admit or drop a
/// trade within one offset of the edge, because the SQL bound uses each core's offset at now.
///
/// Args:
///     zone: Display zone for the report axis.
///     names: Current configured core names, shown in place of the stored ones.
///     from_utc: Inclusive lower bound, true UTC seconds.
///
/// Returns:
///     The trades, oldest close first, or the database error. No rows is an empty `Ok`.
///
/// Errors:
///     The report replica could not be opened or read.
pub(crate) fn read_closed_since(
    zone: Tz,
    names: db::CoreNames,
    from_utc: i64,
) -> db::ReadResult<Vec<ClosedTrade>> {
    let conn = db::open_reader()?;
    read_closed_since_on(&conn, zone, &names, from_utc)
}

/// [`read_closed_since`] on an already open connection.
///
/// Each core is paged on its own: one cross-core `(close_utc, rec_id)` cursor is unsafe, because
/// `LIMIT` can drop a later close on another core.
fn read_closed_since_on(
    conn: &rusqlite::Connection,
    zone: Tz,
    names: &db::CoreNames,
    from_utc: i64,
) -> db::ReadResult<Vec<ClosedTrade>> {
    let snap = db::read_snapshot(conn)?;
    let cores = db::distinct_cores(&snap)?;
    let axis = db::ReportAxis::load(&snap, zone)?;
    let mut trades = Vec::new();
    for (core, _) in cores {
        trades.extend(page_closed_core(&snap, &axis, names, core, from_utc)?);
    }
    trades.sort_by(|left, right| {
        left.close_utc
            .cmp(&right.close_utc)
            .then(left.rec_id.cmp(&right.rec_id))
    });
    Ok(trades)
}

/// SQL window and page size for one core.
struct PageCursor {
    window_to: Option<i64>,
    limit: usize,
}

impl PageCursor {
    /// Start at the newest row with the small page size.
    fn start() -> Self {
        Self {
            window_to: None,
            limit: NOTIFY_READ_PAGE,
        }
    }

    /// `true` once the inclusive upper bound has moved before `from_utc`.
    fn exhausted(&self, from_utc: i64) -> bool {
        self.window_to.is_some_and(|to| to < from_utc)
    }

    /// Move the window after one page. `false` means stop.
    ///
    /// A short page is done. A full page with no accepted close is done, because older rows are
    /// earlier. No new row doubles the limit, then steps the bound back one second at the cap.
    fn advance(&mut self, raw_len: usize, boundary: Option<i64>, added: usize) -> bool {
        if raw_len < self.limit {
            return false;
        }
        let Some(boundary) = boundary else {
            return false;
        };
        if added == 0 {
            return self.grow_or_step(boundary);
        }
        self.window_to = Some(boundary);
        true
    }

    /// Widen the page, or step one second earlier once the page is already at the cap.
    fn grow_or_step(&mut self, boundary: i64) -> bool {
        if self.limit < NOTIFY_READ_PAGE_CAP {
            self.limit = self.limit.saturating_mul(2).min(NOTIFY_READ_PAGE_CAP);
            return true;
        }
        let next = boundary.saturating_sub(1);
        if self.window_to == Some(next) {
            return false;
        }
        self.window_to = Some(next);
        self.limit = NOTIFY_READ_PAGE;
        true
    }
}

/// Page one core until its closed rows in the window are collected.
fn page_closed_core(
    snap: &rusqlite::Transaction<'_>,
    axis: &db::ReportAxis,
    names: &db::CoreNames,
    core: u64,
    from_utc: i64,
) -> db::ReadResult<Vec<ClosedTrade>> {
    let mut cursor = PageCursor::start();
    let mut seen = BTreeSet::new();
    let mut trades = Vec::new();
    while !cursor.exhausted(from_utc) {
        let filter = closed_filter(axis, names, core, from_utc, cursor.window_to);
        let table = db::query_notify_trades(snap, &filter, cursor.limit)?;
        let cols = ClosedCols::from_table(&table);
        let mut boundary: Option<i64> = None;
        let mut added = 0usize;
        for (row_index, row) in table.rows.iter().enumerate() {
            let Some(trade) = map_closed_row(&table, row, row_index, &cols, axis) else {
                continue;
            };
            if trade.close_utc < from_utc {
                continue;
            }
            boundary = Some(boundary.map_or(trade.close_utc, |low| low.min(trade.close_utc)));
            if seen.insert((trade.core, trade.rec_id)) {
                added += 1;
                trades.push(trade);
            }
        }
        if !cursor.advance(table.rows.len(), boundary, added) {
            break;
        }
    }
    Ok(trades)
}

/// Report filter for one core's closed, non-emulator rows inside the SQL window.
fn closed_filter(
    axis: &db::ReportAxis,
    names: &db::CoreNames,
    core: u64,
    from_utc: i64,
    window_to: Option<i64>,
) -> ReportFilter {
    ReportFilter {
        core_uids: vec![core],
        date_from: Some(from_utc),
        date_to: window_to,
        emulator: Some(false),
        rows: RowScope::Closed,
        axis: axis.clone(),
        core_names: names.clone(),
        ..Default::default()
    }
}

/// Column indexes used to map one notification row.
struct ClosedCols {
    coin: Option<usize>,
    core_name: Option<usize>,
    buy_price: Option<usize>,
    buy_date: Option<usize>,
    close_date: Option<usize>,
    rec_id: Option<usize>,
    channel: Option<usize>,
    bought: Option<usize>,
    rate: Option<usize>,
    profit: Option<usize>,
    pct: Option<usize>,
    profit_native: Option<usize>,
    volume_native: Option<usize>,
    quote: Option<usize>,
}

impl ClosedCols {
    /// Resolve the columns this page actually returned.
    fn from_table(table: &db::ReportTable) -> Self {
        let index = |name: &str| table.cols.iter().position(|col| col == name);
        Self {
            coin: index("coin"),
            core_name: index("core_name"),
            buy_price: index("buyprice"),
            buy_date: index("buydate"),
            close_date: index("closedate"),
            rec_id: index("id"),
            channel: index("channelname"),
            bought: index("boughtq"),
            rate: index(db::MINI_ENTRY_VOLUME_RATE_COLUMN),
            profit: index(db::VALUATION_PROFIT_COLUMN),
            pct: index(db::PROFIT_PERCENT_COLUMN),
            profit_native: index(db::NOTIFY_PROFIT_NATIVE_COLUMN),
            volume_native: index(db::NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN),
            quote: index(db::NOTIFY_QUOTE_COLUMN),
        }
    }
}

/// Map one report row. A missing buy time uses the close time, so the duration is zero.
///
/// `rec_id` is the replica `newrecid`. A legacy `0` falls back to the display `id` column.
fn map_closed_row(
    table: &db::ReportTable,
    row: &[rusqlite::types::Value],
    row_index: usize,
    cols: &ClosedCols,
    axis: &db::ReportAxis,
) -> Option<ClosedTrade> {
    let core = *table.core_uids.get(row_index)?;
    let cell = |ix: Option<usize>| ix.and_then(|ix| row.get(ix));
    let close_local = cell(cols.close_date)
        .and_then(value_i64)
        .filter(|secs| *secs > 0)?;
    let close_utc = axis.to_utc(close_local, core);
    let buy_utc = cell(cols.buy_date)
        .and_then(value_i64)
        .filter(|secs| *secs > 0)
        .map(|secs| axis.to_utc(secs, core));
    let replica = table.rec_ids.get(row_index).copied().unwrap_or(0);
    let rec_id = if replica != 0 {
        replica
    } else {
        cell(cols.rec_id).and_then(value_i64).unwrap_or(0)
    };
    Some(ClosedTrade {
        core,
        rec_id,
        close_utc,
        coin: cell(cols.coin).map(value_text).unwrap_or_default(),
        core_name: cell(cols.core_name).map(value_text).unwrap_or_default(),
        strategy: cell(cols.channel).map(value_text).unwrap_or_default(),
        volume_usd: entry_volume_usd(
            cell(cols.bought).and_then(value_f64),
            cell(cols.buy_price).and_then(value_f64),
            cell(cols.rate).and_then(value_f64),
        ),
        profit_usd: finite_number(cell(cols.profit).and_then(value_f64)),
        profit_pct: finite_number(cell(cols.pct).and_then(value_f64)),
        quote: cell(cols.quote)
            .and_then(value_i64)
            .and_then(db::QuoteCurrency::from_report_ordinal),
        profit_native: finite_number(cell(cols.profit_native).and_then(value_f64)),
        volume_native: finite_number(cell(cols.volume_native).and_then(value_f64))
            .filter(|volume| *volume > 0.0),
        open_utc: buy_utc.unwrap_or(close_utc),
    })
}

/// Entry notional in USDT using the Report-gated rate; any missing, non-finite, or non-positive
/// input or product is `None`. The notification rule's USD-named thresholds use this amount.
fn entry_volume_usd(bought: Option<f64>, price: Option<f64>, rate: Option<f64>) -> Option<f64> {
    let bought = bought.filter(|value| value.is_finite() && *value > 0.0)?;
    let price = price.filter(|value| value.is_finite() && *value > 0.0)?;
    let rate = rate.filter(|value| value.is_finite() && *value > 0.0)?;
    let product = bought * price * rate;
    (product.is_finite() && product > 0.0).then_some(product)
}

/// Keep a finite number. `NaN` and infinities are unvalued.
fn finite_number(value: Option<f64>) -> Option<f64> {
    value.filter(|number| number.is_finite())
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
