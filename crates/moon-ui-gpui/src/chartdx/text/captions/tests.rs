//! The line rule: which captions share a line, and which share a column inside it.
//!
//! Explicit imports: the chartdx parent re-exports `gpui::*`, whose own `test` shadows the built-in
//! attribute and makes `#[test]` expand recursively.

use moon_core::config::{
    ChartLabelField, ChartLabelRow, ChartLabelsCfg, LabelAlign, LabelFlow, LabelZone,
};

use super::super::labels::LabelText;
use super::group_lines;

/// The shape that prompted the two axes: a scale badge, then a two-caption delta module, both in
/// the plot's top band and pushed right.
fn cfg(deltas_flow: LabelFlow, deltas_placement: LabelFlow) -> ChartLabelsCfg {
    let mut cfg = ChartLabelsCfg::empty();
    let mut badge = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
    badge.push_part(ChartLabelField::ScaleBadge);
    let mut deltas = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
    deltas.push_part(ChartLabelField::Delta1h);
    deltas.push_part(ChartLabelField::Delta24h);
    deltas.flow = deltas_flow;
    deltas.placement = deltas_placement;
    cfg.rows[0] = badge;
    cfg.rows[1] = deltas;
    cfg
}

/// Captions as the text pass emits them: module by module, caption by caption.
fn texts() -> Vec<LabelText> {
    ["29%", "Δ1ч", "Δ24ч"]
        .into_iter()
        .enumerate()
        .map(|(n, text)| LabelText {
            row: usize::from(n > 0),
            part: n.saturating_sub(1),
            text: text.into(),
            prefix: String::new(),
            reachable: false,
            venue: None,
            sign: None,
            color: None,
        })
        .collect()
}

fn lines(flow: LabelFlow, placement: LabelFlow) -> Vec<Vec<Vec<usize>>> {
    group_lines(
        &cfg(flow, placement),
        &texts(),
        LabelZone::ChartTop,
        LabelAlign::Right,
    )
}

/// The default: the badge on its line, both deltas beside each other on the next.
#[test]
fn a_row_module_gives_each_caption_its_own_column() {
    assert_eq!(
        lines(LabelFlow::Row, LabelFlow::Column),
        vec![vec![vec![0]], vec![vec![1], vec![2]]]
    );
}

/// Placement alone decides the line: a row module continues the one above.
#[test]
fn a_row_placed_module_continues_the_line_above() {
    assert_eq!(
        lines(LabelFlow::Row, LabelFlow::Row),
        vec![vec![vec![0], vec![1], vec![2]]]
    );
}

/// A column module keeps its captions in ONE column.
#[test]
fn a_column_module_keeps_its_captions_in_one_column() {
    assert_eq!(
        lines(LabelFlow::Column, LabelFlow::Column),
        vec![vec![vec![0]], vec![vec![1, 2]]]
    );
}

/// THE case the two axes exist for: a module that runs down a column, standing as a BLOCK beside
/// the module above it — `29%  Δ1ч` / `      Δ24ч`.
#[test]
fn a_column_module_can_stand_beside_the_one_above_it() {
    assert_eq!(
        lines(LabelFlow::Column, LabelFlow::Row),
        vec![vec![vec![0], vec![1, 2]]],
        "one line, two columns: the badge and the delta block"
    );
}

/// A band that opens with a row-placed module still starts a line — there is nothing to continue.
#[test]
fn the_first_module_of_a_band_always_opens_a_line() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut deltas = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
    deltas.push_part(ChartLabelField::Delta1h);
    deltas.placement = LabelFlow::Row;
    cfg.rows[0] = deltas;
    let texts = vec![LabelText {
        row: 0,
        part: 0,
        text: "Δ1ч".into(),
        prefix: String::new(),
        reachable: false,
        venue: None,
        sign: None,
        color: None,
    }];
    assert_eq!(
        group_lines(&cfg, &texts, LabelZone::ChartTop, LabelAlign::Right),
        vec![vec![vec![0]]]
    );
}

/// An arbitrage column stacks whatever the MODULE's flow says: its lines are venues, one under
/// another, and the flow switch decides how the module's ordinary captions run instead.
///
/// Breakage this pins: letting the flow apply to the column. With "in a row" it would give each
/// venue a cell of its own across one line — a row of prices with no way to tell whose they are,
/// and the width budget then drops all but the first few.
#[test]
fn an_arbitrage_column_stacks_whatever_the_flow_says() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut module = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    module.push_part(ChartLabelField::ArbColumn);
    module.push_part(ChartLabelField::Funding);
    // The flow a user can pick, and the one that used to break the column.
    module.flow = LabelFlow::Row;
    cfg.rows[0] = module;

    // Two venue lines from the column's own run range, then the module's ordinary caption.
    let base = moon_core::config::ARB_PART_BASE;
    let texts: Vec<LabelText> = [(base, "BinanceF 1"), (base + 1, "GateF 2"), (1, "+0.01%")]
        .into_iter()
        .map(|(part, text)| LabelText {
            row: 0,
            part,
            text: text.into(),
            prefix: String::new(),
            reachable: false,
            venue: None,
            sign: None,
            color: None,
        })
        .collect();

    let lines = group_lines(&cfg, &texts, LabelZone::ChartTop, LabelAlign::Left);

    assert_eq!(
        lines,
        vec![vec![vec![0, 1], vec![2]]],
        "the venues share one cell; the caption beside them takes its own, as the flow says"
    );

    // And with the column flow the module's own caption joins the block instead.
    let mut stacked = cfg.clone();
    stacked.rows[0].flow = LabelFlow::Column;
    let lines = group_lines(&stacked, &texts, LabelZone::ChartTop, LabelAlign::Left);
    assert_eq!(lines, vec![vec![vec![0, 1], vec![2]]]);
}
