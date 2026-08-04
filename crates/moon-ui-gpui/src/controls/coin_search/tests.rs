//! Regression coverage for the pure ranking/grouping steps behind the coin-search dropdown.
//!
//! `suggest_volatile` and `hits_for` need a live `Backend`, which is not constructible in a unit
//! test, so these tests exercise the pure helpers they delegate to instead.

use super::{
    CoinHit, MOVER_VOL_REF, Mover, group_hits, merge_ranked_heads, mover_score,
    neutralize_blind_provider, turnover_usd,
};
use moon_core::market::MarketLabel;

/// Build one ranked candidate the way `suggest_volatile` does, so a test states only what it is
/// about.
fn mover(movement: f64, turnover: f64, market: &str, slot: usize) -> Mover {
    Mover {
        score: mover_score(movement, turnover),
        movement,
        turnover: Some(turnover),
        market: market.to_string(),
        slot,
    }
}

/// Build a candidate whose quote could not be converted, so its turnover is UNKNOWN rather than
/// a known zero — the case the blind-provider rule exists for.
fn unpriced_mover(movement: f64, market: &str, slot: usize) -> Mover {
    Mover {
        score: mover_score(movement, MOVER_VOL_REF),
        movement,
        turnover: None,
        market: market.to_string(),
        slot,
    }
}

/// `coin_search.rs:merge_ranked_heads` (the merge step inside `suggest_volatile`) must rank the
/// merged set across EVERY provider's head, not just keep the first provider's own order padded
/// out with the next provider's.
///
/// Breakage this pins: dropping the `heads.sort_by(rank_movers)` re-rank before the final
/// `truncate`, or swapping the two so truncation runs first. Either way, on a multi-exchange
/// scope the user would be shown the first provider's top movers padded with the next provider's,
/// instead of the top movers of the whole scope.
#[test]
fn merge_ranked_heads_ranks_across_every_provider_not_just_the_first() {
    // Two providers' own heads, each already ranked and truncated to its own visible size (as
    // `suggest_volatile` produces them) — provider 1's movers are strictly bigger than provider
    // 0's, so a correct merge must promote them ahead of provider 0's entirely.
    // Turnover is equal across all four, so the weighting cannot decide anything here and the
    // merge is judged purely on whether it re-ranks across providers.
    let heads = vec![
        mover(5.0, 10.0 * MOVER_VOL_REF, "AAA", 0),
        mover(4.0, 10.0 * MOVER_VOL_REF, "BBB", 0),
        mover(10.0, 10.0 * MOVER_VOL_REF, "CCC", 1),
        mover(9.0, 10.0 * MOVER_VOL_REF, "DDD", 1),
    ];

    let merged = merge_ranked_heads(heads, 2);

    let markets: Vec<&str> = merged.iter().map(|m| m.market.as_str()).collect();
    assert_eq!(
        markets,
        vec!["CCC", "DDD"],
        "the merged top movers must come from across every provider, not just the first \
         provider's own head padded out with the next: got {markets:?}"
    );
}

/// `coin_search.rs:mover_score` must let a liquid market outrank a thin one that merely printed a
/// bigger percentage — the whole point of weighing the range by turnover.
///
/// Breakage this pins: returning the score to the bare 24-hour range, or making the turnover term
/// additive rather than multiplicative. Either way a market with a handful of trades and a huge
/// percentage heads the suggestions again, which is the complaint this weighting answers.
#[test]
fn a_liquid_mover_outranks_a_dust_spike() {
    // A near-dead market printing 40%, against a heavily traded one printing 15%.
    let dust = mover_score(40.0, 2_000.0);
    let liquid = mover_score(15.0, 50_000_000.0);

    assert!(
        liquid > dust,
        "a market turning over $50M at 15% must outrank one turning over $2k at 40%: \
         liquid={liquid}, dust={dust}"
    );
}

/// `coin_search.rs:mover_score` must stay finite and non-negative when turnover is missing,
/// zero, or garbage.
///
/// Breakage this pins: dropping the sanitizing guard and feeding the raw figure to the logarithm.
/// `log10(1 + 0)` is fine, but a NEGATIVE turnover makes the argument less than one and the score
/// negative, and the screener's own default fills turnover with a plain `0.0` when a market has
/// never reported it — so unranked markets would sort BELOW every real one by a value that is not
/// even comparable, instead of simply scoring zero.
#[test]
fn a_score_without_turnover_is_zero_and_finite() {
    assert_eq!(
        mover_score(10.0, 0.0),
        0.0,
        "no turnover scores exactly zero"
    );
    for bad in [f64::NAN, -1.0, f64::NEG_INFINITY] {
        let score = mover_score(10.0, bad);
        assert!(
            score.is_finite() && score == 0.0,
            "turnover {bad} must be treated as absent, not propagated: got {score}"
        );
    }
}

/// `coin_search.rs:mover_score` must price each tenfold of turnover at exactly one point, so the
/// weight's strength is a stated property rather than an accident of the chosen logarithm.
///
/// Breakage this pins: swapping `log10` for `ln`, or dropping the `MOVER_VOL_REF` normalization.
/// Both still "weigh by turnover" and both still pass a smoke test, but they silently change how
/// much range it takes to beat a more liquid market — with `ln` a decade is worth 2.3 points, so
/// turnover starts overpowering the movement the section is named after.
#[test]
fn an_extra_decade_of_turnover_is_worth_one_point() {
    // Far above the reference figure the `+1` inside the logarithm is negligible, so the step
    // between decades converges on exactly one. The oracle is the arithmetic of a base-10
    // logarithm, not a number read back out of the implementation.
    let base = 1_000.0 * MOVER_VOL_REF;
    let step = mover_score(1.0, 10.0 * base) - mover_score(1.0, base);

    assert!(
        (step - 1.0).abs() < 0.01,
        "a tenfold of turnover must add one point at unit movement: got {step}"
    );
}

