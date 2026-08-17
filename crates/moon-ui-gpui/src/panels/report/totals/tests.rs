//! Unit checks for the Report footer's priority split.
//!
//! Explicit imports throughout: the parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute and makes `#[test]` expand recursively.

use moon_core::db::valuation::{
    FailureKind, ValuationFault, ValuationMode, ValuationStage, ValuationStatus,
};
use moon_core::db::{QuoteBreakdown, QuoteCurrency, QuoteVolume, TradedVolume, ValuationCoverage};

use super::{FactTone, FooterFacts, footer_facts, footer_tooltip};
use crate::panels::report::query::ReportData;

/// Wall clock the fixtures fail at, far enough from zero that elapsed time is unambiguous.
const T0: i64 = 1_700_000_000_000;

/// The fixture worker's reported cause. Distinctive on purpose: it is the independent oracle for
/// "a fact's diagnostic reaches the row tooltip", which no formatting code can synthesize.
const FAULT_DETAIL: &str = "route unreachable at 09:41";

/// Build a snapshot with the given `(quote ordinal, profit, orders)` groups and row count.
fn data(groups: Vec<(Option<i64>, f64, i64)>, shown_rows: usize) -> ReportData {
    ReportData {
        filter: Default::default(),
        rows: vec![Vec::new(); shown_rows],
        core_uids: Vec::new(),
        row_keys: Vec::new(),
        totals: QuoteBreakdown::from_groups(groups),
        valuation: ValuationMode::Historical,
    }
}

/// Attach an independently assembled traded-volume carrier to a Report snapshot.
///
/// Args:
///     snapshot: Loaded Report result whose profit/count behavior stays intact.
///     volume: Volume state to render, complete or partial.
///
/// Returns:
///     The same snapshot carrying the supplied volume state.
fn with_volume(mut snapshot: ReportData, volume: TradedVolume) -> ReportData {
    snapshot.totals.traded_volume = volume;
    snapshot
}

/// A worker that has been failing long enough and often enough to report as stuck.
///
/// The fault is built from its public fields rather than through `FaultCause`, which stays
/// crate-private to moon-core: only the worker gets to mint a cause.
fn stalled() -> ValuationStatus {
    let mut status = ValuationStatus::default();
    let fault = ValuationFault {
        stage: ValuationStage::Reconcile,
        kind: FailureKind::Provider,
        detail: FAULT_DETAIL.to_string(),
    };
    for _ in 0..3 {
        status.record_failure(fault.clone(), T0);
    }
    status
}

/// Assemble the footer over a snapshot built by [`data`], which is historical by default.
///
/// Every priority-order case is independent of the conversion, so none of them says which one it
/// is using.
fn historical_facts(
    data: Option<&ReportData>,
    failed: bool,
    status: &ValuationStatus,
    now_ms: i64,
) -> FooterFacts {
    footer_facts(data, failed, status, now_ms)
}

/// Collect the tones of a fact list, which is what the priority order is asserted against.
fn tones(facts: &[super::FooterFact]) -> Vec<FactTone> {
    facts.iter().map(|fact| fact.tone).collect()
}

/// The head is the caption plus exactly one money figure, and a lone exact currency owns it at full
/// sign strength.
///
/// Breakage: moving the money figure into the tail (`essential.push` -> `tail.push` for the primary
/// total), which lets the one number the row exists to state be clipped away on a narrow dock while
/// the counts qualifying it stay on screen.
#[test]
fn the_head_is_the_caption_and_one_money_figure() {
    let facts = historical_facts(
        Some(&data(vec![(Some(0), 12.5, 3)], 5)),
        false,
        &ValuationStatus::default(),
        T0,
    );

    assert_eq!(
        facts.essential.len(),
        2,
        "the head is exactly the caption plus the leading money figure"
    );
    assert_eq!(tones(&facts.essential)[1], FactTone::Positive);
    assert!(
        !facts.trailing.is_empty(),
        "the shown-rows tally must stay outside the head rather than joining it"
    );
}

