//! Diagnostic channel for the core's ClientSettings snapshot (`channels.settings`), built to answer
//! the questions about the two "do not buy" lists that reading the source cannot settle:
//!
//! - in WHAT SPELLING a core stores a temporary-blacklist row — the bare coin (`ADA`) or the full
//!   market (`ADAUSDT`). MoonBot's own Telegram command takes coins, the protocol's example writes
//!   markets, and a terminal that guesses wrong sends a ban the core silently never applies;
//! - whether the remaining time ticks down between snapshots, and how often a core republishes the
//!   settings at all — which decides whether a countdown can be drawn from this data.
//!
//! A line reports the state the snapshot HOLDS rather than only what a command carried, so
//! switching the channel on mid-session answers immediately instead of waiting for the core's next
//! publication. It is written only when the blacklist state actually CHANGES: TempBL remainders
//! tick down continuously, and a line per snapshot would fill the file with the same rows counting
//! themselves down. That test lives with the publisher in `feed::live`, so the log records exactly
//! the events the UI is told about.
//!
//! Off by default in every build. Enable it with `channels.settings` in `cfg/diagnostics.toml` (or
//! `MOON_SETTINGS_DIAG=1`) → lines are appended to `logs/settings_diag.log` beside the
//! application's own logs.

/// The blacklist state of one core, as one log line's worth of material.
///
/// TempBL rows keep their remaining time in DAYS exactly as the wire carries it, unrounded: what
/// this channel is read for is the raw value, not a tidied one. WHEN a line is written is decided
/// by the feed, which uses the same test it uses to publish the rows to the UI — so the log and
/// the panel cannot disagree about what happened.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlacklistShot {
    /// `use_coins_black_list`.
    pub global_on: bool,
    /// `coins_black_list_text`, verbatim.
    pub global_text: String,
    /// `TempBLSymbols` paired with `TempBLTimes`, in wire order.
    pub temp: Vec<(String, f64)>,
}

impl BlacklistShot {
    /// One log line: the global list, then every TempBL row with its remaining time both as the
    /// wire carries it and in hours, which is the unit MoonBot itself speaks.
    pub fn describe(&self) -> String {
        use std::fmt::Write as _;

        let mut out = format!(
            "global_on={} global_len={} global=\"{}\" temp_rows={}",
            self.global_on,
            self.global_text.len(),
            self.global_text,
            self.temp.len(),
        );
        for (symbol, days) in &self.temp {
            // Writing in place rather than through a `format!` per row: a core can hold a long list
            // and this runs per reported change.
            let _ = write!(
                out,
                " | temp \"{symbol}\" days={days} hours={:.4}",
                days * 24.0
            );
        }
        out
    }
}

/// Whether the channel is on, as set by `channels.settings` in `cfg/diagnostics.toml` or by the
/// matching environment variable. A live atomic, so the switch applies without a restart.
pub fn enabled() -> bool {
    crate::diagnostics::settings()
}

/// Appends one line, stamped with the wall clock so it can be lined up against the core's own log
/// and MoonBot's own settings window. A no-op while the channel is off.
pub fn line(msg: &str) {
    if !enabled() {
        return;
    }
    crate::diagnostics::stamped_line("settings_diag.log", msg);
}

#[cfg(test)]
mod tests;
