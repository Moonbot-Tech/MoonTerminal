//! Unit checks for the Report footer's priority split.
//!
//! Explicit imports throughout: the parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute and makes `#[test]` expand recursively.

use moon_core::db::valuation::{FailureKind, ValuationFault, ValuationStage, ValuationStatus};
use moon_core::db::{QuoteBreakdown, ValuationCoverage};

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
        rows: vec![Vec::new(); shown_rows],
        core_uids: Vec::new(),
        row_keys: Vec::new(),
        totals: QuoteBreakdown::from_groups(groups),
    }
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
    let facts = footer_facts(
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
        !facts.tail.is_empty(),
        "the counts must stay clippable rather than joining the head"
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
    let facts = footer_facts(Some(&snapshot), false, &stalled(), T0 + 200_000);

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

/// A stall warning outranks every tally, and the two counts close the tail.
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
    let facts = footer_facts(Some(&snapshot), false, &stalled(), T0 + 200_000);

    assert_eq!(
        tones(&facts.tail),
        vec![
            // The stall: a number on screen may be wrong.
            FactTone::Alarm,
            // The second exact currency total.
            FactTone::Negative,
            // Rows whose currency could not be identified at all.
            FactTone::Warn,
            // Order count, then shown rows: tallies the table itself already shows.
            FactTone::Soft,
            FactTone::Soft,
        ]
    );
    assert!(
        facts.tail[0].tip.is_some(),
        "the stall marker must carry its diagnostic codes"
    );
}

/// A healthy worker states nothing, and a stall is stated whether or not the money is comparable.
///
/// Breakage: gating the health fact inside the mixed-quote branch. Most filters are
/// single-currency, so the warning would be invisible exactly when the report is simplest — and a
/// stuck worker is a property of the worker, not of the rows on screen.
#[test]
fn worker_health_is_stated_outside_every_quote_scope_branch() {
    let single = data(vec![(Some(1), 5.0, 2)], 2);

    let healthy = footer_facts(Some(&single), false, &ValuationStatus::default(), T0);
    assert!(
        !tones(&healthy.tail).contains(&FactTone::Alarm),
        "a healthy worker adds nothing"
    );

    let stuck = footer_facts(Some(&single), false, &stalled(), T0 + 200_000);
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
    let failed: FooterFacts = footer_facts(None, true, &ValuationStatus::default(), T0);
    assert_eq!(
        tones(&failed.essential),
        vec![FactTone::Soft, FactTone::Warn]
    );
    assert!(failed.tail.is_empty(), "there is nothing to qualify");

    let pending = footer_facts(None, false, &ValuationStatus::default(), T0);
    assert_eq!(pending.essential[1].text, "—");
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

    let facts = footer_facts(Some(&snapshot), false, &ValuationStatus::default(), T0);
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
        tones(&footer_facts(Some(&mixed), false, &ValuationStatus::default(), T0).essential)[1],
        FactTone::Untrusted,
        "two currencies and no unified total: the leading bucket is not the answer"
    );

    let unknown = data(vec![(Some(0), 12.5, 3), (None, 0.0, 2)], 5);
    assert_eq!(
        tones(&footer_facts(Some(&unknown), false, &ValuationStatus::default(), T0).essential)[1],
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
    let facts = footer_facts(
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
