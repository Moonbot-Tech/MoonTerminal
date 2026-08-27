//! Pure assembly of one Versions-pane row: which facts it states, which of them may be
//! clipped and in what order, and the tooltip that keeps every clipped one reachable.
//!
//! Kept free of `cx`, `MoonPalette`, `moon(`, `moon_alpha(` and `Context<` so the clip priority
//! and the "nothing-clipped-is-unreachable" property are assertable without a GPUI app context —
//! the same reasoning as `panels/report/totals.rs`. `MoonTone` and `design::delta_tone` are the
//! one narrow exception: both are palette-free, so naming a colour ROLE here does not require
//! resolving one. `versions.rs` alone turns a `MoonTone` into an actual colour.

use moon_core::strat_db::stats::VersionInfo;
use moon_core::util::display_time;
use moon_core::util::fmt;
use moon_ui::MoonTone;
use rust_i18n::t;

use crate::design::delta_tone;

#[cfg(test)]
mod tests;

/// One rendered version fact, already localized.
pub(super) struct VersionFact {
    pub(super) text: String,
    /// What the tooltip states in place of `text` when the row abbreviates. `None` = identical.
    pub(super) spelled: Option<String>,
    pub(super) tone: MoonTone,
    /// Render as a `MoonBadge` rather than bare text.
    pub(super) badge: bool,
}

/// Where a version sits relative to the live strategy, derived from `valid_to`, NEVER from the
/// row index: the list order is a query contract, the openness is a data fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VersionSlot {
    InEffect,
    Previous,
    Older,
}

/// One version row split by what a 90 px pane may drop.
pub(super) struct VersionRowFacts {
    /// Never clipped: the instant that identifies the row, and the money. One inner `Vec` per
    /// rendered LINE — a narrow pane may need the stamp and the money on separate lines, and even
    /// the money's own wording degrades before a second line is used. See `version_row_facts`.
    pub(super) head: Vec<Vec<VersionFact>>,
    /// Line(s) after the head, clipped from the right; earlier entries survive longer.
    pub(super) tail: Vec<VersionFact>,
}

/// Build the full localized stamp for the tooltip, `YYYY-MM-DD HH:MM:SS` in the display zone.
///
/// `valid_from` is Unix MILLISECONDS; `display_time::format_second` takes seconds and would
/// silently render empty for a contemporary millisecond value passed to it directly, so this goes
/// through the millisecond seam (`at_millis`) instead. Never hand-divide by 1000 here.
///
/// Args:
///     v: Persisted version whose creation instant is displayed.
///     zone: User-selected display timezone.
///     now_ms: Current Unix time in milliseconds for relative clock formatting.
///     with_seconds: Whether a same-minute collision requires seconds in the row text.
///     bare_stamp: Precomputed seconds-less stamp from the caller's collision scan.
///
/// Returns:
///     A muted timestamp fact with full tooltip text in the display timezone.
fn stamp_fact(
    v: &VersionInfo,
    zone: chrono_tz::Tz,
    now_ms: i64,
    with_seconds: bool,
    bare_stamp: &str,
) -> VersionFact {
    // The caller already formatted the seconds-less stamp for its whole-list collision scan, and
    // this runs per ROW per FRAME: reuse it rather than paying `format_chart_clock`'s two timezone
    // conversions a second time. Only a colliding row needs the seconds-bearing variant.
    let text = if with_seconds {
        display_time::format_chart_clock(v.valid_from, zone, true, now_ms)
    } else {
        bare_stamp.to_string()
    };
    let spelled = display_time::at_millis(v.valid_from, zone)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .map(|full| t!("strat.version_stamp_tip", stamp = full).to_string());
    VersionFact {
        text,
        spelled,
        tone: MoonTone::Muted,
        badge: false,
    }
}