/// `coin_search.rs:neutralize_blind_provider` must rescue a provider when none of its candidates'
/// quote currencies can be converted to USD.
///
/// Breakage this pins: deleting the rule, or gating it on one row rather than the whole provider.
/// The provider would then lose the merged suggestions because missing conversion rates make its
/// liquidity incomparable, not because its markets are known to be thin.
#[test]
fn a_provider_reporting_no_turnover_still_competes() {
    // No candidate has a convertible quote, so every turnover is UNKNOWN rather than zero.
    let mut blind = vec![unpriced_mover(8.0, "AAA", 0), unpriced_mover(3.0, "BBB", 0)];
    neutralize_blind_provider(&mut blind);

    assert!(
        blind.iter().all(|m| m.score > 0.0),
        "every rescued row must carry a positive score"
    );
    assert!(
        blind[0].score > blind[1].score,
        "rescued rows keep their order by movement alone: {:?} vs {:?}",
        blind[0].score,
        blind[1].score
    );
    // Any convertible turnover makes the provider comparable, and a converted zero is genuine
    // information about a dead market — including when EVERY value is zero.
    let mut sighted = vec![mover(8.0, 0.0, "AAA", 0), mover(3.0, 5_000_000.0, "BBB", 0)];
    neutralize_blind_provider(&mut sighted);
    assert_eq!(
        sighted[0].score, 0.0,
        "one reporting market makes the provider's zeros meaningful, so a dead market must stay          at zero"
    );
    assert!(
        sighted[1].score > 0.0,
        "and the market that does report turnover keeps its weighted score"
    );
    let mut all_dead = vec![mover(8.0, 0.0, "AAA", 0), mover(3.0, 0.0, "BBB", 0)];
    neutralize_blind_provider(&mut all_dead);
    assert!(
        all_dead.iter().all(|m| m.score == 0.0),
        "a provider REPORTING zero everywhere is describing dead markets, not staying silent:          rescuing those puts the dead curves straight back on top"
    );
}

/// `coin_search.rs:group_hits` must key a run on the FULL instrument label
/// ([`MarketLabel::pair`]), never a contract-stripped coin.
///
/// Breakage this pins: changing the grouping key to `MarketLabel::match_key` or
/// `MarketLabel::display_coin`, reasoning "same coin, group it together". A perpetual and a
/// dated contract of the same coin would then merge into one run, and the continuation row would
/// make the two instruments indistinguishable in the dropdown.
#[test]
fn a_dated_contract_never_groups_with_its_perpetual() {
    let perpetual = CoinHit {
        core: 1,
        market: "BTCUSDT".to_string(),
        server: "Core A".to_string(),
        label: MarketLabel {
            coin: "BTC".to_string(),
            quote: "USDT".to_string(),
            contract: None,
        },
    };
    let dated = CoinHit {
        core: 2,
        market: "BTCUSD0925".to_string(),
        server: "Core B".to_string(),
        label: MarketLabel {
            coin: "BTC_0925".to_string(),
            quote: "USDT".to_string(),
            contract: None,
        },
    };
    // `match_key`/`display_coin` would fold both hits to "BTC", which is exactly the collapse
    // this test must catch if the grouping key is ever weakened to either of them.
    assert_eq!(perpetual.label.match_key(), dated.label.match_key());

    let rows = group_hits(vec![perpetual, dated]);

    assert_eq!(
        rows.len(),
        2,
        "both instruments must survive as distinct rows"
    );
    let runs: Vec<(String, bool)> = rows
        .into_iter()
        .map(|row| (row.pair.to_string(), row.first_of_group))
        .collect();
    assert_eq!(
        runs,
        vec![
            ("BTC-USDT".to_string(), true),
            ("BTC-USDT-0925".to_string(), true),
        ],
        "a perpetual and a dated contract of the same coin must form two SEPARATE runs, each \
         opening its own group, not one run where the second row reads as a continuation: {runs:?}"
    );
}

/// `coin_search.rs:turnover_usd` must keep "this market traded nothing" apart from "this market's
/// turnover cannot be converted".
///
/// Breakage this pins: filtering non-positive USD values out of the `Option`, which turns a KNOWN
/// zero into `None`. `None` means unknown, and unknown is
/// scored at the reference turnover so a market is not buried for a missing rate; a market that
/// genuinely traded nothing would inherit that rescue and climb back to the top of the movers,
/// which is the exact complaint the turnover weighting answers.
#[test]
fn a_market_that_traded_nothing_is_not_a_market_of_unknown_turnover() {
    assert_eq!(
        turnover_usd(0.0, Some(1.0)),
        Some(0.0),
        "a known rate and no turnover is a DEAD market, not an unknown one"
    );
    assert_eq!(
        turnover_usd(f64::NAN, Some(1.0)),
        Some(0.0),
        "and so is a figure that is not a number, once the rate is known"
    );
    assert_eq!(
        turnover_usd(5.0, None),
        None,
        "an unconvertible quote leaves the turnover unknown, whatever the raw figure"
    );
    // The conversion itself: turnover is denominated in the market's OWN quote.
    assert_eq!(turnover_usd(3.0, Some(70_000.0)), Some(210_000.0));

    // And the two answers must reach different scores.
    let dead = mover_score(40.0, turnover_usd(0.0, Some(1.0)).unwrap_or(MOVER_VOL_REF));
    let unknown = mover_score(40.0, turnover_usd(5.0, None).unwrap_or(MOVER_VOL_REF));
    assert_eq!(dead, 0.0, "a dead market scores zero");
    assert!(unknown > 0.0, "an unknown one still competes: {unknown}");
}
