//! Mapping the session's and the replica's rows onto the Mini App's JSON.

use chrono_tz::Tz;
use moon_core::db::QuoteBreakdown;
use moon_core::feed::order_math::{
    MONEY_DECIMALS, order_pnl, order_pnl_pct, pct_to_entry, position_qty,
};
use moon_core::feed::{ConnStatus, OrderRow};
use moon_core::session::BalanceState;
use moon_core::telegram::report::Period;
use moon_core::telegram::web::MiniAppApiError;
use moon_core::telegram::web::dto::{
    BalanceStateDto, CommandErrorDto, CommandResultDto, ConnDto, DayDto, MoneyDto, OrderDto,
    ReportDto, ReportPeriodDto, RowDto, StrategyPendingDto, TradeDto,
};
use moon_core::util::{display_time, fmt};
use std::time::{Duration, Instant};

use crate::TgHost;
use crate::labels::section_label;
use crate::report::{MiniReport, MiniTrade};

/// How long a strategy toggle stays `Pending` before it is reported as `TimedOut`.
const STRATEGY_CONFIRM_WINDOW: Duration = Duration::from_secs(45);

/// Map a stored order into the Mini App row.
///
/// The change percent uses the desktop orders table's arithmetic and precision.
/// Adaptive price text cannot be parsed back into that percent.
/// Entry volume uses the market quote and is absent for unusable entry inputs.
pub(super) fn order_dto(
    host: &dyn TgHost,
    id: u64,
    name: String,
    exchange: String,
    order: &OrderRow,
) -> OrderDto {
    let pnl = order_pnl(order).filter(|value| value.is_finite());
    let quote = order_quote(&order.quote, &order.market);
    let quote = quote.as_str();
    let market_source = host.session().market_source();
    let change = order_pnl_pct(order).filter(|value| value.is_finite());
    let to_entry = order_to_entry_pct(order);
    OrderDto {
        core: id,
        core_name: name,
        exchange,
        uid: order.uid,
        coin: order.coin.clone(),
        market: order.market.clone(),
        side: if order.is_short { "sell" } else { "buy" }.to_string(),
        volume_text: entry_volume_text(order.size, order.buy_price, 1.0, &order.quote),
        entry_text: price_text(order.buy_price),
        mark_text: price_text(f64::from(order.price)),
        pnl,
        pnl_text: pnl.and_then(|value| pnl_text(value, quote)),
        pnl_sign: pnl.map_or(0, |value| pnl_sign(value, quote)),
        pnl_usd: pnl.and_then(|value| {
            pnl_usd(value, quote, |currency| {
                market_source.currency_usd_rate(id, currency)
            })
        }),
        emulator: order.emulator,
        change_pct: change,
        change_text: change
            .and_then(|value| fmt::signed_pct(value, MONEY_DECIMALS).map(|(text, _)| text)),
        to_entry_pct: to_entry,
        to_entry_text: to_entry.and_then(distance_text),
        panic_armed: host.is_panic_armed(id, &order.market),
    }
}

/// The currency an order's PnL is counted in: the catalog quote, else the quote the market name
/// spells, else USDC.
///
/// The last step is the terminal's own rule (`MarketDataSource::quote_usd_rate`): a market with no
/// quote anywhere is a Hyperliquid one — a linear perp named after its coin (`BTC`) or a HIP-3
/// perp (`xyz:BIRD`) — and both settle in USDC. This is the ONE source for both the unit printed
/// beside the PnL and the rate that converts it, so the two never describe different currencies.
pub(super) fn order_quote(quote: &str, market: &str) -> String {
    if !quote.is_empty() {
        return quote.to_string();
    }
    let named = moon_core::symbol::resolve_quote(market);
    if named.is_empty() {
        "USDC".to_string()
    } else {
        named
    }
}