/// Which wording `money_fact` renders, widest first. Each degrade keeps the row honest: never a
/// clipped figure, only a shorter but still-whole one, with the full sentence always reachable
/// through `spelled`.
#[derive(Clone, Copy)]
enum MoneyForm {
    /// The full sentence: amount, trade count, open-trades caveat.
    Full,
    /// The bare signed amount plus `$`, no trade count — used when the full sentence does not fit
    /// beside the stamp on one line.
    Compact,
    /// An SI-abbreviated signed amount (`compact_si`) — used when even the bare figure still
    /// exceeds the pane's character budget. Still a whole, honest number, never a clipped one.
    Si,
}

/// Build the money fact, defect 2 — and why it is ONE fact and never three separate widgets.
///
/// The trade count and the open-trades caveat are never dropped outright: whenever `form` is not
/// `Full`, they survive in `spelled`, so the row's recovery tooltip always states them.
///
/// A non-finite `profit` (from which `fmt::signed_fixed` returns `None`) is treated exactly like
/// the no-trades case: stating nothing is honest where stating `NaN$` is not.
///
/// Args:
///     v: Persisted version supplying trade and profit statistics.
///     form: The widest honest wording that the current row budget can hold.
///
/// Returns:
///     One money fact whose tooltip preserves any detail omitted from its row text.
fn money_fact(v: &VersionInfo, form: MoneyForm) -> VersionFact {
    // Both "we cannot state a figure" cases render the same way, and they must not drift apart:
    // no trades at all, and a non-finite profit (which is exactly when `signed_fixed` yields
    // `None`). Saying nothing is honest where `NaN$` is not.
    let no_figure = || VersionFact {
        text: t!("strat.version_no_trades").to_string(),
        spelled: Some(t!("strat.version_profit_tip").to_string()),
        tone: MoonTone::Muted,
        badge: false,
    };
    if v.trades == 0 {
        return no_figure();
    }
    let Some((amount, sign)) = fmt::signed_fixed(v.profit, 2) else {
        return no_figure();
    };
    let full = if v.open_left > 0 {
        t!(
            "strat.version_profit_partial",
            amount = amount.as_str(),
            n = v.trades,
            open = v.open_left
        )
        .to_string()
    } else {
        t!(
            "strat.version_profit",
            amount = amount.as_str(),
            n = v.trades
        )
        .to_string()
    };
    // The shortfall shares the fact with the figure regardless of which wording is chosen: no
    // pane width can show the number without its caveat.
    let tone = if v.open_left > 0 {
        MoonTone::Warning
    } else {
        delta_tone(sign)
    };
    match form {
        MoneyForm::Full => VersionFact {
            text: full,
            spelled: Some(t!("strat.version_profit_tip").to_string()),
            tone,
            badge: false,
        },
        MoneyForm::Compact => VersionFact {
            text: format!("{amount}$"),
            spelled: Some(full),
            tone,
            badge: false,
        },
        MoneyForm::Si => {
            // `amount` already carries the correct sign prefix from `signed_fixed` ("+"/"-", or
            // none for an exact zero); `compact_si` itself is unsigned, so the prefix is lifted
            // off `amount` rather than re-derived from `sign` to avoid a second, divergent rule.
            let sign_prefix = if amount.starts_with('+') {
                "+"
            } else if amount.starts_with('-') {
                "-"
            } else {
                ""
            };
            let si = fmt::compact_si(v.profit.abs());
            VersionFact {
                text: format!("{sign_prefix}{si}$"),
                spelled: Some(full),
                tone,
                badge: false,
            }
        }
    }
}

