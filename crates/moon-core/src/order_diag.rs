//! Environment-gated diagnostic for the ORDER pipeline, built to answer one question that cannot be
//! read out of the source: does an order the core says it published actually arrive here?
//!
//! The chain is `core → OrdersProto → moonproto client → snapshot().orders() → build_order_rows →
//! store.orders → panels`. Static reading proved there is no status filter anywhere in OUR half, so
//! whatever is lost is lost before `build_order_rows` — and the moonproto client drops exactly one
//! class of order silently: one whose MARKET it does not know yet is "parked" until the market
//! appears, with no log line of its own. This channel makes that visible from our side.
//!
//! Enable with `MOON_ORDER_DIAG=1` for everything, `MOON_ORDER_DIAG=BTC` for one market, or
//! `MOON_ORDER_DIAG=GateF/BTC` for one market on one core. Both halves match case-insensitively as
//! SUBSTRINGS, so `BTC` finds `BTCUSDT` and `GateF` finds the core named `GateF` — narrow enough
//! that a busy core with a hundred churning orders does not bury the one being watched. Lines are
//! appended to `order_diag.log` in the process working directory — the same relative-path rule
//! `diag.rs` and `detect_diag.rs` follow, so read the file the RUN wrote, not the one beside the exe.
//!
//! Off by default in every build (the workspace sets `debug-assertions = false`, so
//! `cfg(debug_assertions)` is not a gate here).

use std::sync::OnceLock;

/// The `MOON_ORDER_DIAG` value, read once: `None` when unset.
fn setting() -> Option<&'static String> {
    static VALUE: OnceLock<Option<String>> = OnceLock::new();
    VALUE
        .get_or_init(|| std::env::var("MOON_ORDER_DIAG").ok())
        .as_ref()
}

/// Whether the channel is on at all.
pub fn enabled() -> bool {
    setting().is_some()
}

/// Case-insensitive substring, with an empty needle matching everything.
fn matches(haystack: &str, needle: &str) -> bool {
    needle.is_empty()
        || haystack
            .to_ascii_uppercase()
            .contains(&needle.to_ascii_uppercase())
}

/// The selector itself, apart from where its value comes from, so it can be tested: the environment
/// is read once into a `OnceLock` and a test cannot set it twice.
fn follows_setting(setting: &str, core: &str, market: &str) -> bool {
    if setting == "1" {
        return true;
    }
    match setting.split_once('/') {
        Some((c, m)) => matches(core, c) && matches(market, m),
        None => matches(market, setting),
    }
}

/// Whether `market` on `core` is being followed. `1` or an empty value follows everything; a plain
/// value follows markets; `CORE/MARKET` narrows to one core as well.
pub fn follows(core: &str, market: &str) -> bool {
    setting().is_some_and(|v| follows_setting(v, core, market))
}

/// Appends one line, with a wall-clock stamp so it can be lined up against the core's own log.
pub fn line(msg: &str) {
    if !enabled() {
        return;
    }
    use std::io::Write;
    let stamp = crate::util::time::now_unix_ms_i64();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("order_diag.log")
    {
        let _ = writeln!(f, "{stamp} {msg}");
    }
}

#[cfg(test)]
mod tests;