/// A per-fact diagnostic must reach the ROW tooltip, not only the fact's own hover target.
///
/// The oracle is the fixture's own detail string, which `footer_tooltip` never computes — it can
/// only pass it through. Breakage: building the tooltip from `fact.text` alone. The stall marker
/// sits in the clipping tail, so once it clips there is no element left to hover, and its cause,
/// codes and duration become unreachable exactly on the narrow dock the tooltip exists to rescue.
#[test]
fn a_clipped_facts_diagnostic_survives_in_the_row_tooltip() {
    let snapshot = data(vec![(Some(0), 12.5, 3)], 5);
    let facts = historical_facts(Some(&snapshot), false, &stalled(), T0 + 200_000);

    let tip = footer_tooltip(&facts);
    assert!(
        tip.contains(FAULT_DETAIL),
        "the worker's reported cause must survive into the row tooltip, got {tip:?}"
    );
    assert!(
        tip.contains("reconcile/provider"),
        "so must the machine codes a user is asked to quote"
    );
}

/// A stall warning outranks every tally, and the money facts close the tail.
///
/// The order IS the contract: facts are laid out from highest to lowest priority, and clipping at
/// the right edge removes the later facts first. Breakage: inserting the stall marker after a tally,
/// which hides a wrong-number warning before a count the table itself already exposes.
#[test]
fn a_stall_leads_the_tail_and_the_counts_close_it() {
    let snapshot = data(
        vec![(Some(0), 12.5, 3), (Some(2), -0.25, 2), (None, 0.0, 4)],
        5,
    );
    let facts = historical_facts(Some(&snapshot), false, &stalled(), T0 + 200_000);

    assert_eq!(
        tones(&facts.tail),
        vec![
            // The stall: a number on screen may be wrong.
            FactTone::Alarm,
            // The second exact currency total.
            FactTone::Negative,
            // Rows whose currency could not be identified at all.
            FactTone::Warn,
        ]
    );
    assert!(
        facts.tail[0].tip.is_some(),
        "the stall marker must carry its diagnostic codes"
    );
}

/// The tooltip spells out what the caption's bare number counts.
///
/// The row is one fixed-height line, so it can only afford "Total (7):"; the tooltip is not
/// constrained that way and names the unit. Breakage: building the tooltip from `fact.text` alone,
/// which silently drops every spelled-out form the moment one is introduced — the count then reads
/// as an unlabelled number in the one place there was room to explain it.
#[test]
fn the_tooltip_spells_out_the_caption_count() {
    let snapshot = data(vec![(Some(0), 12.5, 3), (Some(2), -0.25, 4)], 2);
    let facts = historical_facts(Some(&snapshot), false, &ValuationStatus::default(), T0);

    let spelled = facts.essential[0]
        .spelled
        .as_deref()
        .expect("the caption carries a spelled-out form");
    assert!(spelled.contains('7'), "got {spelled:?}");
    assert_ne!(
        spelled, facts.essential[0].text,
        "the spelled form must differ from the row's abbreviation"
    );

    let tip = footer_tooltip(&facts);
    assert!(tip.contains(spelled), "the tooltip states it, got {tip:?}");
    assert!(
        !tip.contains(facts.essential[0].text.as_str()),
        "and not the abbreviation as well, got {tip:?}"
    );
}

/// The shown-rows tally leaves the clipping tail and holds the row's right edge.
///
/// It describes the grid, not the money, so it is not competing for the same space as the figures.
/// Breakage: pushing it back onto `tail`, where it both trails the amounts and is clipped away by a
/// narrow dock — and where the row's right edge goes empty.
#[test]
fn the_shown_rows_tally_is_pinned_right_rather_than_clipped() {
    let snapshot = data(vec![(Some(0), 12.5, 3)], 5);
    let facts = historical_facts(Some(&snapshot), false, &ValuationStatus::default(), T0);

    assert_eq!(facts.trailing.len(), 1, "exactly the shown-rows tally");
    assert!(
        facts.trailing[0].text.contains('5'),
        "got {:?}",
        facts.trailing[0].text
    );
    assert!(
        !facts
            .tail
            .iter()
            .chain(&facts.essential)
            .any(|fact| fact.text == facts.trailing[0].text),
        "and it is stated once, not in two groups"
    );
    assert!(
        footer_tooltip(&facts).contains(facts.trailing[0].text.as_str()),
        "a pinned fact must still reach the row tooltip"
    );
}

