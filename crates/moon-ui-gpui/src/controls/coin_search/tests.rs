//! Regression coverage for the pure ranking/grouping steps behind the coin-search dropdown.
//!
//! `suggest_volatile` and `hits_for` need a live `Backend`, which is not constructible in a unit
//! test, so these tests exercise the pure helpers they delegate to instead.

use super::{
    CoinHit, CoinResults, MOVER_VOL_REF, Mover, direct_row_count, enter_target, group_hits,
    group_is_open, group_starts_expanded, merge_ranked_heads, mover_score,
    neutralize_blind_provider, pick_core, turnover_usd, whole_row_cap,
};
use moon_core::market::MarketLabel;
use moon_core::venue::CoreVenue;
use std::collections::HashSet;

/// Build one coin hit with a stable full instrument label for grouped-search assertions.
fn coin_hit(core: u64, venue: u8, coin: &str) -> CoinHit {
    CoinHit {
        core,
        market: format!("{coin}USDT"),
        server: format!("Core {core}"),
        label: MarketLabel {
            coin: coin.to_string(),
            canonic: String::new(),
            quote: "USDT".to_string(),
            contract: None,
        },
        venue: Some(CoreVenue::identify(venue, "", None)),
    }
}

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

/// The list cap must not exceed its raw viewport budget after it rounds down to complete rows.
///
/// Breakage this pins: rounding the slot quotient upward would let the dropdown exceed its
/// intended maximum height.
#[test]
fn whole_row_cap_never_exceeds_its_raw_limit() {
    let raw_cap = 340.0;
    let cap = whole_row_cap(raw_cap, 27.0);

    assert!(
        cap <= raw_cap,
        "the complete-row cap must stay within raw limit {raw_cap}: got {cap}"
    );
}

/// The list cap must finish on a complete direct-child row at a font-scaled row height.
///
/// Breakage this pins: returning the raw cap would clip the final visible coin result when the
/// row height does not divide 340 logical pixels.
#[test]
fn whole_row_cap_is_an_integral_multiple_of_the_row_height() {
    let row_h = 27.0;
    let cap = whole_row_cap(340.0, row_h);

    assert_eq!(cap / row_h, (cap / row_h).floor());
}

/// The dropdown must preserve one visible row even when its configured cap is smaller than a row.
///
/// Breakage this pins: dropping the one-row minimum would collapse a scaled coin list to an empty
/// viewport.
#[test]
fn whole_row_cap_retains_one_row_below_the_raw_floor() {
    assert_eq!(whole_row_cap(5.0, 27.0), 27.0);
}

/// `render_popup` must cap the scroll list through the complete-row helper instead of raw 340 px.
///
/// Breakage this pins: restoring `max_h(px(340.0))` would clip the final visible result again at
/// larger font scales even while the pure helper's direct tests remain green.
#[test]
fn render_popup_wires_whole_row_cap_instead_of_raw_height() {
    let source = include_str!("../coin_search.rs");
    let popup_source = source
        .split_once("pub(crate) fn render_popup")
        .expect("coin search module must retain render_popup")
        .1;

    assert!(popup_source.contains("whole_row_cap(COIN_LIST_RAW_CAP, row_h)"));
    assert!(
        !popup_source.contains(".max_h(px(340.0))"),
        "render_popup must not restore the raw 340 px cap"
    );
}

/// `coin_search.rs::group_hits` must fold all core offerings of one instrument on one venue into
/// one expandable group while retaining every original choice in core order.
///
/// Breakage this pins: reverting grouping to one visible row per `CoinHit`. A coin available on
/// many cores would flood the dropdown and hide unrelated instruments below the scroll cap.
#[test]
fn fifty_six_cores_of_one_coin_fold_into_one_coin_row() {
    let hits = (1..=56).map(|core| coin_hit(core, 1, "BTC")).collect();

    let sections = group_hits(hits);

    assert_eq!(sections.len(), 1, "one venue must make one section");
    assert_eq!(
        sections[0].groups.len(),
        1,
        "one full instrument label on one venue must make one group"
    );
    let members = &sections[0].groups[0].members;
    assert_eq!(
        members.len(),
        56,
        "the group must retain every core offering"
    );
    assert_eq!(
        members.iter().map(|hit| hit.core).collect::<Vec<_>>(),
        (1..=56).collect::<Vec<_>>(),
        "members must preserve the canonical input order"
    );
}