/// Decimals of `quote`: cents for a USD stablecoin or an unknown quote, a coin's precision
/// otherwise — two decimals would round a BTC PnL to zero.
fn pnl_decimals(quote: &str) -> usize {
    if quote.is_empty() || moon_core::symbol::is_usd_stable(quote) {
        MONEY_DECIMALS
    } else {
        COIN_DECIMALS
    }
}

/// Signed open PnL with its unit, in [`order_quote`]'s currency: `$` for a USD stablecoin, the
/// ticker for a coin, a bare number when the quote is unknown.
pub(super) fn pnl_text(value: f64, quote: &str) -> Option<String> {
    let (text, _) = fmt::signed_fixed(value, pnl_decimals(quote))?;
    Some(if quote.is_empty() {
        text
    } else if moon_core::symbol::is_usd_stable(quote) {
        format!("{text}$")
    } else {
        format!("{text} {quote}")
    })
}

/// Sign of [`pnl_text`] as printed: -1, 0 or 1, zero when the value rounds to zero.
pub(super) fn pnl_sign(value: f64, quote: &str) -> i8 {
    match fmt::signed_fixed(value, pnl_decimals(quote)) {
        Some((_, fmt::DeltaSign::Positive)) => 1,
        Some((_, fmt::DeltaSign::Negative)) => -1,
        _ => 0,
    }
}

/// Open PnL in dollars: as is for a USD stablecoin, through `rate` for any other quote, `None`
/// for an unknown quote or rate.
pub(super) fn pnl_usd(
    value: f64,
    quote: &str,
    rate: impl FnOnce(&str) -> Option<f64>,
) -> Option<f64> {
    if quote.is_empty() {
        return None;
    }
    let usd = if moon_core::symbol::is_usd_stable(quote) {
        value
    } else {
        value * rate(quote)?
    };
    usd.is_finite().then_some(usd)
}

/// Decimals of a PnL quoted in a coin such as BTC: a cent-style two would round it to zero.
const COIN_DECIMALS: usize = 8;

/// Unsigned text of a distance to entry: a "+" beside a resting order reads as profit, and which
/// way the mark has to travel is the side's business, not the figure's.
pub(super) fn distance_text(pct: f64) -> Option<String> {
    fmt::pct(pct.abs(), MONEY_DECIMALS).map(|(text, _)| text)
}

/// Distance from the current mark to the entry of an order that holds no position yet.
///
/// A resting entry has no PnL, so the row states how far the price still has to travel to fill
/// it, with the arithmetic Moonbot's price-approach alert uses ([`pct_to_entry`]). Negative once
/// the mark has passed the entry.
///
/// Args:
///     order: Stored order row.
///
/// Returns:
///     The percent, or `None` once the order holds a position or a price is unusable.
pub(super) fn order_to_entry_pct(order: &OrderRow) -> Option<f64> {
    if position_qty(order).is_some() {
        return None;
    }
    pct_to_entry(order, f64::from(order.price)).filter(|value| value.is_finite())
}

/// Map a read closed trade into the Mini App row.
///
/// Args:
///     host: Source of venues and live strategy names.
///     zone: Display zone for the close time.
///     now_secs: Current UTC Unix seconds; decides whether the short close time drops the date.
///     trade: Trade read by [`crate::report::read_mini_trades`].
///
/// Returns:
///     The row, its strategy attributed by [`trade_strategy`].
///     Entry volume uses bought quantity and the Report-gated rate, never exit quantity.
pub(super) fn trade_dto(host: &dyn TgHost, zone: Tz, now_secs: i64, trade: &MiniTrade) -> TradeDto {
    let profit = trade.profit_usdt.filter(|value| value.is_finite());
    let pct = trade.pct.filter(|value| value.is_finite());
    let (strategy, manual) = trade_strategy(trade.strategy_id, &trade.channel_name, |sid| {
        host.session()
            .store()
            .core(trade.core_uid)
            .and_then(|data| data.strategies.iter().find(|row| row.id == sid))
            .map(|row| row.name.clone())
    });
    TradeDto {
        core: trade.core_uid,
        core_name: trade.core_name.clone(),
        exchange: section_label(host.session().core_venues().get(&trade.core_uid)),
        rec_id: trade.rec_id,
        coin: trade.coin.clone(),
        side: if trade.is_short { "sell" } else { "buy" }.to_string(),
        profit,
        profit_text: profit.map(signed_dollars),
        profit_pct: pct,
        profit_pct_text: pct
            .and_then(|value| fmt::signed_pct(value, MONEY_DECIMALS).map(|(text, _)| text)),
        closed_at: trade.close_utc,
        closed_text: display_time::format_minute(trade.close_utc, zone),
        closed_short_text: display_time::format_short_minute(trade.close_utc, zone, now_secs),
        entry_text: price_text(trade.buy_price),
        exit_text: price_text(trade.sell_price),
        qty_text: fmt::qty(trade.quantity),
        volume_text: trade_volume_text(trade),
        duration_secs: trade
            .buy_utc
            .map(|buy| trade.close_utc - buy)
            .filter(|secs| *secs >= 0),
        strategy,
        manual,
    }
}

