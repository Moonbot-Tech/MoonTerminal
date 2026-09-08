//! The core's marked-markets list: one comma-separated `trading.fav_markets` string.
//!
//! Its own module because it is its own SHAPE — every other field of the safe-share projection is a
//! number, a flag or a struct, while this one is a list encoded in a string, and the rules for
//! reading and changing it (separators, case, what an empty name means) belong together rather than
//! scattered among the readers.
//!
//! Written as a DELTA, never as a whole list: the core writes this field too, so
//! [`fav_markets_set`] is applied to the core's own snapshot at send time — see
//! `feed::live::shared_config::SharedConfigSequence::enqueue_fav_market`.

/// Markets named by one `trading.fav_markets` string, in the order the core lists them.
///
/// Through [`crate::symbol::split_coin_list`], which owns the separator set for every list field a
/// core writes: this one is comma-separated in practice, but deciding that here would be a second
/// answer to a question the terminal already answers once — and a list a trader typed with spaces
/// or semicolons would read back as one long market that matches nothing.
///
/// Args:
///     text: The raw field.
///
/// Returns:
///     The markets, without empties.
pub fn fav_markets_list(text: &str) -> Vec<&str> {
    crate::symbol::split_coin_list(text).collect()
}

/// Whether that list names this market.
///
/// Case-insensitive, the rule every other symbol comparison here uses: a core that echoes a
/// different case must not read as a different coin.
///
/// Args:
///     text: The raw field.
///     market: Market to look for, as the core spells it.
///
/// Returns:
///     Whether it is listed.
pub fn fav_markets_has(text: &str, market: &str) -> bool {
    // Streamed rather than collected: this is asked once per chart pane per frame, and a list
    // built only to be searched is an allocation on the render path.
    crate::symbol::split_coin_list(text).any(|held| held.eq_ignore_ascii_case(market))
}

/// Put this market in the list, or take it out — an ABSOLUTE state, never a toggle.
///
/// Absolute because of WHERE this runs: the write is applied to the core's snapshot at SEND time,
/// which may be several round trips after the trader pressed the star. A toggle resolved then would
/// answer against a list nobody was looking at, and two presses queued together would cancel each
/// other. The press decides the state; this only carries it.
///
/// Every other market survives spelled as the core spelled it — a write that respells one while
/// changing another is an edit nobody asked for. The separators are rebuilt, so a core that wrote
/// `"A , B"` reads back `"A,B"`; the readers here compare NAMES, and every echo of this field is
/// checked by membership rather than by string equality for that reason.
///
/// A new market is appended, because this is not an MRU — the order is the one the trader built.
///
/// Args:
///     text: The raw field as the core last reported it.
///     market: Market to set, as the core spells it. Blank is refused: the field is a list of
///         names, and an empty name would grow a separator per press and never be found again.
///     on: Whether it must be listed afterwards.
///
/// Returns:
///     The new field value.
pub fn fav_markets_set(text: &str, market: &str, on: bool) -> String {
    let market = market.trim();
    if market.is_empty() {
        return text.to_string();
    }
    let held = fav_markets_list(text);
    let listed = held.iter().any(|item| item.eq_ignore_ascii_case(market));
    if listed == on {
        return held.join(",");
    }
    let kept: Vec<&str> = match on {
        true => held.into_iter().chain(std::iter::once(market)).collect(),
        false => held
            .into_iter()
            .filter(|item| !item.eq_ignore_ascii_case(market))
            .collect(),
    };
    kept.join(",")
}

#[cfg(test)]
mod tests;