/// The order count rides in the caption, where clipping cannot reach it.
///
/// The caption states what the money figure beside it is a total OF. Breakage: pushing the count
/// back into the tail, where a narrow dock drops it first and leaves a sum with no denominator —
/// which is exactly the fact a user reads the footer for.
#[test]
fn the_caption_states_the_order_count_and_the_tail_no_longer_does() {
    // 3 + 4 orders over 2 shown rows: the count "7" appears in no amount and in no other tally,
    // so finding it is evidence about the caption rather than about the fixture.
    let snapshot = data(vec![(Some(0), 12.5, 3), (Some(2), -0.25, 4)], 2);
    let facts = historical_facts(Some(&snapshot), false, &ValuationStatus::default(), T0);

    let caption = &facts.essential[0].text;
    assert!(
        caption.contains('7'),
        "the caption must carry the 7 summed orders, got {caption:?}"
    );
    assert!(
        !facts.tail.iter().any(|fact| fact.text.contains('7')),
        "and no tail fact may restate it, got {:?}",
        facts
            .tail
            .iter()
            .map(|fact| fact.text.as_str())
            .collect::<Vec<_>>()
    );
}

/// The per-currency totals read as ONE bracketed breakdown of the figure above them.
///
/// The row states "Total (401): +500 USDT (+600 USDT -100 USDC)", so the brackets belong to the
/// group, not to each amount. Breakage: bracketing every currency separately, or gluing both
/// brackets onto the first one — either turns the breakdown into a list of parenthesised asides
/// that no longer reads as the decomposition of the headline figure.
#[test]
fn the_currency_breakdown_is_wrapped_in_one_pair_of_brackets() {
    let mut snapshot = data(vec![(Some(0), 12.5, 3), (Some(2), -0.25, 2)], 5);
    snapshot.totals = snapshot.totals.with_valuation(ValuationCoverage {
        eligible_orders: 5,
        valued_orders: 5,
        unavailable_orders: 0,
        usdt: Some(moon_core::db::UsdtTotal {
            profit: 99.0,
            spent: None,
        }),
    });

    let facts = historical_facts(Some(&snapshot), false, &ValuationStatus::default(), T0);
    let breakdown: Vec<&str> = facts
        .tail
        .iter()
        .map(|fact| fact.text.as_str())
        .filter(|text| text.contains("12.5") || text.contains("0.25"))
        .collect();

    assert_eq!(breakdown.len(), 2, "got {breakdown:?}");
    assert!(
        breakdown[0].starts_with('(') && !breakdown[0].ends_with(')'),
        "the first amount opens the group, got {:?}",
        breakdown[0]
    );
    assert!(
        breakdown[1].ends_with(')') && !breakdown[1].starts_with('('),
        "the last amount closes it, got {:?}",
        breakdown[1]
    );
}

/// A lone currency total in the breakdown carries BOTH brackets.
///
/// Reached with two currencies and no unified figure: the first is promoted into the head, leaving
/// exactly one behind. Breakage: opening the group on the first fact and closing it on a separately
/// tracked "previous" index, which for a one-element breakdown applies only one of the two and
/// ships a stray unmatched bracket into the footer.
#[test]
fn a_single_currency_breakdown_is_bracketed_on_both_sides() {
    let snapshot = data(vec![(Some(0), 12.5, 3), (Some(2), -0.25, 2)], 5);

    let facts = historical_facts(Some(&snapshot), false, &ValuationStatus::default(), T0);
    let only = facts
        .tail
        .iter()
        .find(|fact| fact.text.contains("0.25"))
        .map(|fact| fact.text.as_str())
        .expect("the second currency total is the whole breakdown");

    assert!(only.starts_with('(') && only.ends_with(')'), "got {only:?}");
}

