//! The status line's delta part: how many rows the model reads live deltas for, and — in its
//! tooltip — how well each delta's history reproduces the core, field by field
//! (`deltas::summarize`), how many read the mark delta off the recorded market list, and the
//! deltas the model does not re-evaluate and why.

use rust_i18n::t;

use super::state::{DealRow, TapeStatus};
use moon_core::db::tuner::ticks::deltas::{self, NotComputed};

/// The formatted summary belongs to both a row publication and an interface language.
pub(super) struct Cache {
    rows_rev: u64,
    locale: String,
    text: Option<(String, String)>,
}

/// Reuse the complete caption and tooltip without scanning tracks or sorting medians per paint.
pub(super) fn cached(state: &mut super::state::TicksState) -> Option<(String, String)> {
    let locale = rust_i18n::locale();
    if !state
        .delta_summary
        .as_ref()
        .is_some_and(|cache| cache.rows_rev == state.rows_rev && cache.locale == *locale)
    {
        state.delta_summary = Some(Cache {
            rows_rev: state.rows_rev,
            locale: locale.to_string(),
            text: state.data.data().and_then(|data| delta_summary(&data.rows)),
        });
    }
    state
        .delta_summary
        .as_ref()
        .and_then(|cache| cache.text.clone())
}

/// The caption and its tooltip, or `None` while no row has its tape.
pub(super) fn delta_summary<'a>(
    rows: impl IntoIterator<Item = &'a DealRow> + Clone,
) -> Option<(String, String)> {
    let covered = rows
        .clone()
        .into_iter()
        .filter(|r| r.tape == TapeStatus::Covered)
        .count();
    if covered == 0 {
        return None;
    }
    // Rows whose mark delta the recorded market list gives (`ticks::quotes`).
    let marked = rows
        .clone()
        .into_iter()
        .filter(|r| r.tape == TapeStatus::Covered)
        .filter(|r| r.deal.quotes.as_ref().is_some_and(|q| q.mark_live()))
        .count();
    let tracks = rows
        .into_iter()
        .filter(|r| r.tape == TapeStatus::Covered)
        .filter_map(|r| r.deal.delta_track.as_deref());
    let quality = deltas::summarize(tracks);
    let caption = t!(
        "analytics.ticks.deltas_caption",
        tracked = quality.tracks,
        covered = covered
    )
    .to_string();
    let mut tip = vec![t!("analytics.ticks.deltas_tip_head").to_string()];
    for field in &quality.fields {
        tip.push(field_line(field));
    }
    tip.push(format!(
        "dmark: {}",
        t!("analytics.ticks.deltas_tip_mark", n = marked)
    ));
    for missing in NotComputed::ALL {
        let reason = match missing {
            NotComputed::PriceBug => t!("analytics.ticks.deltas_tip_pricebug"),
            NotComputed::Market => t!("analytics.ticks.deltas_tip_market"),
        };
        tip.push(format!("{}: {reason}", missing.columns()));
    }
    Some((caption, tip.join("\n")))
}

/// One field's line of the tooltip.
fn field_line(field: &deltas::FieldQuality) -> String {
    let name = field.field.column();
    if field.live == 0 {
        return t!("analytics.ticks.deltas_tip_none", name = name).to_string();
    }
    let coverage = field
        .coverage_median
        .map_or_else(|| "—".to_string(), |c| format!("{:.0}", c * 100.0));
    let error = field.error_median.map_or_else(
        || "—".to_string(),
        |e| {
            let (text, _) = super::rows::paint_fixed(e, 3);
            text
        },
    );
    let mut line = t!(
        "analytics.ticks.deltas_tip_field",
        name = name,
        live = field.live,
        coverage = coverage,
        reproduced = field.reproduced,
        checked = field.checked,
        error = error
    )
    .to_string();
    if field.field.is_btc() {
        line.push_str(&t!("analytics.ticks.deltas_tip_btc_note"));
    }
    line
}

#[cfg(test)]
mod tests;