/// `"restored"` -> rollback, `"created"` -> created, `"params"` and anything else -> nothing: a
/// `None` default arm keeps an unrecognized future token from leaking into the pane untranslated.
///
/// Args:
///     change_kind: Persisted machine-readable kind token.
///
/// Returns:
///     A localized kind fact for recognized non-parameter changes, otherwise `None`.
fn kind_fact(change_kind: &str) -> Option<VersionFact> {
    match change_kind {
        "restored" => Some(VersionFact {
            text: t!("strat.version_kind_restored").to_string(),
            spelled: Some(t!("strat.version_kind_restored_tip").to_string()),
            tone: MoonTone::Muted,
            badge: false,
        }),
        "created" => Some(VersionFact {
            text: t!("strat.version_kind_created").to_string(),
            spelled: Some(t!("strat.version_kind_created_tip").to_string()),
            tone: MoonTone::Muted,
            badge: false,
        }),
        _ => None,
    }
}

/// `Some("local")` -> edited here, `Some("external")` -> external, `None` -> first snapshot, any
/// other token -> nothing (same `None`-default reasoning as `kind_fact`).
///
/// Args:
///     origin: Optional persisted machine-readable source token.
///
/// Returns:
///     A localized origin fact for recognized values, otherwise `None`.
fn origin_fact(origin: Option<&str>) -> Option<VersionFact> {
    match origin {
        Some("local") => Some(VersionFact {
            text: t!("strat.version_origin_local").to_string(),
            spelled: Some(t!("strat.version_origin_local_tip").to_string()),
            tone: MoonTone::Muted,
            badge: false,
        }),
        Some("external") => Some(VersionFact {
            text: t!("strat.version_origin_external").to_string(),
            spelled: Some(t!("strat.version_origin_external_tip").to_string()),
            tone: MoonTone::Muted,
            badge: false,
        }),
        None => Some(VersionFact {
            text: t!("strat.version_origin_initial").to_string(),
            spelled: Some(t!("strat.version_origin_initial_tip").to_string()),
            tone: MoonTone::Muted,
            badge: false,
        }),
        Some(_) => None,
    }
}

/// Localized relative age, or `None` for a future/invalid stamp.
///
/// Works entirely in the millisecond domain (thresholds are minute/hour/day millisecond counts)
/// rather than converting through seconds, so this stays clear of the plain `/ 1000` this module
/// avoids everywhere else.
///
/// Args:
///     valid_from_ms: Version creation time in Unix milliseconds.
///     now_ms: Current Unix time in milliseconds.
///
/// Returns:
///     Localized elapsed age, or `None` when the stamp is future-dated or invalid.
pub(super) fn relative_age(valid_from_ms: i64, now_ms: i64) -> Option<String> {
    let diff_ms = now_ms - valid_from_ms;
    if diff_ms < 0 {
        return None;
    }
    const MINUTE_MS: i64 = 60_000;
    const HOUR_MS: i64 = 3_600_000;
    const DAY_MS: i64 = 86_400_000;
    if diff_ms < MINUTE_MS {
        Some(t!("strat.version_age_now").to_string())
    } else if diff_ms < HOUR_MS {
        Some(t!("strat.version_age_m", n = diff_ms / MINUTE_MS).to_string())
    } else if diff_ms < DAY_MS {
        Some(t!("strat.version_age_h", n = diff_ms / HOUR_MS).to_string())
    } else {
        Some(t!("strat.version_age_d", n = diff_ms / DAY_MS).to_string())
    }
}

