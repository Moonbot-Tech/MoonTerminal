//! Pure ranking policy for the coin search's top-movers suggestions.

/// Daily turnover, in USD, that one step of the liquidity weight is measured against.
///
/// The weight is a base-10 logarithm, so every tenfold step above this figure adds exactly one
/// point: 100k scores 0.30, 1M scores 1.04, 10M scores 2.00, 1B scores 4.00. A market trading a
/// hundredth of another therefore needs roughly three times the range to outrank it. 100k is the
/// order of magnitude the Screener's own turnover filter is written against — its input carries
/// `500k` as its placeholder — so the two surfaces call the same markets thin.
pub(super) const MOVER_VOL_REF: f64 = 100_000.0;

/// Ceiling on the rows ONE suggestion section may put in the popup.
///
/// The section's own limit counts markets, but each market is offered once per core that can open
/// it — the same instrument on forty cores is forty rows, all of them real elements in a
/// non-virtualized list. This bounds the section by what it costs to draw rather than by what it
/// ranked, and it is generous enough that the count only bites on a config large enough for the
/// list to be unreadable anyway.
pub(super) const SUGGEST_ROW_CAP: usize = 48;

/// One market competing for a place in the top-movers suggestion.
///
/// `turnover` is already converted to USD, and stays an `Option` all the way down: `None` means the
/// quote could not be converted, which is a different fact from a market that traded nothing and
/// must not be flattened into the same zero — see [`turnover_usd`]. `score` is [`mover_score`] over
/// the pair, kept beside the inputs so ranking never recomputes it and the blind-provider rule can
/// overwrite it.
pub(super) struct Mover {
    /// Turnover-weighted movement used as the primary ranking key.
    pub(super) score: f64,
    /// Unsigned 24-hour movement used to break equal weighted scores.
    pub(super) movement: f64,
    /// USD-converted turnover, or `None` when the quote rate is unavailable.
    pub(super) turnover: Option<f64>,
    /// Canonical market name passed to the selected core.
    pub(super) market: String,
    /// Provider-consumer group that can open this market.
    pub(super) slot: usize,
}

/// Weighs a market's 24-hour range by how much money actually moved through it.
///
/// A range on its own answers the wrong question: an illiquid market with a handful of trades
/// prints the largest percentages on the board and is exactly what a trader does not want offered.
/// Multiplying by `log10(1 + turnover/REF)` keeps range in the lead among comparable markets while
/// sending a dead one to the bottom, and it degrades smoothly rather than at a cliff — no market
/// is excluded, so a feed that reports no turnover cannot empty the section.
///
/// Args:
///     movement: Unsigned 24-hour range magnitude, in percent.
///     turnover_usd: 24-hour turnover converted to USD; anything not positive and finite counts
///         as zero, which scores zero rather than a negative or infinite weight.
///
/// Returns:
///     The weighted score; zero when either input is zero.
pub(super) fn mover_score(movement: f64, turnover_usd: f64) -> f64 {
    let turnover = if turnover_usd.is_finite() && turnover_usd > 0.0 {
        turnover_usd
    } else {
        0.0
    };
    movement * (1.0 + turnover / MOVER_VOL_REF).log10()
}

/// Converts a market's own-quote turnover into USD, keeping a known zero and an unavailable rate
/// apart.
///
/// The two must not collapse into one answer. A market whose rate IS known and whose 24-hour
/// turnover is zero is a dead market: it scores zero and sinks, which is the whole point of
/// weighting by turnover. A market whose quote cannot be converted says nothing about its
/// liquidity, and scoring it as dead would bury a real market for a missing rate — those are
/// scored at the reference figure instead, so they compete on movement alone.
///
/// Args:
///     vol_24h: Turnover over the last 24 hours, in the market's OWN quote currency.
///     rate: USD value of one unit of that quote, or `None` when it cannot be resolved.
///
/// Returns:
///     `Some(usd)` when the rate is known — zero for a market that traded nothing, or whose
///     figures are not finite — and `None` when the quote cannot be converted.
pub(super) fn turnover_usd(vol_24h: f64, rate: Option<f64>) -> Option<f64> {
    rate.map(|rate| {
        let usd = vol_24h * rate;
        if usd.is_finite() && usd > 0.0 {
            usd
        } else {
            0.0
        }
    })
}

/// Keeps a provider whose candidates cannot be converted to USD from losing the merge outright.
///
/// When every candidate has an unavailable quote rate, none has comparable USD turnover. Rescoring
/// all of them at the reference turnover leaves their internal order purely by movement and lets
/// the provider compete without pretending its markets are dead. Any known turnover, including a
/// genuine zero, keeps the provider on the ordinary weighted ranking.
///
/// Args:
///     rows: One provider's candidates, already carrying movement and USD turnover.
///
/// Returns:
///     Nothing; a provider with any USD-valued turnover is left alone.
pub(super) fn neutralize_blind_provider(rows: &mut [Mover]) {
    // Blind means no candidate's quote can be converted — not that every converted value is zero.
    // Genuine zeros describe dead markets, and rescuing those would put them back at the top.
    if rows.iter().any(|m| m.turnover.is_some()) {
        return;
    }
    for row in rows.iter_mut() {
        row.score = mover_score(row.movement, MOVER_VOL_REF);
    }
}

/// Orders two candidate markets by weighted score, then movement, then turnover, then name.
///
/// A total order over every field read, so equal movers keep a stable position instead of
/// reshuffling between calls.
fn rank_movers(a: &Mover, b: &Mover) -> std::cmp::Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            b.movement
                .partial_cmp(&a.movement)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| {
            // Unknown turnover ranks with the dead here: this is only a tie-break, and the
            // rescue it deserves has already been applied to the score above.
            b.turnover
                .unwrap_or(0.0)
                .partial_cmp(&a.turnover.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| a.market.cmp(&b.market))
}

/// Merges every provider's already-ranked head into the top movers of the WHOLE scope.
///
/// Pulled out of `suggest_volatile` so it can be exercised without a `Backend`: each element of
/// `heads` is the visible head one provider contributed. Re-ranking the merged set — rather than
/// trusting each head's own order and simply truncating — is what keeps the result the top movers
/// across every provider instead of the first provider's top movers padded out with the next
/// provider's.
///
/// Args:
///     heads: Per-provider ranked heads, already truncated to their own visible size.
///     limit: Maximum number of rows to keep after merging.
///
/// Returns:
///     The heads re-ranked across the whole scope and truncated to `limit`.
pub(super) fn merge_ranked_heads(mut heads: Vec<Mover>, limit: usize) -> Vec<Mover> {
    heads.sort_by(rank_movers);
    heads.truncate(limit);
    heads
}
