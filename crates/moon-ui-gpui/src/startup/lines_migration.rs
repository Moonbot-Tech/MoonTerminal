//! Carry-over of the four switches that moved into `ChartGraphicsCfg`: the three price-line and
//! corridor flags out of `candle_view`, and the liquidations switch out of the tab spec's own
//! per-tab field.
//!
//! They moved so that every setting the "Chart graphics" popup shows travels with that popup's ⧉
//! press, whose model is "one slot, one struct". Nothing about the move is automatic for an
//! existing profile: the old keys are still read (`CandleViewCfg::carried_lines`,
//! `ChartTabSpec::liquidations_enabled`) but no longer drawn from, so without this pass every user
//! who turned a line, the corridor or the liquidation crosses off would watch them come back.
//!
//! The defaults in `layout.toml` are carried by `WindowLayout::carry_lines_into_graphics`; this
//! module does the tab specs, AFTER that, because a tab with no graphics of its own opens with its
//! kind's default and that default must already say what the kind drew.
//!
//! What a tab drew: its own `candle_view` switches, else its kind's — and the kind's have to be
//! read BEFORE the layout pass consumes them ([`KindCarried::snapshot`]), because a spec holding
//! graphics of its own but no candles of its own drew the kind's switches and must be stamped with
//! them. A spec with inherited graphics gets graphics of its own — the kind's migrated copy with
//! what it drew on top — only where the two differ; one following both chains needs nothing,
//! since its kind's default now carries them.
//!
//! Candles of a tab's own that carry nothing are taken for a value written AFTER the move, and
//! such a spec is left alone. `charts.json` is machine-written, and every build since the
//! price-line split wrote every field of the table, so an old spec with candles of its own names
//! all three (one from before the split, carrying no line key at all, follows its kind's graphics
//! instead of the shipped defaults it drew — a build that old is not carried for); the layout
//! pass, whose file IS hand-edited, treats an unnamed switch in an old profile as the shipped
//! default instead — see `WindowLayout::carried_lines_for`. Reading the kind's carrier for such
//! a spec would be wrong twice over: a tab's own candles replaced the kind's whole, so the kind's
//! switches were never what it drew, and a launch that re-reads a layout whose save failed would
//! stamp them over a tab the previous launch already carried correctly.
//!
//! A tab whose own switches equalled its kind's gets no graphics of its own and follows the kind
//! from now on. Deliberate: the alternative freezes every such tab on a copy of a default it never
//! chose, which is the one-way ticket the ⧉ press was redesigned to stop handing out.
//!
//! No marker, for the reason the layout pass has none: both sources are consumed, and written
//! back only while carried, so the save that follows ends the carry-over for good.

use moon_core::config::ChartTabKind;
use moon_core::config::layout::{WindowLayout, stamp_carried_lines};
use moon_core::market::candles::CarriedLines;

use crate::chart_tabs::apply_all::spec_kind;
use crate::persistence::chart_persist::ChartTabSpec;

/// What each kind's `candle_view` carried before the layout pass consumed it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct KindCarried(Vec<(ChartTabKind, CarriedLines)>);

impl KindCarried {
    /// Read every kind's carried switches off the layout as loaded. Call BEFORE
    /// `WindowLayout::carry_lines_into_graphics`, which takes them.
    pub(super) fn snapshot(layout: &WindowLayout) -> Self {
        Self(
            ChartTabKind::ALL
                .iter()
                .filter_map(|kind| layout.carried_lines_for(*kind).map(|c| (*kind, c)))
                .collect(),
        )
    }

    fn get(&self, kind: ChartTabKind) -> Option<CarriedLines> {
        self.0.iter().find(|(k, _)| *k == kind).map(|(_, c)| *c)
    }
}

/// Carry one spec's old switches into its graphics; see the module doc for the cases.
///
/// Args:
///     layout: The ALREADY migrated window layout, whose kind defaults now carry the switches.
///     kinds: What each kind carried before that.
///     spec: The tab spec to rewrite in place.
///
/// Returns:
///     Whether the spec changed.
pub(super) fn carry_spec(
    layout: &WindowLayout,
    kinds: &KindCarried,
    spec: &mut ChartTabSpec,
) -> bool {
    let mut changed = false;
    let kind = spec_kind(spec);
    let own_lines = spec.candle_view.and_then(|cv| cv.carried_lines);
    let liquidations = spec.liquidations_enabled;
    // The lines this tab drew: its own candles' carrier, every switch it did not name read as
    // the shipped default since its table replaced the kind's whole — nothing at all, for
    // candles written after the move (see the module doc) — or, with candles inherited, the
    // kind's old switches.
    let drawn = match spec.candle_view {
        Some(_) => own_lines.map(CarriedLines::or_shipped),
        None => kinds.get(kind),
    };
    match spec.chart_graphics.as_mut() {
        Some(cfg) => {
            if let Some(lines) = drawn {
                changed |= stamp_carried_lines(cfg, lines);
            }
            if let Some(v) = liquidations
                && cfg.liquidations != v
            {
                cfg.liquidations = v;
                changed = true;
            }
        }
        None => {
            // Inherited graphics carry the kind's switches already: only where what this tab
            // drew differs from them does it need graphics of its own.
            let mut cfg = layout.chart_graphics_for(kind);
            let mut differs = false;
            if let Some(lines) = drawn {
                differs |= stamp_carried_lines(&mut cfg, lines);
            }
            if let Some(v) = liquidations
                && cfg.liquidations != v
            {
                cfg.liquidations = v;
                differs = true;
            }
            if differs {
                spec.chart_graphics = Some(cfg);
                changed = true;
            }
        }
    }
    // Consumed either way: the spec stops carrying them, so the next save drops the old keys.
    if let Some(cv) = spec.candle_view.as_mut()
        && cv.carried_lines.take().is_some()
    {
        changed = true;
    }
    if spec.liquidations_enabled.take().is_some() {
        changed = true;
    }
    changed
}

/// Carry the old switches of every tab spec into its graphics.
///
/// Args:
///     layout: The ALREADY migrated window layout.
///     kinds: What each kind carried before that, from [`KindCarried::snapshot`].
///     specs: Every loaded chart tab spec, rewritten in place.
///
/// Returns:
///     Whether any spec changed and `charts.json` therefore has to be saved.
pub(super) fn carry_specs(
    layout: &WindowLayout,
    kinds: &KindCarried,
    specs: &mut [ChartTabSpec],
) -> bool {
    let mut changed = false;
    for spec in specs.iter_mut() {
        changed |= carry_spec(layout, kinds, spec);
    }
    changed
}

#[cfg(test)]
mod tests;