/// Assemble one row: the never-clipped head (identity + money, one or two lines) and the
/// clip-ordered tail (slot badge, changed count, kind, origin, age — in that descending priority).
///
/// `budget_chars` is how many monospace characters one line of the pane can hold; the caller
/// derives it from the pane width and a single glyph's measured width, keeping this function pure
/// and its narrow-pane behaviour unit-testable without a GPUI context.
///
/// Args:
///     v: Persisted version supplying row facts.
///     slot: Version's position relative to the live strategy.
///     zone: User-selected display timezone.
///     now_ms: Current Unix time in milliseconds.
///     with_seconds: Whether the stamp collides with another rendered minute.
///     budget_chars: Maximum monospace characters per rendered row line.
///     bare_stamp: Precomputed seconds-less stamp from the whole-list collision scan.
///
/// Returns:
///     Never-clipped head lines and tail facts in their clip-priority order.
pub(super) fn version_row_facts(
    v: &VersionInfo,
    slot: VersionSlot,
    zone: chrono_tz::Tz,
    now_ms: i64,
    with_seconds: bool,
    budget_chars: usize,
    bare_stamp: &str,
) -> VersionRowFacts {
    let stamp = stamp_fact(v, zone, now_ms, with_seconds, bare_stamp);
    let form_full = money_fact(v, MoneyForm::Full);

    // The money fact degrades by TEXT before the row degrades by LINE: pick the widest form that
    // fits beside the stamp, then fall back to a second line only when even the compact form does
    // not fit alongside it. On its own line, the compact form itself may still overflow a very
    // narrow pane, so a THIRD, SI-abbreviated form is the final fallback — never clipped, since a
    // half-rendered monospaced figure reads as a plausible WRONG number.
    const GAP_CHARS: usize = 1;
    let stamp_len = stamp.text.chars().count();
    let fits_beside =
        |f: &VersionFact| stamp_len + GAP_CHARS + f.text.chars().count() <= budget_chars;
    let fits_alone = |f: &VersionFact| f.text.chars().count() <= budget_chars;
    let head = if fits_beside(&form_full) {
        vec![vec![stamp, form_full]]
    } else {
        // Only here is a narrower wording worth building. This runs per ROW per FRAME, so on any
        // pane wide enough for the full sentence -- the common case -- the extra `money_fact`
        // (an i18n lookup plus two allocations) never happens at all.
        let form_compact = money_fact(v, MoneyForm::Compact);
        if fits_beside(&form_compact) {
            vec![vec![stamp, form_compact]]
        } else if fits_alone(&form_full) {
            vec![vec![stamp], vec![form_full]]
        } else if fits_alone(&form_compact) {
            vec![vec![stamp], vec![form_compact]]
        } else {
            vec![vec![stamp], vec![money_fact(v, MoneyForm::Si)]]
        }
    };

    let mut tail = Vec::new();
    match slot {
        VersionSlot::InEffect => tail.push(VersionFact {
            text: t!("strat.version_open").to_string(),
            spelled: Some(t!("strat.version_open_tip").to_string()),
            tone: MoonTone::Accent,
            badge: true,
        }),
        VersionSlot::Previous => tail.push(VersionFact {
            text: t!("strat.version_previous").to_string(),
            spelled: Some(t!("strat.version_previous_tip").to_string()),
            tone: MoonTone::Muted,
            badge: true,
        }),
        VersionSlot::Older => {}
    }
    if v.n_changed > 0 {
        tail.push(VersionFact {
            text: t!("strat.version_changed_n", n = v.n_changed).to_string(),
            spelled: Some(t!("strat.version_changed_tip", n = v.n_changed).to_string()),
            tone: MoonTone::Muted,
            badge: true,
        });
    }
    if let Some(f) = kind_fact(&v.change_kind) {
        tail.push(f);
    }
    if let Some(f) = origin_fact(v.origin.as_deref()) {
        tail.push(f);
    }
    if let Some(age) = relative_age(v.valid_from, now_ms) {
        tail.push(VersionFact {
            text: age,
            spelled: None,
            tone: MoonTone::Muted,
            badge: false,
        });
    }

    VersionRowFacts { head, tail }
}

/// Build the row's recovery text: one line per fact, head first (line by line), then tail.
///
/// Args:
///     facts: Rendered row facts, including abbreviated text and tooltip replacements.
///
/// Returns:
///     Newline-separated full text for the row tooltip.
pub(super) fn row_tooltip(facts: &VersionRowFacts) -> String {
    facts
        .head
        .iter()
        .flatten()
        .chain(facts.tail.iter())
        .map(|f| f.spelled.as_deref().unwrap_or(f.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}
