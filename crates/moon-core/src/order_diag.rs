//! Diagnostic channel for the ORDER pipeline (`channels.orders` in `cfg/diagnostics.toml`), built to answer one question that cannot be
//! read out of the source: does an order the core says it published actually arrive here?
//!
//! The chain is `core → OrdersProto → moonproto client → snapshot().orders() → build_order_rows →
//! store.orders → panels`. Static reading proved there is no status filter anywhere in OUR half, so
//! whatever is lost is lost before `build_order_rows` — and the moonproto client drops exactly one
//! class of order silently: one whose MARKET it does not know yet is "parked" until the market
//! appears, with no log line of its own. This channel makes that visible from our side.
//!
//! Set `channels.orders` in `cfg/diagnostics.toml` (or `MOON_ORDER_DIAG`) to `1` for everything,
//! `BTC` for one market, or `GateF/BTC` for one market on one core. Both halves match
//! case-insensitively as SUBSTRINGS, so `BTC` finds `BTCUSDT` and `GateF` finds the core named
//! `GateF` — narrow enough that a busy core with a hundred churning orders does not bury the one
//! being watched. Lines are appended to `logs/order_diag.log` beside the application's own logs.
//!
//! Off by default in every build (the workspace sets `debug-assertions = false`, so
//! `cfg(debug_assertions)` is not a gate here).

/// Whether the channel is on at all.
pub fn enabled() -> bool {
    crate::diagnostics::orders()
}

/// Case-insensitive substring, with an empty needle matching everything.
fn matches(haystack: &str, needle: &str) -> bool {
    needle.is_empty()
        || haystack
            .to_ascii_uppercase()
            .contains(&needle.to_ascii_uppercase())
}

/// The selector itself, apart from where its value comes from, so it can be tested directly on a
/// string rather than through the process-wide switch every other test would race.
fn follows_setting(setting: &str, core: &str, market: &str) -> bool {
    if setting == "1" {
        return true;
    }
    match setting.split_once('/') {
        Some((c, m)) => matches(core, c) && matches(market, m),
        None => matches(market, setting),
    }
}

/// Whether `market` on `core` is being followed. `1` follows everything; a plain value follows
/// markets; `CORE/MARKET` narrows to one core as well.
///
/// The selector is borrowed rather than copied out: this runs per order per core, and the channel
/// is only worth leaving on because the off case costs a single atomic load.
pub fn follows(core: &str, market: &str) -> bool {
    crate::diagnostics::with_orders_selector(|s| follows_setting(s, core, market)).unwrap_or(false)
}

/// Appends one line, with a wall-clock stamp so it can be lined up against the core's own log.
pub fn line(msg: &str) {
    if !enabled() {
        return;
    }
    let stamp = crate::util::time::now_unix_ms_i64();
    crate::diagnostics::channel_line("order_diag.log", &format!("{stamp} {msg}"));
}

#[cfg(test)]
mod tests;