/// A healthy worker states nothing, and a stall is stated whether or not the money is comparable.
///
/// Breakage: gating the health fact inside the mixed-quote branch. Most filters are
/// single-currency, so the warning would be invisible exactly when the report is simplest — and a
/// stuck worker is a property of the worker, not of the rows on screen.
#[test]
fn worker_health_is_stated_outside_every_quote_scope_branch() {
    let single = data(vec![(Some(1), 5.0, 2)], 2);

    let healthy = historical_facts(Some(&single), false, &ValuationStatus::default(), T0);
    assert!(
        !tones(&healthy.tail).contains(&FactTone::Alarm),
        "a healthy worker adds nothing"
    );

    let stuck = historical_facts(Some(&single), false, &stalled(), T0 + 200_000);
    assert_eq!(
        tones(&stuck.tail).first(),
        Some(&FactTone::Alarm),
        "one exact currency must not suppress the stall warning"
    );
}

/// An absent snapshot states why, and never a fabricated zero.
///
/// Breakage: rendering the totals of an empty snapshot instead of the dash, which prints `+0.00`
/// and `0 orders` for a read that simply has not landed — indistinguishable from a genuinely empty
/// period.
#[test]
fn an_absent_snapshot_states_the_read_rather_than_a_zero() {
    let failed: FooterFacts = historical_facts(None, true, &ValuationStatus::default(), T0);
    assert_eq!(
        tones(&failed.essential),
        vec![FactTone::Soft, FactTone::Warn]
    );
    assert!(
        failed.tail.is_empty() && failed.trailing.is_empty(),
        "there is nothing to qualify, and no row count to pin either"
    );

    let pending = historical_facts(None, false, &ValuationStatus::default(), T0);
    assert_eq!(pending.essential[1].text, "—");
}

/// A current-rate figure must never wear the sentence that claims historical profit.
///
/// Breakage: `totals.rs::footer_facts` ignoring `data.valuation` and always reaching for
/// `report.valuation_total`. Both conversions would then render the identical caption, and a
/// number that says what the position would be worth today would read as what it actually made.
///
/// The conversion is read off the SNAPSHOT, never off the live setting, so that a mode change
/// whose requery is still running cannot put the new mode's words under the old mode's numbers.
#[test]
fn a_current_rate_total_is_labelled_apart_from_historical_profit() {
    let head_under = |mode| {
        let mut snapshot = data(vec![(Some(0), 12.5, 3), (Some(2), -0.25, 2)], 5);
        snapshot.totals = snapshot.totals.with_valuation(ValuationCoverage {
            eligible_orders: 5,
            valued_orders: 5,
            unavailable_orders: 0,
            usdt: Some(moon_core::db::UsdtTotal {
                profit: 99.0,
                spent: None,
            }),
        });
        snapshot.valuation = mode;
        footer_facts(Some(&snapshot), false, &ValuationStatus::default(), T0).essential[1]
            .text
            .clone()
    };

    let historical = head_under(ValuationMode::Historical);
    let current = head_under(ValuationMode::Current);
    assert!(
        historical.contains("99") && current.contains("99"),
        "both state the same amount, got {historical:?} and {current:?}"
    );
    assert_ne!(
        historical, current,
        "the two conversions must not share one sentence"
    );
}

