//! Bounded text for an Adjusted strategy edit.
//!
//! moon-core passes the field names and the formatted values. This module wraps them in the
//! sentence the locale owns, and stops the sentence after a few fields so a strategy that differs
//! in dozens of places does not paint a paragraph into the banner.

use moon_core::feed::{STRATEGY_ADJUSTMENT_PREVIEW, StrategyFieldChange};

#[cfg(test)]
mod tests;

/// Suffix for `strat.edit_adjusted` / `shell.strat_edit_adjusted`.
///
/// Args:
///     changes: Differences the core reported, in report order. Empty when the Adjusted note
///         arrived without a field list.
///
/// Returns:
///     `""` when `changes` is empty, so the locale sentence stays the short form. Otherwise
///     `": name (sent → saved), …"` for the first [`STRATEGY_ADJUSTMENT_PREVIEW`] fields, then
///     ` +N` for however many remain.
pub(crate) fn adjusted_diff_suffix(changes: &[StrategyFieldChange]) -> String {
    if changes.is_empty() {
        return String::new();
    }
    let shown = changes.len().min(STRATEGY_ADJUSTMENT_PREVIEW);
    let mut text = changes
        .iter()
        .take(shown)
        .map(|change| format!("{} ({} → {})", change.name, change.sent, change.saved))
        .collect::<Vec<_>>()
        .join(", ");
    let rest = changes.len() - shown;
    if rest > 0 {
        text.push_str(&format!(" +{rest}"));
    }
    format!(": {text}")
}
