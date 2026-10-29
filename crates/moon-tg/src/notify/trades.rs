//! Bot-facing closed-trade decisions, implemented by the core's pure notification backend.

pub(crate) use moon_core::telegram::notify::trades::{
    Announced, ClosedTrade, HOLD_SECS, decide, decide_charts, floor_from, held_until, hold_until,
    read_from_utc,
};