/// A complete unified USDT total takes the head, and every exact currency total moves to the tail.
///
/// Breakage: leading with the first quote total while a unified figure exists, so a mixed-currency
/// report's headline number is one currency out of several — a partial sum wearing the total's slot.
/// Asserted on TEXT, because tone cannot tell a `+99.00` unified figure from a `+12.50` currency
/// total: both are simply positive.
#[test]
fn a_unified_usdt_total_outranks_the_currency_it_was_built_from() {
    let mut snapshot = data(vec![(Some(0), 12.5, 3), (Some(2), -0.25, 2)], 5);
    snapshot.totals = snapshot.totals.with_valuation(ValuationCoverage {
        eligible_orders: 5,
        valued_orders: 5,
        unavailable_orders: 0,
        usdt: Some(moon_core::db::UsdtTotal {
            profit: 99.0,
            spent: None,
        }),
    });

    let facts = historical_facts(Some(&snapshot), false, &ValuationStatus::default(), T0);
    let head = &facts.essential[1].text;

    assert_eq!(facts.essential.len(), 2);
    assert!(
        head.contains("99") && !head.contains("12.5"),
        "the unified figure owns the head, not the first currency bucket, got {head:?}"
    );
    assert!(
        !facts.tail.iter().any(|fact| fact.text.contains("99")),
        "the unified figure must not also be clippable"
    );
    assert_eq!(
        tones(&facts.tail)[..2],
        [FactTone::Positive, FactTone::Negative],
        "both currency totals move into the tail behind it, keeping their signs"
    );
    assert_eq!(
        tones(&facts.essential)[1],
        FactTone::Positive,
        "a complete unified total IS the period's answer, so it keeps full sign strength"
    );
}

/// A figure that is only one currency out of several must not wear the confident sign colour.
///
/// `totals.totals` is ordered by quote ordinal, not by magnitude or row count, so the promoted
/// bucket may represent a minority of the rows — and the tail clips with no ellipsis to hint the
/// rest exists. Colour is the clip-proof half of that warning, the same signal the Assets footer
/// carries. Breakage: dropping the `comparable` gate, which restores a partial sum to the headline
/// slot in full green as though it were the period's result.
///
/// Asserted against [`FactTone::Untrusted`], which nothing else in the row carries: were the
/// demotion spelled as the ordinary muted tone, this would also pass for a figure that merely
/// rounded to zero, and the invariant it names would not be pinned at all.
#[test]
fn a_partial_headline_figure_is_stated_in_the_untrusted_tone() {
    let mixed = data(vec![(Some(0), 12.5, 3), (Some(2), -0.25, 2)], 5);
    assert_eq!(
        tones(&historical_facts(Some(&mixed), false, &ValuationStatus::default(), T0).essential)[1],
        FactTone::Untrusted,
        "two currencies and no unified total: the leading bucket is not the answer"
    );

    let unknown = data(vec![(Some(0), 12.5, 3), (None, 0.0, 2)], 5);
    assert_eq!(
        tones(&historical_facts(Some(&unknown), false, &ValuationStatus::default(), T0).essential)
            [1],
        FactTone::Untrusted,
        "rows of unidentified currency make the known total partial too"
    );
}