/// Closed entry notional in USDT, using bought quantity and only a Report-approved rate.
pub(super) fn trade_volume_text(trade: &MiniTrade) -> Option<String> {
    trade
        .bought_quantity
        .zip(trade.entry_volume_rate)
        .and_then(|(qty, rate)| entry_volume_text(qty, trade.buy_price, rate, "USDT"))
}

/// Format a positive finite entry notional, omitting unavailable inputs instead of inventing zero.
///
/// `rate` converts closed trades to USDT; open orders use 1 and their native market quote.
/// Known USD/USDT quotes use dollars. An empty quote means unavailable market metadata and
/// cannot establish a currency; known COIN-M names already resolve to USD in the feed.
/// Other quotes retain adaptive precision so small BTC positions do not round to zero.
pub(super) fn entry_volume_text(qty: f64, entry: f64, rate: f64, quote: &str) -> Option<String> {
    if [qty, entry, rate]
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return None;
    }
    let amount = qty * entry * rate;
    if !amount.is_finite() || amount <= 0.0 {
        return None;
    }
    let quote = quote.trim();
    if quote.is_empty() {
        return None;
    }
    if quote.eq_ignore_ascii_case("USD") || quote.eq_ignore_ascii_case("USDT") {
        Some(format!("{}$", fmt::usd_grouped(amount)))
    } else {
        Some(format!(
            "{} {quote}",
            fmt::group_decimal(&fmt::adaptive(amount))
        ))
    }
}

/// Attribute a closed trade to its strategy the way the desktop Report does.
///
/// The stored id is Delphi-signed: a strategy whose id has the top bit set is stored negative, so
/// it is reinterpreted `as u64` exactly like the Report's own lookup rather than dropped.
///
/// Args:
///     strategy_id: The row's `strategyid`; `None` when the row carries none.
///     stored: The row's own `channelname`, the name the Report shows for a strategy the core no
///         longer lists.
///     name: Live name of a strategy id on the trade's core, `None` when the core does not list it.
///
/// Returns:
///     `(name, manual)`: the live name, else the stored one, else `#<id>`; `(None, true)` for `0`
///     (manual), and `(None, false)` when the id is unknown.
pub(super) fn trade_strategy(
    strategy_id: Option<i64>,
    stored: &str,
    name: impl FnOnce(u64) -> Option<String>,
) -> (Option<String>, bool) {
    match strategy_id {
        None => (None, false),
        Some(0) => (None, true),
        Some(sid) => {
            let sid = sid as u64;
            let stored = stored.trim();
            let shown = name(sid)
                .or_else(|| (!stored.is_empty()).then(|| stored.to_string()))
                .unwrap_or_else(|| format!("#{sid}"));
            (Some(shown), false)
        }
    }
}

