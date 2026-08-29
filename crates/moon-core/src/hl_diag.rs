//! Diagnostic channel for the HyperLiquid request quota (`channels.hl_limit`), built to answer one
//! question that cannot be read out of the source: what does a core actually put into
//! `THLRequestLimitStateCommand` (UI CmdId=30)?
//!
//! The value is the only number the protocol carries about the address-level HyperLiquid action
//! quota, and nothing in the terminal consumes it — so without this channel it arrives, updates
//! `snapshot().settings().hyperliquid_requests_left`, and is never observable anywhere.
//!
//! A line reports the value the snapshot HOLDS, not only the moment a command lands: the core
//! publishes it rarely — in practice once, on connect — so a channel switched on an hour later
//! would otherwise stay empty while the answer sat in the snapshot the whole time. Turning the
//! switch on therefore writes one line per connected core, and after that only a change or a fresh
//! command writes another. `on_command=true` marks the lines that came with a command in the batch.
//!
//! Read `requests_left=none` carefully: moonproto decodes the field as `Option<u64>`, and its
//! "no value yet" sentinel is a payload of `0xFF` bytes, i.e. `-1`. `u64::try_from` rejects that —
//! and rejects EVERY other negative value the same way. So `none` means one of: the core has not
//! published a value, the core is not HyperLiquid, or the core sent a NEGATIVE number. A line with
//! `none` against a positive `Requests left` in the core's own Info window is therefore evidence
//! that the wire carries something other than the remaining quota.
//!
//! Off by default in every build. Enable it with `channels.hl_limit` in `cfg/diagnostics.toml` (or
//! `MOON_HL_LIMIT_DIAG=1`) → lines are appended to `logs/hl_limit_diag.log` beside the
//! application's own logs.

/// Whether the channel is on, as set by `channels.hl_limit` in `cfg/diagnostics.toml` or by the
/// matching environment variable. A live atomic, so the switch applies without a restart.
pub fn enabled() -> bool {
    crate::diagnostics::hl_limit()
}

/// Appends one line, stamped with the wall clock so it can be lined up against the core's own log
/// and its Info window. A no-op while the channel is off.
pub fn line(msg: &str) {
    if !enabled() {
        return;
    }
    let (date, hms) = crate::applog::split_unix_ms(crate::util::time::now_unix_ms_i64());
    crate::diagnostics::channel_line("hl_limit_diag.log", &format!("{date} {hms} {msg}"));
}
