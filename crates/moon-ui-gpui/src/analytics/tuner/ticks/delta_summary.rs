//! The status line's delta part: how many rows the model reads live deltas for, and — in its
//! tooltip — how well each delta's history reproduces the core, field by field
//! (`deltas::summarize`), with the deltas the model does not re-evaluate and why.

use rust_i18n::t;

use super::state::{DealRow, TapeStatus};
use moon_core::db::tuner::ticks::deltas::{self, NotComputed};

/// The caption and its tooltip, or `None` while no row has its tape.
pub(super) fn delta_summary(rows: &[DealRow]) -> Option<(String, String)> {
    let covered = rows
        .iter()
        .filter(|r| r.tape == TapeStatus::Covered)
        .count();
    if covered == 0 {
        return None;
    }
    let tracks = rows
        .iter()
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
    for missing in NotComputed::ALL {
        let reason = match missing {
            NotComputed::MarkPrice => t!("analytics.ticks.deltas_tip_mark"),
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