/// `coin_search.rs::group_hits` must key groups by the full `MarketLabel::pair`, rather than a
/// contract-stripped search key.
///
/// Breakage this pins: changing the key to `match_key` or `display_coin`. A perpetual and dated
/// contract would merge, so selecting the visible coin could open the wrong instrument.
#[test]
fn a_dated_contract_never_groups_with_its_perpetual() {
    let perpetual = coin_hit(1, 1, "BTC");
    let dated = coin_hit(2, 1, "BTC_0925");

    assert_eq!(perpetual.label.match_key(), dated.label.match_key());
    assert_ne!(perpetual.label.pair(), dated.label.pair());

    let sections = group_hits(vec![perpetual, dated]);

    assert_eq!(sections.len(), 1, "one venue must stay one section");
    assert_eq!(
        sections[0].groups.len(),
        2,
        "distinct full instrument labels must remain distinct groups"
    );
}

/// `coin_search.rs::group_hits` must make the venue a section boundary as well as grouping by
/// full instrument label.
///
/// Breakage this pins: dropping the exchange section from the group key. Identically named
/// markets on two exchanges would collapse into one ambiguous choice.
#[test]
fn the_same_coin_on_two_exchanges_stays_in_two_groups() {
    let sections = group_hits(vec![coin_hit(1, 1, "BTC"), coin_hit(2, 2, "BTC")]);

    assert_eq!(
        sections.len(),
        2,
        "each exchange must retain its own section"
    );
    assert!(
        sections.iter().all(|section| section.groups.len() == 1),
        "each exchange section must hold its own BTC group"
    );
}

/// `coin_search.rs::direct_row_count` must count a collapsed multi-core group as one row and an
/// open multi-core group as its trigger plus its cores, while a single-member group stays one row.
///
/// Breakage this pins: dropping `group.members.len() > 1 &&` from the child-row guard. A
/// single-core group has no caret but would count as two rows, so the continuation fade appears
/// over a list with nothing below it.
#[test]
fn a_collapsed_group_is_one_row_and_an_open_one_is_its_cores() {
    let sections = group_hits(vec![
        coin_hit(1, 1, "BTC"),
        coin_hit(2, 1, "BTC"),
        coin_hit(3, 1, "ETH"),
    ]);
    let toggled = HashSet::from([sections[0].groups[0].key.clone()]);
    assert_eq!(
        direct_row_count(&sections, &toggled),
        2,
        "a collapsed multi-core group and a single-core group each contribute one trigger"
    );
    assert!(
        group_is_open(
            &sections[0].groups[0].key,
            sections[0].groups[0].members.len(),
            &HashSet::new(),
        ),
        "the two-core group must start open before its explicit toggle"
    );
    assert_eq!(
        direct_row_count(&sections, &HashSet::new()),
        4,
        "an open two-core group draws three rows and its single-core neighbour draws one"
    );
}

/// `coin_search.rs::group_starts_expanded` must expand no more than three cores by default.
///
/// Breakage this pins: raising or removing the automatic-collapse boundary. A large multi-core
/// search would consume the dropdown before the user can see other matching instruments.
#[test]
fn groups_above_three_cores_start_collapsed() {
    assert!(group_starts_expanded(3));
    assert!(!group_starts_expanded(4));
}

/// `coin_search.rs::pick_core` must prefer the active core when it offers the selected market.
///
/// Breakage this pins: always taking the first group member. Enter or click would open the same
/// coin on a foreign core despite the user searching from a narrowed workspace.
#[test]
fn pick_core_prefers_the_active_core_and_falls_back_to_the_first() {
    let members = vec![coin_hit(11, 1, "BTC"), coin_hit(22, 1, "BTC")];

    assert_eq!(pick_core(&members, Some(22)).map(|hit| hit.core), Some(22));
    assert_eq!(pick_core(&members, Some(99)).map(|hit| hit.core), Some(11));
    assert_eq!(pick_core(&members, None).map(|hit| hit.core), Some(11));
}

/// `coin_search.rs::enter_target` must open only a typed-query match, selecting its active-core
/// group member when available.
///
/// Breakage this pins: returning a suggestion from the `Suggest` arm. Pressing Enter in an empty
/// field would unexpectedly open an arbitrary top-mover chart.
#[test]
fn enter_opens_the_first_match_on_the_active_core_and_nothing_otherwise() {
    let query = CoinResults::Query(vec![coin_hit(11, 1, "BTC"), coin_hit(22, 1, "BTC")]);
    assert_eq!(
        enter_target(query, Some(22)),
        Some((22, "BTCUSDT".to_string()))
    );
    assert_eq!(
        enter_target(CoinResults::Query(Vec::new()), Some(22)),
        None,
        "an empty query has no market to open"
    );
    assert_eq!(
        enter_target(
            CoinResults::Suggest {
                recent: vec![coin_hit(11, 1, "BTC")],
                volatile: vec![coin_hit(22, 2, "ETH")],
            },
            Some(22),
        ),
        None,
        "Enter on an empty field must not select a suggestion"
    );
}
