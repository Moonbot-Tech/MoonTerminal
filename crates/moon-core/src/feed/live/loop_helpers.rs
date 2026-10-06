//! Small projections and publication predicates used by the live loop.

use super::*;

/// The Telegram event of a trade's entry whose strategy has `ReportTradesToTelegram`, else
/// `None` — a manual trade (no strategy), a row that never carried its strategy, or no strategy
/// snapshot yet.
///
/// Args:
///     snap: The client's state, for the strategy's flag and name.
///     strategy: The entry's `StrategyID`, from whichever upsert of the row carried it — a limit
///         buy files it with the order, and the fill's own upsert does not repeat it.
///     emulator: Its `Emulator` flag, carried the same way.
///     rec_id: The trade's report row.
///     coin: Its coin token.
///     buy: Its entry stamp, core-local.
pub(super) fn tg_opened(
    snap: Option<&MoonStateSnapshot>,
    strategy: Option<i64>,
    emulator: Option<bool>,
    rec_id: i64,
    coin: &str,
    buy: crate::db::ReportStamp,
) -> Option<CoreTgEvent> {
    let strat_id = match strategy {
        Some(id) if id > 0 => id as u64,
        Some(_) => return None,
        None => {
            // Not announced, and this is the only trace of it.
            log::debug!("telegram: entry of {coin} (row {rec_id}) carries no StrategyID");
            return None;
        }
    };
    let snap = snap?;
    if !strat_field_bool(snap, strat_id, "ReportTradesToTelegram") {
        return None;
    }
    let emulator = emulator.unwrap_or(false);
    Some(CoreTgEvent::Opened {
        rec_id,
        coin: coin.to_string(),
        strat_name: detect_strat_name(snap.strats().snapshot(strat_id)),
        emulator,
        buy,
    })
}

/// Field indices of `ReportUID` and `CloseDate` in one schema revision, or `None` when the core
/// does not report the identity: such a core's rows cannot be asked about.
///
/// Args:
///     schema: The revision the core just sent.
pub(super) fn trace_field_indices(schema: &moonproto::ReportSchema) -> Option<(u16, u16)> {
    let integer = |name: &str| {
        schema
            .field_by_name(name)
            .filter(|field| field.kind == moonproto::ReportFieldKind::Integer)
            .map(|field| field.index)
    };
    Some((integer("ReportUID")?, integer("CloseDate")?))
}

/// The `ReportUID` of an upserted row that is CLOSED, or `None`.
///
/// A live upsert is partial: a row that does not carry `CloseDate` is not known to be closed and
/// is left alone — the upsert that closes a trade carries the field it changed. A zero uid is the
/// replica's placeholder and never a key.
///
/// Args:
///     row: The upserted row.
///     fields: `(ReportUID, CloseDate)` indices from [`trace_field_indices`].
pub(super) fn closed_row_uid(
    row: &moonproto::ReportRow,
    (uid_ix, close_ix): (u16, u16),
) -> Option<i64> {
    let closed = matches!(row.value(close_ix), Some(moonproto::ReportValue::Integer(v)) if *v != 0);
    if !closed {
        return None;
    }
    match row.value(uid_ix) {
        Some(moonproto::ReportValue::Integer(uid)) if *uid != 0 => Some(*uid),
        _ => None,
    }
}

/// Returns whether this event batch must publish the retained assets snapshot.
///
/// Balance events bypass the ordinary market-driven rate limit so the header reflects confirmed
/// free funds immediately. Other domain events publish only after `assets_every` has elapsed.
pub(super) fn should_publish_assets(
    events: &[Event],
    assets_elapsed: Duration,
    assets_every: Duration,
) -> bool {
    !events.is_empty()
        && (assets_elapsed >= assets_every
            || events
                .iter()
                .any(|event| matches!(event, Event::Balance(_))))
}

/// Returns bytes as a separator-free hex string for dumping chart-alert blobs while reverse
/// engineering the `TChartObject.Save()` format; see alert stage 0 in the internal docs.
pub(super) fn hex_dump(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