/// A loaded but empty result promotes no figure at all.
///
/// Breakage: falling back to an em dash when no currency bucket exists. That glyph is this footer's
/// word for "the read has not landed", and a genuinely empty period borrowing it makes a settled
/// answer indistinguishable from a pending one.
#[test]
fn a_loaded_empty_result_promotes_no_figure() {
    let facts = historical_facts(
        Some(&data(Vec::new(), 0)),
        false,
        &ValuationStatus::default(),
        T0,
    );

    assert_eq!(
        facts.essential.len(),
        1,
        "the caption stands alone, got {:?}",
        facts
            .essential
            .iter()
            .map(|fact| fact.text.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        !facts.tail.iter().any(|fact| fact.text == "—"),
        "and the dash does not reappear in the tail either"
    );
}

/// Breakage 1: `totals.rs:native_volume` replaces `compact_si` with the exact decimal formatter,
/// leaving a seven-digit raw amount unreadable in the fixed-height footer. Breakage 2: the same
/// helper reuses `compact_si` for the recovery representation, so hover can no longer reveal the
/// exact full numeric amount. The remaining assertions keep unsigned accounting, complete mixed
/// scopes, the separator and clipping-tooltip reachability pinned alongside those representations.
#[test]
fn traded_volume_is_unsigned_complete_separated_and_tooltip_recoverable() {
    let usdt = QuoteCurrency::from_report_ordinal(1).expect("USDT ordinal");
    let usdc = QuoteCurrency::from_report_ordinal(8).expect("USDC ordinal");
    let single = with_volume(
        data(vec![(Some(1), 12.5, 2)], 2),
        TradedVolume {
            totals: vec![QuoteVolume {
                currency: usdt,
                amount: 1_617_960.71,
                orders: 2,
                reconstructed: 2,
            }],
            eligible_orders: 2,
            reconstructed_orders: 2,
            valued_orders: 2,
            usdt: Some(1_617_960.71),
            ..Default::default()
        },
    );
    let facts = historical_facts(Some(&single), false, &ValuationStatus::default(), T0);
    let volume = facts.tail.last().expect("volume closes the clipping tail");
    assert_eq!(volume.text, "Volume: 1.62M USDT");
    assert_eq!(volume.tone, FactTone::Soft);
    assert!(!volume.bold, "volume must not borrow profit emphasis");
    assert!(
        volume.section_start,
        "the first volume fact owns the separator"
    );
    assert!(
        !volume.text.contains('+') && !volume.text.contains('-'),
        "traded volume is unsigned"
    );
    let spelled = volume
        .spelled
        .as_deref()
        .expect("clipped compact volume carries recovery text");
    assert_eq!(spelled, "Total traded volume: 1617960.71 USDT");
    let tooltip = footer_tooltip(&facts);
    assert!(tooltip.contains(spelled));
    assert!(
        !tooltip.contains("1.62M USDT"),
        "the recovery tooltip must retain the exact amount, got {tooltip:?}"
    );

    let mut mixed = with_volume(
        data(vec![(Some(1), 12.5, 1), (Some(8), 2.0, 1)], 2),
        TradedVolume {
            totals: vec![
                QuoteVolume {
                    currency: usdt,
                    amount: 12_345.67,
                    orders: 1,
                    reconstructed: 1,
                },
                QuoteVolume {
                    currency: usdc,
                    amount: 2_345_678_901.23,
                    orders: 1,
                    reconstructed: 1,
                },
            ],
            eligible_orders: 2,
            reconstructed_orders: 2,
            valued_orders: 2,
            usdt: Some(1_617_960.71),
            ..Default::default()
        },
    );
    let facts = historical_facts(Some(&mixed), false, &ValuationStatus::default(), T0);
    assert_eq!(
        facts.tail.last().expect("unified volume").text,
        "Volume: 1.62M USDT",
        "mixed scopes prefer their complete active-mode USDT amount"
    );
    mixed.valuation = ValuationMode::Current;
    let facts = footer_facts(Some(&mixed), false, &ValuationStatus::default(), T0);
    assert_eq!(
        facts.tail.last().expect("current unified volume").text,
        "Volume at the current rate: 1.62M USDT",
        "the loaded current-rate mode must remain visible on a unified conversion"
    );

    let mut native_mixed = mixed;
    native_mixed.totals.traded_volume.usdt = None;
    native_mixed.totals.traded_volume.valued_orders = 0;
    let facts = historical_facts(Some(&native_mixed), false, &ValuationStatus::default(), T0);
    assert_eq!(
        facts.tail.last().expect("native mixed volume").text,
        "Volume: 12.3K USDT + 2.35B USDC",
        "every native bucket remains explicit and SI-compacted when unified valuation is unavailable"
    );
    assert_eq!(
        facts.tail.last().and_then(|fact| fact.spelled.as_deref()),
        Some("Total traded volume: 12345.67 USDT + 2345678901.23 USDC"),
        "the mixed-native recovery text retains every exact full amount"
    );
}

/// `totals.rs:traded_volume_amount` must keep a complete native bucket when another bucket is
/// unreconstructable; restoring the old all-or-nothing collection hides known traded volume from a
/// Full summary, while empty/loading/failed states must still never fabricate a volume fact.
#[test]
fn traded_volume_keeps_its_known_buckets_and_is_absent_only_with_nothing_provable() {
    let usdt = QuoteCurrency::from_report_ordinal(1).expect("USDT ordinal");
    let usdc = QuoteCurrency::from_report_ordinal(8).expect("USDC ordinal");
    let incomplete = with_volume(
        data(vec![(Some(1), 12.5, 1), (Some(8), 2.0, 1)], 2),
        TradedVolume {
            totals: vec![
                QuoteVolume {
                    currency: usdt,
                    amount: 420.0,
                    orders: 1,
                    reconstructed: 1,
                },
                QuoteVolume {
                    currency: usdc,
                    amount: 0.0,
                    orders: 1,
                    reconstructed: 0,
                },
            ],
            eligible_orders: 2,
            reconstructed_orders: 1,
            ..Default::default()
        },
    );
    let facts = historical_facts(Some(&incomplete), false, &ValuationStatus::default(), T0);
    let volume = facts
        .tail
        .iter()
        .find(|fact| fact.section_start)
        .expect("a complete USDT bucket must remain visible beside the incomplete warning");
    assert_eq!(volume.text, "Volume (partial): 420 USDT");
    assert_eq!(volume.tone, FactTone::Warn);

    for (snapshot, failed) in [(None, false), (None, true)] {
        let facts = historical_facts(snapshot, failed, &ValuationStatus::default(), T0);
        assert!(facts.tail.iter().all(|fact| !fact.section_start));
    }
    let empty = data(Vec::new(), 0);
    let facts = historical_facts(Some(&empty), false, &ValuationStatus::default(), T0);
    assert!(
        facts.tail.iter().all(|fact| !fact.section_start),
        "an empty loaded result may not render an orphan volume separator"
    );
}

/// `totals.rs:traded_volume_amount` must not restore the all-or-nothing native-volume guard;
/// otherwise a Full summary with one unreconstructable USDC bucket silently loses the complete
/// USDT volume instead of warning that one order and its quote currency were withheld.
#[test]
fn partial_traded_volume_keeps_the_known_bucket_and_spells_its_shortfall() {
    let usdt = QuoteCurrency::from_report_ordinal(1).expect("USDT ordinal");
    let usdc = QuoteCurrency::from_report_ordinal(8).expect("USDC ordinal");
    let incomplete = with_volume(
        data(vec![(Some(1), 12.5, 1), (Some(8), 2.0, 1)], 2),
        TradedVolume {
            totals: vec![
                QuoteVolume {
                    currency: usdt,
                    amount: 420.0,
                    orders: 1,
                    reconstructed: 1,
                },
                QuoteVolume {
                    currency: usdc,
                    amount: 0.0,
                    orders: 1,
                    reconstructed: 0,
                },
            ],
            eligible_orders: 2,
            reconstructed_orders: 1,
            ..Default::default()
        },
    );

    let facts = historical_facts(Some(&incomplete), false, &ValuationStatus::default(), T0);
    let volume = facts
        .tail
        .iter()
        .find(|fact| fact.section_start)
        .expect("the independently complete USDT bucket must produce one volume fact");
    assert_eq!(volume.text, "Volume (partial): 420 USDT");
    assert_eq!(volume.tone, FactTone::Warn);
    assert_eq!(
        volume.spelled.as_deref(),
        Some(
            "Traded volume over the reconstructed trades: 420 USDT. Orders not accounted for: 1 (USDC)"
        ),
        "the recovery text must identify the independently withheld order and its persisted quote"
    );
    let tooltip = footer_tooltip(&facts);
    assert!(
        tooltip.contains("Orders not accounted for: 1 (USDC)"),
        "the row tooltip must retain the partial fact's independently assembled shortfall, got {tooltip:?}"
    );
}

/// `totals.rs:traded_volume_amount` must reserve unified USDT conversion for Mixed scopes; widening
/// that shortcut to a single USDC bucket swaps its persisted native amount for a rate-derived USDT
/// figure and, in current valuation mode, gives a lone-currency Report the wrong money and wording.
#[test]
fn single_quote_volume_keeps_its_persisted_native_amount() {
    let usdc = QuoteCurrency::from_report_ordinal(8).expect("USDC ordinal");
    let single = with_volume(
        data(vec![(Some(8), 12.5, 1)], 1),
        TradedVolume {
            totals: vec![QuoteVolume {
                currency: usdc,
                amount: 12.5,
                orders: 1,
                reconstructed: 1,
            }],
            eligible_orders: 1,
            reconstructed_orders: 1,
            valued_orders: 1,
            usdt: Some(20.0),
            ..Default::default()
        },
    );

    let facts = historical_facts(Some(&single), false, &ValuationStatus::default(), T0);
    let volume = facts
        .tail
        .iter()
        .find(|fact| fact.section_start)
        .expect("a complete single-currency scope must state its native volume");
    assert_eq!(volume.text, "Volume: 12.5 USDC");
    assert_eq!(volume.tone, FactTone::Soft);
}

/// The regression the previous fix missed: a SINGLE-currency scope has no second bucket, so
/// deciding completeness inside the bucket blanked the whole footer figure over one liquidation out
/// of a thousand trades. Restoring an all-or-nothing bucket — dropping `QuoteVolume::reconstructed`
/// and gating the amount on `orders == reconstructed` again — turns the first half red; counting
/// the shortfall per BUCKET instead of per ROW turns the mixed half red, because a bucket that is
/// stated AND short would then contribute nothing to the gap.
#[test]
fn partial_single_currency_volume_is_stated_and_marked_rather_than_withheld() {
    let usdt = QuoteCurrency::from_report_ordinal(1).expect("USDT ordinal");
    let usdc = QuoteCurrency::from_report_ordinal(8).expect("USDC ordinal");
    let single = with_volume(
        data(vec![(Some(1), 322.79, 1004)], 1004),
        TradedVolume {
            totals: vec![QuoteVolume {
                currency: usdt,
                amount: 1_234_567.89,
                orders: 1004,
                reconstructed: 1003,
            }],
            eligible_orders: 1004,
            reconstructed_orders: 1003,
            ..Default::default()
        },
    );

    let facts = historical_facts(Some(&single), false, &ValuationStatus::default(), T0);
    let volume = facts
        .tail
        .iter()
        .find(|fact| fact.section_start)
        .expect("one unreconstructable trade may not erase the only bucket's volume");
    assert_eq!(volume.text, "Volume (partial): 1.23M USDT");
    assert_eq!(
        volume.tone,
        FactTone::Warn,
        "a partial subtotal must never wear the tone of a complete filter total"
    );
    assert_eq!(
        volume.spelled.as_deref(),
        Some(
            "Traded volume over the reconstructed trades: 1234567.89 USDT. Orders not accounted for: 1 (USDT)"
        ),
        "the shortfall names the one row and the quote it belongs to"
    );
    assert!(
        footer_tooltip(&facts).contains("Orders not accounted for: 1 (USDT)"),
        "the shortfall must survive clipping through the shared row tooltip"
    );

    // A mixed scope whose buckets are BOTH stated while one is short: the gap is a row count, not a
    // bucket count, so the partially reconstructed USDT bucket keeps its money and still warns.
    let mixed = with_volume(
        data(vec![(Some(1), 12.5, 4), (Some(8), 2.0, 2)], 6),
        TradedVolume {
            totals: vec![
                QuoteVolume {
                    currency: usdt,
                    amount: 900.0,
                    orders: 4,
                    reconstructed: 3,
                },
                QuoteVolume {
                    currency: usdc,
                    amount: 75.0,
                    orders: 2,
                    reconstructed: 2,
                },
            ],
            eligible_orders: 6,
            reconstructed_orders: 5,
            ..Default::default()
        },
    );
    let facts = historical_facts(Some(&mixed), false, &ValuationStatus::default(), T0);
    let volume = facts
        .tail
        .iter()
        .find(|fact| fact.section_start)
        .expect("both known buckets must remain stated");
    assert_eq!(volume.text, "Volume (partial): 900 USDT + 75 USDC");
    assert_eq!(volume.tone, FactTone::Warn);
    assert_eq!(
        volume.spelled.as_deref(),
        Some(
            "Traded volume over the reconstructed trades: 900 USDT + 75 USDC. Orders not accounted for: 1 (USDT)"
        ),
        "only the short bucket names itself, and only its missing row is counted"
    );
}
