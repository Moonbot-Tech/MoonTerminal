//! The public boards: what the last twenty-four hours look like from the outside.
//!
//! The rolling minute in [`crate::crowd::minute`] is ours — we add it up ourselves from the trades
//! as they arrive. These two are the SERVICE's: it computes them and hands them over whole, a coin
//! top and a trader top, and there is no stream for either. They arrive as snapshots and replace
//! what was there.
//!
//! The service calls both of them twenty-four hours and its endpoints are named for it; whether it
//! means a rolling day or the time since midnight is its own business, and nothing here recomputes
//! either figure.
//!
//! Kept here, beside the minute, because they are the same kind of thing — public crowd figures —
//! and because the statistics screen is built on this crate alone.

/// What the whole crowd did in the day, as the service adds it up.
///
/// Its own figure rather than a sum of the boards: the boards are TOPS — fifty traders and ten
/// coins — while this counts everybody, so adding the rows up would report a fraction of the day
/// as the whole of it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DaySummary {
    /// Closed trades over the day, the wire's `c`.
    pub trades: u64,
    /// Profit in dollars over the day, the wire's `s`.
    pub profit: f64,
}

/// One coin's twenty-four hours.
///
/// The board carries the best few and the worst few, which is why the profit is signed. The
/// service's own rank inside each half is not kept: the two halves are read as one column ordered
/// by money, so a second ordering would only be a number nothing could check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoinDay {
    /// Ticker, base asset only, exactly as the service spells it.
    pub coin: String,
    /// Profit in dollars over the rolling day, signed.
    pub profit: f64,
}

/// One trader's twenty-four hours.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trader {
    /// Place on the board, from one.
    pub place: u32,
    /// The service's own account number for this trader — the wire's `i`.
    ///
    /// Shown next to the place because it is the only name an anonymous row has, and because it
    /// is the one that stays the same: a handle can be changed and a place changes every hour.
    /// It is not a secret — the board is a public page, and this number is what it publishes.
    pub id: u64,
    /// Telegram handle, or a bare `@` for somebody who has not consented to being named. Their
    /// figures are still counted — the service says so — but the person is not.
    pub handle: String,
    /// Profit in dollars over the day.
    ///
    /// The wire calls this `pb`, and that it is dollars was checked rather than assumed: the fifty
    /// rows sum to about 58.9k against the service's own 24-hour total of 55.7k, which is exactly
    /// the shape to expect when everybody below the top fifty is losing.
    pub profit: f64,
    /// Closed trades behind it. The wire's `c`; the fifty rows sum to 65k against a total of 74k.
    pub trades: u64,
}

impl Trader {
    /// Whether this row belongs to somebody who chose not to be named.
    ///
    /// Worth asking rather than comparing strings at the call site: the service uses a bare `@`
    /// for it, which is easy to print by accident and reads as a broken row.
    pub fn anonymous(&self) -> bool {
        self.handle.trim() == "@" || self.handle.trim().is_empty()
    }

    /// What to show for the name.
    pub fn name(&self) -> &str {
        if self.anonymous() {
            "—"
        } else {
            // Trimmed the same way [`Self::anonymous`] trims: the two disagreeing is how a handle
            // with a space in front of it printed its at-sign.
            self.handle.trim().trim_start_matches('@')
        }
    }
}

#[cfg(test)]
mod tests;
