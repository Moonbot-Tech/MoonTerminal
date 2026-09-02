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
use crate::workspace::scope_marker::ScopeMarker;

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
    /// Exactly the text the row renders.
    pub(super) text: String,
    /// What the tooltip states in place of [`Self::text`], when the row abbreviates.
    ///
    /// The row is one fixed-height line and spells a count as a bare number; the tooltip has the
    /// space to say what that number counts. `None` means the two are identical, which is the
    /// case for every fact that is already a whole sentence.
    pub(super) spelled: Option<String>,
    /// Colour role.
    pub(super) tone: FactTone,
    /// Whether the figure carries the row's bold weight.
    pub(super) bold: bool,
    /// Diagnostic detail shown on this fact and repeated in the shared recovery tooltip.
    pub(super) tip: Option<String>,
    /// Begin a visually separated section without putting punctuation into localized text.
    pub(super) section_start: bool,
}

/// The totals row split by what may disappear on a narrow dock.
pub(super) struct FooterFacts {
    /// Never clipped: the caption and the one money figure the row exists to state.
    pub(super) essential: Vec<FooterFact>,
    /// Clipped from the right; earlier entries survive longer.
    pub(super) tail: Vec<FooterFact>,
    /// Pinned to the row's right edge, past the clipping tail.
    ///
    /// A table statistic rather than a fact about the money: it describes what the grid above is
    /// currently showing, so it sits away from the figures instead of trailing them.
    pub(super) trailing: Vec<FooterFact>,
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
        spelled: None,
        tone,
        bold,
        tip: None,
        section_start: false,
    }
}

/// Visible and recovery representations of one traded-volume amount.
struct VolumeAmountText {
    /// SI-compacted text used by the fixed-height footer row.
    visible: String,
    /// Full quote-precision text used by the shared recovery tooltip.
    exact: String,
    /// Whether the amount is a unified active-mode USDT conversion.
    unified: bool,
    /// What the stated amount leaves out, or `None` when it is the complete filter total.
    gap: Option<VolumeGap>,
}

/// The eligible rows a stated volume amount could not account for.
///
/// Carried only by an INCOMPLETE amount, which is why its presence decides the fact's wording and
/// tone rather than adding a second fact behind it: a shortfall stated in a neighbouring fact would
/// be the FIRST thing a narrow dock clips, leaving a partial figure reading as the filter total —
/// the exact failure the fail-closed rule exists to prevent.
struct VolumeGap {
    /// Eligible closed trades absent from the stated amount.
    orders: i64,
    /// What those trades are in the row's own terms: the quote tickers that fell short — including
    /// a ticker whose own subtotal IS stated, since a bucket can be partial without being absent —
    /// plus unknown quote identity as its own entry. Never empty while a gap exists.
    currencies: String,
}

/// Format an unsigned volume amount in compact and exact native quote forms.
///
/// Args:
///     amount: Complete two-sided notional in `currency`.
///     currency: Exact persisted quote identity.
///
/// Returns:
///     SI-compacted footer text and full quote-precision tooltip text, both unsigned and followed
///     by the ticker.
fn native_volume(amount: f64, currency: db::QuoteCurrency) -> (String, String) {
    (
        format!("{} {}", fmt::compact_si(amount), currency.ticker()),
        format!(
            "{} {}",
            fmt::compact(amount, currency.display_decimals()),
            currency.ticker()
        ),
    )
}