/// Unconfirmed state of one strategy toggle, or `None` once it is settled.
///
/// Settled means the core acknowledged a checkbox delta, the strategy list was rebuilt, and the
/// row shows the asked state. After [`STRATEGY_CONFIRM_WINDOW`] an unconfirmed toggle is
/// `TimedOut` until the row shows the asked state or the core sends a fresh strategy list
/// (`strategies_rev` moved), whose state is then the truth.
///
/// Args:
///     entry: `(wanted, sent_at, ack_rev before, strategies_rev before)` recorded at send.
///     ack_now: Current `strategies_ack_rev` of the core.
///     rev_now: Current `strategies_rev` of the core.
///     checked: Current checked state of the row.
///     now: Current instant.
///
/// Returns:
///     `Pending`, `TimedOut`, or `None` when the entry should be dropped.
pub(super) fn strategy_pending(
    entry: (bool, Instant, u64, u64),
    ack_now: u64,
    rev_now: u64,
    checked: bool,
    now: Instant,
) -> Option<StrategyPendingDto> {
    let (wanted, sent_at, ack_before, rev_before) = entry;
    if ack_now != ack_before && rev_now != rev_before && checked == wanted {
        return None;
    }
    if now.saturating_duration_since(sent_at) < STRATEGY_CONFIRM_WINDOW {
        return Some(StrategyPendingDto::Pending);
    }
    if checked == wanted || rev_now != rev_before {
        return None;
    }
    Some(StrategyPendingDto::TimedOut)
}

/// `Period` values the chat buttons Today, Yesterday, Month, and Last month already use.
pub(super) fn report_period(period: ReportPeriodDto) -> Period {
    match period {
        ReportPeriodDto::Today => Period::Today,
        ReportPeriodDto::Yesterday => Period::Yesterday,
        ReportPeriodDto::Month => Period::Month,
        ReportPeriodDto::LastMonth => Period::LastMonth,
    }
}

/// A money command the core accepted. `armed` is set only for Panic Sell.
///
/// Args:
///     armed: Panic Sell state after the call, or `None` for cancel.
///
/// Returns:
///     `ok: true` and no error.
pub(super) fn command_hit(armed: Option<bool>) -> CommandResultDto {
    CommandResultDto {
        ok: true,
        armed,
        error: None,
    }
}

/// A money command that did not run or was not accepted. Nothing about `armed` is claimed.
///
/// Args:
///     error: Why the command stopped.
///
/// Returns:
///     `ok: false` with that error and `armed: None`.
pub(super) fn command_miss(error: CommandErrorDto) -> CommandResultDto {
    CommandResultDto {
        ok: false,
        armed: None,
        error: Some(error),
    }
}

/// JSON report. Money text follows the chat report's signed dollars, with `None` when unvalued.
///
/// Returns:
///     The report, or `ReadFailed` when a window edge has no civil date.
pub(super) fn report_dto(report: MiniReport) -> Result<ReportDto, MiniAppApiError> {
    let zone = report.zone;
    let from = iso_day(report.from, zone).ok_or(MiniAppApiError::ReadFailed)?;
    let to = iso_day(report.to, zone).ok_or(MiniAppApiError::ReadFailed)?;
    Ok(ReportDto {
        from,
        to,
        from_text: stamp(report.from, zone),
        to_text: stamp(report.to, zone),
        total: money_of(&report.total),
        by_exchange: report
            .by_exchange
            .into_iter()
            .map(|(key, name, total)| RowDto {
                key,
                name,
                section: None,
                money: money_of(&total),
            })
            .collect(),
        by_core: report
            .by_core
            .into_iter()
            .map(|(key, name, section, total)| RowDto {
                key,
                name,
                section: Some(section),
                money: money_of(&total),
            })
            .collect(),
        days: report
            .days
            .into_iter()
            .map(|(start, total)| {
                let money = money_of(&total);
                DayDto {
                    start,
                    usdt: money.usdt,
                    text: money.text,
                    trades: money.orders,
                }
            })
            .collect(),
    })
}

