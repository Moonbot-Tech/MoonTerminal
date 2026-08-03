//! Pure assembly of the Report totals row: which facts it states, the order in which they may be
//! clipped, and the tooltip that keeps every clipped one reachable.
//!
//! The facts must stay on one fixed-height line inside a dock the user may drag arbitrarily narrow,
//! and a dock panel cannot measure itself (`design::ticker_visible` needs a window render root). So
//! instead of breakpoints the facts are split in two: an essential head that never yields, and a
//! tail laid out left to right in descending priority. The tail clips at its right edge, so later
//! facts disappear first. Assembling that split here rather than in the render file makes the order
//! testable, and lets the tooltip be built from the very strings the
//! row renders — so "everything clippable stays reachable" holds by construction rather than by two
//! call sites agreeing. This module is the canonical statement of that reasoning; the render file
//! documents only what it alone knows, which is how the split is expressed in flex.

use moon_core::db::{self, valuation::ValuationStatus};
use moon_core::util::fmt::{self, DeltaSign};
use rust_i18n::t;

use super::query::ReportData;
use crate::valuation_health;

/// Colour role of one footer fact, resolved against the palette by the render file.
///
/// Kept as a role rather than a resolved colour so this module stays free of the active theme,
/// which is what makes the priority order unit-testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FactTone {
    /// Ordinary supporting text, and money that rounds to exactly zero.
    Soft,
    /// Money above zero.
    Positive,
    /// Money below zero.
    Negative,
    /// A real figure that is nonetheless NOT the answer it occupies the slot of. Distinct from
    /// [`FactTone::Soft`] on purpose: a trust demotion that shared a value with every caption could
    /// not be asserted, and the whole point is that this signal survives clipping.
    Untrusted,
    /// An incomplete, unknown or unavailable figure the user should notice.
    Warn,
    /// Valuation has stopped making progress, so a number on screen may be wrong.
    Alarm,
}

/// One rendered footer item, already localized.
pub(super) struct FooterFact {
    /// Exactly the text the row renders, and exactly what the tooltip repeats.
    pub(super) text: String,
    /// Colour role.
    pub(super) tone: FactTone,
    /// Whether the figure carries the row's bold weight.
    pub(super) bold: bool,
    /// Diagnostic detail shown on this fact and repeated in the shared recovery tooltip.
    pub(super) tip: Option<String>,
}

/// The totals row split by what may disappear on a narrow dock.
pub(super) struct FooterFacts {
    /// Never clipped: the caption and the one money figure the row exists to state.
    pub(super) essential: Vec<FooterFact>,
    /// Clipped from the right; earlier entries survive longer.
    pub(super) tail: Vec<FooterFact>,
}

/// Build one plain fact with no diagnostic detail.
///
/// Args:
///     text: Localized text rendered for the fact.
///     tone: Theme-independent colour role.
///     bold: Whether the rendered text uses the footer's bold weight.
///
/// Returns:
///     A fact ready for the essential or clipping group.
fn fact(text: String, tone: FactTone, bold: bool) -> FooterFact {
    FooterFact {
        text,
        tone,
        bold,
        tip: None,
    }
}

/// Choose a money colour role from the sign of the rounded display amount.
///
/// Args:
///     sign: Classification returned by the shared amount formatter.
///
/// Returns:
///     Positive, negative, or soft-zero tone matching the rendered sign.
fn money_tone(sign: DeltaSign) -> FactTone {
    sign.pick(FactTone::Positive, FactTone::Negative, FactTone::Soft)
}

/// State the valuation worker's trouble, if it has any.
///
/// Returns the stall marker in the alarm tone with the machine codes on its own tooltip, or the
/// quiet retry note, or nothing. A short failing run can be an ordinary rate-limit backoff, so it
/// is stated softly rather than in the colour reserved for a wrong number.
///
/// Args:
///     status: Health published by the valuation worker.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     The current health fact, or `None` while the worker is healthy.
fn health_fact(status: &ValuationStatus, now_ms: i64) -> Option<FooterFact> {
    if let Some(facts) = valuation_health::stall_facts(status, now_ms) {
        // The marker states the cause in words; the machine codes ride in its tooltip, where a user
        // can quote them into a bug report without them leaking into a translated line.
        let tip = t!(
            "report.valuation_stalled_tip",
            stage = facts.stage,
            kind = facts.kind,
            codes = facts.codes,
            minutes = facts.minutes,
            detail = facts.detail
        )
        .to_string();
        return Some(FooterFact {
            tip: Some(tip),
            ..fact(
                t!("report.valuation_stalled").to_string(),
                FactTone::Alarm,
                true,
            )
        });
    }
    status.is_retrying().then(|| {
        fact(
            t!("report.valuation_retrying").to_string(),
            FactTone::Soft,
            false,
        )
    })
}