/// Select the amount the volume footer may state, and name what that amount leaves out.
///
/// A complete active-mode USDT conversion is preferred over a MIXED scope, because it is the only
/// single scalar such a scope can be stated as; a single known currency keeps its own native money.
/// Otherwise the known quote buckets are stated explicitly and INDIVIDUALLY.
///
/// Fail-closed is a WORDING rule, not a withholding rule, and it lives HERE rather than in the
/// bucket. [`db::QuoteVolume::amount`] sums nothing but reconstructed rows, so it is always a
/// dimensionally sound subtotal — what varies is whether it is the WHOLE bucket, which
/// [`db::QuoteVolume::reconstructed`] against [`db::QuoteVolume::orders`] answers. Deciding that at
/// the bucket instead cost the user the figure entirely: a single-currency Report has no second
/// bucket to fall back on, so one liquidation out of a thousand trades blanked the footer. Every
/// bucket with at least one reconstructed trade is therefore stated, and each row it could not
/// account for is carried into the shortfall that forces the partial wording and the warn tone.
///
/// Args:
///     volume: Volume carrier from the Report snapshot, complete or not.
///
/// Returns:
///     Compact visible text, exact tooltip text, unified-conversion identity and the unaccounted
///     rows, or `None` when not one bucket reconstructed a single trade.
fn traded_volume_amount(volume: &db::TradedVolume) -> Option<VolumeAmountText> {
    // A unified conversion answers only the scope a native amount CANNOT. Under one known currency
    // that currency's own money already IS the answer, and `TradedVolume::usdt` is published for a
    // fully valued SINGLE bucket too — so without this gate a lone USDC report would swap its
    // persisted figure for a rate-derived USDT one, and under the current-rate mode take that
    // mode's wording with it. A unified figure also exists only over a fully reconstructed and
    // fully valued scope, which is why it can never carry a gap.
    if matches!(volume.scope(), db::QuoteScope::Mixed) {
        if let Some(usdt) = volume.usdt {
            let (visible, exact) = native_volume(usdt, db::QuoteCurrency::usdt());
            return Some(VolumeAmountText {
                visible,
                exact,
                unified: true,
                gap: None,
            });
        }
    }
    let mut visible = Vec::new();
    let mut exact = Vec::new();
    let mut withheld = Vec::new();
    let mut unaccounted = 0;
    for bucket in &volume.totals {
        if bucket.reconstructed > 0 {
            let (compact, full) = native_volume(bucket.amount, bucket.currency);
            visible.push(compact);
            exact.push(full);
        }
        // A bucket can be stated AND short at the same time, so the shortfall is counted per row
        // rather than per bucket — the case the previous all-or-nothing bucket could not express.
        let missing = bucket.orders.saturating_sub(bucket.reconstructed);
        if missing > 0 {
            withheld.push(bucket.currency.ticker().to_string());
            unaccounted += missing;
        }
    }
    // Rows whose quote identity is unknown are part of the shortfall too, and they have no ticker to
    // name themselves with.
    if volume.unknown_orders > 0 {
        withheld.push(t!("report.traded_volume_unknown_quote").to_string());
        unaccounted += volume.unknown_orders;
    }
    // Nothing provable at all — an empty filter, or a scope where not one trade reconstructed —
    // states no volume rather than an amount-less warning: the row already carries the unknown-quote
    // tally, and a fact with no figure in it would only occupy the slot the figure belongs in.
    if visible.is_empty() {
        return None;
    }
    Some(VolumeAmountText {
        visible: visible.join(" + "),
        exact: exact.join(" + "),
        unified: false,
        gap: (unaccounted > 0).then(|| VolumeGap {
            orders: unaccounted,
            currencies: withheld.join(", "),
        }),
    })
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

/// State the still-running positions as ONE fact, or nothing when none are open.
///
/// Count and money share a single fact deliberately, the way the traded-volume shortfall shares
/// one with its figure: a dock narrow enough to clip half of this pair would leave either a count
/// of positions with no money or an amount with nothing saying what it belongs to, and the second
/// reads exactly like realized profit.
///
/// The tone is ALWAYS [`FactTone::Soft`], never a sign colour. In this row the sign colours belong
/// to settled money; an unrealized figure wearing green is how it gets read as earned.
///
/// Args:
///     open: Unrealized tally computed over the same filter and snapshot as the totals.
///
/// Returns:
///     The assembled fact, or `None` when nothing is open.
fn open_positions_fact(open: &db::OpenPositions) -> Option<FooterFact> {
    if open.orders <= 0 {
        return None;
    }
    // Per known currency, mirroring the quote breakdown above rather than inventing a unified
    // figure: converting floating money would state a second estimate on top of an estimate.
    let amount = open
        .totals
        .iter()
        .map(|total| total.signed_display().0)
        .collect::<Vec<_>>()
        .join(" + ");
    // With every open row's currency unknown there is no amount to show, so the count stands
    // alone rather than the fact disappearing — positions ARE running and the row must say so.
    let text = if amount.is_empty() {
        t!("report.open_positions_bare", n = open.orders).to_string()
    } else {
        t!("report.open_positions", n = open.orders, amount = amount).to_string()
    };
    let mut open_fact = fact(text, FactTone::Soft, false);
    open_fact.spelled = Some(if open.unknown_orders > 0 {
        t!(
            "report.open_positions_tip_unknown",
            n = open.orders,
            unknown = open.unknown_orders
        )
        .to_string()
    } else {
        t!("report.open_positions_tip", n = open.orders).to_string()
    });
    Some(open_fact)
}

/// Assemble every fact the totals row states, split by whether it may be clipped.
///
/// The tail order is the priority order and is deliberate. A valuation stall leads it because it
/// warns that a number already on screen may be wrong, and that outranks any tally. The remaining
/// quote totals come next because a missing currency total silently changes what the row appears to
/// sum, whereas a missing row count only withholds a tally the table itself shows. Everything ahead
/// of the traded volume QUALIFIES the realized figure in the never-clipped head, so losing one of
/// them changes what that head appears to mean. The open-positions fact closes the tail because it
/// is the one entry that names itself completely — it can never be misread as part of the head, and
/// the grid above already shows those rows — so it is the cheapest thing to lose to a narrow dock.
/// The shown-rows count is not in the tail at all; it is pinned right, and the ORDER count rides in
/// the never-clipped caption.
///
/// Args:
///     data: Current report snapshot, or `None` while none is renderable.
///     failed: Whether the absent snapshot is a failed read rather than a pending one.
///     status: Health published by the valuation worker.
///     now_ms: Current wall-clock time in Unix milliseconds.
///     marker: Workspace scope marker for this group, or `None` for a standalone report.
///
/// Returns:
///     The essential head and the clip-ordered tail.
pub(super) fn footer_facts(
    data: Option<&ReportData>,
    failed: bool,
    status: &ValuationStatus,
    now_ms: i64,
    marker: Option<&ScopeMarker>,
) -> FooterFacts {
    // The caption carries the order count, so the tally the row is a total OF can never be the
    // thing a narrow dock clips away. Without a snapshot there is no count to state.
    let caption = match data {
        Some(data) => {
            let orders = data.totals.orders;
            FooterFact {
                // The row has one line and states the count as a bare number; the tooltip says
                // what it counts.
                spelled: Some(t!("report.totals_n_tip", n = orders).to_string()),
                ..fact(
                    t!("report.totals_n", n = orders).to_string(),
                    FactTone::Soft,
                    false,
                )
            }
        }
        None => fact(t!("report.totals").to_string(), FactTone::Soft, false),
    };
    let mut essential = vec![caption];
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
        return FooterFacts {
            essential,
            tail,
            trailing: Vec::new(),
        };
    };

    let totals = &data.totals;
    // Read from the snapshot, never from the live setting: this row must label the numbers it is
    // actually showing, and those may predate a mode change whose requery has not landed yet.
    let mode = data.valuation;
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
                // A current-rate figure is not historical P&L, so it never borrows that sentence.
                t!(
                    mode.key("report.valuation_total", "report.valuation_total_current"),
                    amount = amount
                )
                .to_string(),
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
    // The exact per-currency totals read as ONE parenthesised breakdown of the figure above —
    // "+500 USDT (+600 USDT -100 USDC)". They stay separate facts so each keeps its own sign
    // colour; only the outer brackets are glued to the first and last of them.
    let breakdown: Vec<_> = quotes.map(|total| total.signed_display()).collect();
    let last = breakdown.len().saturating_sub(1);
    for (i, (text, sign)) in breakdown.into_iter().enumerate() {
        let mut text = text;
        if i == 0 {
            text.insert(0, '(');
        }
        if i == last {
            text.push(')');
        }
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
                        mode.key(
                            "report.valuation_progress",
                            "report.valuation_current_progress"
                        ),
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
                        mode.key(
                            "report.valuation_unavailable",
                            "report.valuation_current_unavailable"
                        ),
                        n = coverage.unavailable_orders
                    )
                    .to_string(),
                    FactTone::Warn,
                    false,
                ));
            }
        }
    }
    if let Some(amount) = traded_volume_amount(&totals.traded_volume) {
        let VolumeAmountText {
            visible,
            exact,
            unified,
            gap,
        } = amount;
        // A unified conversion is the only amount the current-rate wording can apply to, and it is
        // also the only one that is never partial — so the two labels below cannot collide.
        let current = unified && mode == db::valuation::ValuationMode::Current;
        let mut volume = match gap {
            // The shortfall shares ONE fact with the figure it qualifies, so no dock width can show
            // the number without it, and the warn tone repeats the same signal in colour.
            Some(gap) => {
                let mut partial = fact(
                    t!("report.traded_volume_partial", amount = visible).to_string(),
                    FactTone::Warn,
                    false,
                );
                partial.spelled = Some(
                    t!(
                        "report.traded_volume_partial_tip",
                        amount = exact,
                        n = gap.orders,
                        currencies = gap.currencies
                    )
                    .to_string(),
                );
                partial
            }
            None => {
                let (label, tip) = if current {
                    (
                        "report.traded_volume_current",
                        "report.traded_volume_current_tip",
                    )
                } else {
                    ("report.traded_volume", "report.traded_volume_tip")
                };
                let mut complete = fact(
                    t!(label, amount = visible).to_string(),
                    FactTone::Soft,
                    false,
                );
                complete.spelled = Some(t!(tip, amount = exact).to_string());
                complete
            }
        };
        volume.section_start = true;
        tail.push(volume);
    }
    if let Some(mut open) = open_positions_fact(&data.open) {
        open.section_start = true;
        tail.push(open);
    }
    // The scope marker's own facts, appended last — never a second copy, and never rendered when
    // the active preset hides nothing (decision 1). The closing hint line lives in the tooltip
    // only (`render.rs`), never on the row itself.
    if let Some(marker) = marker {
        let mut facts = marker.facts().into_iter();
        if let Some(first) = facts.next() {
            let mut first = fact(first, FactTone::Soft, false);
            first.section_start = true;
            tail.push(first);
            tail.extend(facts.map(|text| fact(text, FactTone::Soft, false)));
        }
    }
    let trailing = vec![fact(
        t!("report.shown_top", n = data.rows.len()).to_string(),
        FactTone::Soft,
        false,
    )];

    FooterFacts {
        essential,
        tail,
        trailing,
    }
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
        .chain(&facts.trailing)
        .flat_map(|fact| {
            std::iter::once(fact.spelled.as_deref().unwrap_or(&fact.text))
                .chain(fact.tip.as_deref())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