/// Window edge as the chat report stamps it, `DD.MM.YYYY HH:MM` in the report zone.
///
/// Args:
///     secs: UTC unix seconds of the window edge.
///     zone: Display zone.
///
/// Returns:
///     The stamp, or an empty string outside chrono's range.
fn stamp(secs: i64, zone: Tz) -> String {
    display_time::at(secs, zone)
        .map(|value| value.format("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_default()
}

/// Inclusive window edge as `YYYY-MM-DD` in the report zone.
///
/// Args:
///     secs: UTC unix seconds of the window edge.
///     zone: Display zone.
///
/// Returns:
///     The civil date, or `None` outside chrono's range.
fn iso_day(secs: i64, zone: Tz) -> Option<String> {
    display_time::date(secs, zone).map(|date| date.to_string())
}

/// Chat-report money: `0.00$` when there are no closed trades, `None` when unvalued.
///
/// `total.orders` is that closed-trade count, the figure the chat report labels
/// `telegram.report_trades`. It is not a count of open orders.
///
/// Args:
///     total: Quote breakdown whose `orders` field counts closed trades.
///
/// Returns:
///     Signed dollar text plus those counts, or no text when the total is unvalued.
fn money_of(total: &QuoteBreakdown) -> MoneyDto {
    let orders = u64::try_from(total.orders).unwrap_or(0);
    let unknown_orders = u64::try_from(total.unknown_orders).unwrap_or(0);
    if total.orders == 0 {
        return MoneyDto {
            usdt: Some(0.0),
            text: Some(signed_dollars(0.0)),
            orders: 0,
            unknown_orders: 0,
        };
    }
    match total.unified_usdt() {
        Some(amount) => MoneyDto {
            usdt: Some(amount.profit),
            text: Some(signed_dollars(amount.profit)),
            orders,
            unknown_orders,
        },
        None => MoneyDto {
            usdt: None,
            text: None,
            orders,
            unknown_orders,
        },
    }
}

/// Signed fixed dollars, the same digits the chat report appends `$` to.
fn signed_dollars(value: f64) -> String {
    let digits = fmt::signed_fixed(value, 2)
        .map(|(text, _)| text)
        .unwrap_or_else(|| "0.00".to_string());
    format!("{digits}$")
}

/// Mini App balance figure: grouped thousands, exactly two decimals, then `$`.
///
/// The hero total, each exchange, and each core share this text. The desktop
/// Assets panel keeps [`fmt::usd_grouped`], which drops a trailing zero.
///
/// Args:
///     value: USDT amount. Callers pass a finite value; a non-finite one prints `0.00$`.
///
/// Returns:
///     Display text such as `10 000.00$`.
pub(super) fn balance_text(value: f64) -> String {
    let mut text = fmt::usd_grouped_cents(value);
    text.push('$');
    text
}

/// Positive finite price as adaptive text. Zero and non-finite prices are absent.
fn price_text(value: f64) -> Option<String> {
    if value.is_finite() && value > 0.0 {
        Some(fmt::adaptive(value))
    } else {
        None
    }
}

/// Connection phase, dropping the stage and failure payloads.
pub(super) fn conn_of(status: &ConnStatus) -> ConnDto {
    match status {
        ConnStatus::Ready => ConnDto::Ready,
        ConnStatus::Connecting => ConnDto::Connecting,
        ConnStatus::Stage(_) => ConnDto::Stage,
        ConnStatus::Failed(_) => ConnDto::Failed,
        ConnStatus::Disconnected => ConnDto::Disconnected,
    }
}

/// Store trust state onto the Mini App enum.
pub(super) fn balance_state_dto(state: BalanceState) -> BalanceStateDto {
    match state {
        BalanceState::Live => BalanceStateDto::Live,
        BalanceState::Stale => BalanceStateDto::Stale,
        BalanceState::Awaiting => BalanceStateDto::Awaiting,
        BalanceState::Unpriced => BalanceStateDto::Unpriced,
    }
}