/// Assemble every fact the totals row states, split by whether it may be clipped.
///
/// The tail order is the priority order and is deliberate. A valuation stall leads it because it
/// warns that a number already on screen may be wrong, and that outranks any tally. The remaining
/// quote totals come next because a missing currency total silently changes what the row appears to
/// sum, whereas a missing row count only withholds a tally the table itself shows. The two counts
/// therefore close the tail and are the first to go.
///
/// Args:
///     data: Current report snapshot, or `None` while none is renderable.
///     failed: Whether the absent snapshot is a failed read rather than a pending one.
///     status: Health published by the valuation worker.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     The essential head and the clip-ordered tail.
pub(super) fn footer_facts(
    data: Option<&ReportData>,
    failed: bool,
    status: &ValuationStatus,
    now_ms: i64,
) -> FooterFacts {
    let mut essential = vec![fact(t!("report.totals").to_string(), FactTone::Soft, false)];
    let mut tail = Vec::new();

    let Some(data) = data else {
        // Without current data, never render +0.00 / 0 orders: those values are indistinguishable
        // from a genuinely empty period. A read that fails or has not landed also clears the
        // selection, so no commands share the row in this arm.
        essential.push(if failed {
            fact(
                t!("common.db_read_failed_short").to_string(),
                FactTone::Warn,
                true,
            )
        } else {
            fact("—".to_string(), FactTone::Soft, true)
        });
        return FooterFacts { essential, tail };
    };

    let totals = &data.totals;
    // A unified USDT figure is offered only where the raw money is not comparable as one scalar.
    // Under a single known currency its own total already IS the answer.
    let mixed = matches!(
        totals.scope(),
        db::QuoteScope::Mixed | db::QuoteScope::Unknown
    );
    let unified = mixed.then(|| totals.unified_usdt()).flatten();
    // Whether the promoted figure is the period's ANSWER or merely one currency out of several.
    // `totals.totals` is ordered by quote ordinal — not by magnitude, not by row count — so the
    // leading bucket can represent a minority of the rows. Wearing the confident sign colour in the
    // never-clipped slot is how such a partial sum gets read as the total, and the tail clips with
    // no ellipsis to hint that the rest exists.
    let comparable = unified.is_some() || (totals.totals.len() <= 1 && totals.unknown_orders == 0);

    let mut quotes = totals.totals.iter();
    let promoted = match unified {
        Some(usdt) => {
            let (amount, sign) = fmt::signed_amount(usdt.profit, 2);
            Some((
                t!("report.valuation_total", amount = amount).to_string(),
                sign,
            ))
        }
        // With no unified figure the first exact currency total leads and the rest join the tail.
        // A loaded result with no known currency at all promotes NOTHING: an em dash here is the
        // glyph for "the read has not landed", and a real empty period must not borrow it.
        None => quotes.next().map(|total| total.signed_display()),
    };
    essential.extend(promoted.map(|(text, sign)| {
        let tone = if comparable {
            money_tone(sign)
        } else {
            FactTone::Untrusted
        };
        fact(text, tone, true)
    }));

    if let Some(health) = health_fact(status, now_ms) {
        tail.push(health);
    }
    for total in quotes {
        let (text, sign) = total.signed_display();
        tail.push(fact(text, money_tone(sign), true));
    }
    if totals.unknown_orders > 0 {
        tail.push(fact(
            t!("report.unknown_quote_orders", n = totals.unknown_orders).to_string(),
            FactTone::Warn,
            true,
        ));
    }
    if unified.is_none() {
        if let Some(coverage) = mixed.then_some(totals.valuation).flatten() {
            if coverage.eligible_orders > 0 {
                tail.push(fact(
                    t!(
                        "report.valuation_progress",
                        ready = coverage.valued_orders,
                        total = coverage.eligible_orders
                    )
                    .to_string(),
                    FactTone::Warn,
                    false,
                ));
            }
            if coverage.unavailable_orders > 0 {
                tail.push(fact(
                    t!(
                        "report.valuation_unavailable",
                        n = coverage.unavailable_orders
                    )
                    .to_string(),
                    FactTone::Warn,
                    false,
                ));
            }
        }
    }
    tail.push(fact(
        t!("report.orders_count", count = totals.orders).to_string(),
        FactTone::Soft,
        false,
    ));
    tail.push(fact(
        t!("report.shown_top", n = data.rows.len()).to_string(),
        FactTone::Soft,
        false,
    ));

    FooterFacts { essential, tail }
}

/// Build the recovery text for the whole row: one line per fact, each followed by its detail.
///
/// A fact's own diagnostic is carried too, not just its text. Hovering that fact directly is
/// otherwise the only way to read it, and a fact clipped off the tail has no hover target left —
/// which would strand the stall marker's cause, codes and duration exactly when the panel is too
/// narrow to state them.
///
/// Args:
///     facts: The head and tail the row is about to render.
///
/// Returns:
///     The rendered order, essential first, joined by newlines.
pub(super) fn footer_tooltip(facts: &FooterFacts) -> String {
    facts
        .essential
        .iter()
        .chain(&facts.tail)
        .flat_map(|fact| std::iter::once(fact.text.as_str()).chain(fact.tip.as_deref()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
