//! The "Entry/Exit" tuning axis — a what-if evaluated by REPLAYING the trade tape around each
//! closed trade rather than by an SQL mask over report fields.
//!
//! What lives here so far is the part the trade tape itself is kept by: which report rows the
//! axis reads ([`scope`]) and the stretch of prints each of them needs ([`model_window_at`]).
//! The Storage tab's cleanup keeps exactly that, so what the axis will fetch is what it keeps.

use crate::market::trade_replay::{ReplayWindow, replay_window_ms};

pub mod scope;

pub use scope::{is_service_row, is_tunable};

/// When an entry order's life began, for a model that replays it whole: its creation stamp,
/// unless the order waited longer than [`ORDER_WAIT_CAP_MS`] — then its early life is not
/// fetched and the model starts where the tape does. One rule for every kind of the tuner, the
/// ones without an entry model too: their tape is fetched and kept for a model to come.
///
/// Args:
///     buy_ms: The fill of the entry.
///     buy_set_ms: The order's creation, on the same clock (`buysetdatems`).
pub fn order_open_at(buy_ms: i64, buy_set_ms: Option<i64>) -> Option<i64> {
    buy_set_ms.filter(|&set| set <= buy_ms && buy_ms - set <= ORDER_WAIT_CAP_MS)
}

/// The longest wait of an entry order the tape is fetched for, from its creation to its fill.
/// MoonShot orders on this machine's reports (2026-09-23, 289 with a creation stamp) waited a
/// median 114 s, 280 s at the 90th percentile and hours at the 99th; the cap keeps the few that
/// wait for hours from asking the venue for hours of prints, and they replay as before.
pub const ORDER_WAIT_CAP_MS: i64 = 10 * 60_000;

/// The replay window of a trade as the model needs it: from the entry order's creation
/// ([`order_open_at`]) through the close, one stretch. Where that stretch would be walked as its
/// two ends — the order's life plus the position outrun `long_position_ms` — the window opens at
/// the fill as before: an entry end around the creation would leave the fill itself between
/// the ends, where nothing is fetched. The tape cleanup claims by this same rule
/// (`trades_cleanup`), so what the tuner fetched is what it keeps.
///
/// Args:
///     order_open_ms: The order's creation where a replay may start there ([`order_open_at`]).
///     buy_ms: The fill of the entry.
///     close_ms: The close.
///     margin_ms: The margin setting (`trade_replay::margin_ms`).
///     long_position_ms: The threshold the window is split by — the caller's, so every stage
///         of one row splits it the same way.
///
/// Returns:
///     The window, or `None` when the stamps describe none.
pub fn model_window_at(
    order_open_ms: Option<i64>,
    buy_ms: i64,
    close_ms: i64,
    margin_ms: i64,
    long_position_ms: i64,
) -> Option<ReplayWindow> {
    let with_threshold = |window: ReplayWindow| ReplayWindow {
        long_position_ms,
        ..window
    };
    let from_creation = order_open_ms
        .and_then(|open| replay_window_ms(open, close_ms, margin_ms))
        .map(with_threshold)
        .filter(|w| w.close_ms - w.open_ms <= w.long_position_ms);
    from_creation.or_else(|| replay_window_ms(buy_ms, close_ms, margin_ms).map(with_threshold))
}
