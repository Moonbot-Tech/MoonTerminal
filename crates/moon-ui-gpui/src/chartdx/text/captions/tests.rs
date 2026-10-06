//! The line rule: which captions share a line, and which share a column inside it.
//!
//! Explicit imports: the chartdx parent re-exports `gpui::*`, whose own `test` shadows the built-in
//! attribute and makes `#[test]` expand recursively.

use moon_core::config::{
    ChartLabelField, ChartLabelRow, ChartLabelsCfg, LabelAlign, LabelFlow, LabelZone,
};

use super::super::caption::CaptionGeom;
use super::super::labels::LabelText;
use super::fit::group_lines;
use super::model::{CaptionGeomInput, MAX_CAPTION_LINE_H, ZONE_PAD};
use super::zone::zone_start_y;

/// Synthetic modules cover every zone and alignment with five captions each.
fn row_plan_fixture() -> (ChartLabelsCfg, Vec<LabelText>) {
    let mut cfg = ChartLabelsCfg::empty();
    let mut texts = Vec::new();
    for (row, (zone, align)) in LabelZone::ALL
        .into_iter()
        .flat_map(|zone| LabelAlign::ALL.map(|align| (zone, align)))
        .enumerate()
    {
        cfg.rows[row] = ChartLabelRow::new(zone, align);
        for part in 0..5 {
            cfg.rows[row].push_part(ChartLabelField::Coin);
            texts.push(LabelText {
                row,
                part,
                text: format!("caption {row} {part}"),
                prefix: String::new(),
                reachable: false,
                venue: None,
                sign: None,
                color: None,
                bar: None,
                volume_menu: false,
                action: None,
            });
        }
    }
    (cfg, texts)
}

/// Measures grouping and row construction against cloning pristine rows in the test profile.
#[test]
#[ignore]
fn caption_row_plan_bench() {
    use std::{hint::black_box, time::Instant};
    let engine = crate::chartdx::ChartEngine::new(
        1_700_000_000_000.0,
        moon_core::config::ChartTheme::default(),
    );
    let data = engine.data.borrow();
    let state = data.render.borrow();
    let (cfg, texts) = row_plan_fixture();
    assert_eq!(texts.len(), 60);
    let cfg = std::rc::Rc::new(cfg);
    let mut cache = None;
    super::RowPlanCache::working_rows(&mut cache, 1, &cfg, state.label_font_px(), |zone, align| {
        state.collect_rows(&cfg, &texts, zone, align)
    });
    let start = Instant::now();
    let mut rebuild_calls = 0;
    for _ in 0..10_000 {
        for zone in LabelZone::ALL {
            for align in LabelAlign::ALL {
                black_box(state.collect_rows(&cfg, &texts, zone, align));
                rebuild_calls += 1;
            }
        }
    }
    let rebuild = start.elapsed();
    let mut cached_builder_calls = 0;
    let start = Instant::now();
    for _ in 0..10_000 {
        black_box(super::RowPlanCache::working_rows(
            &mut cache,
            1,
            &cfg,
            state.label_font_px(),
            |zone, align| {
                cached_builder_calls += 1;
                state.collect_rows(&cfg, &texts, zone, align)
            },
        ));
    }
    let cached = start.elapsed();
    assert_eq!(rebuild_calls, 120_000);
    assert_eq!(cached_builder_calls, 0);
    println!(
        "caption_row_plan_bench test profile: REBUILD {rebuild:?} builder_calls={rebuild_calls}; CACHED {cached:?} builder_calls={cached_builder_calls}"
    );
}

/// A stable key must skip the production builder on the second pass.
#[test]
fn row_plan_reused_while_key_unchanged() {
    let engine = crate::chartdx::ChartEngine::new(0.0, moon_core::config::ChartTheme::default());
    let data = engine.data.borrow();
    let state = data.render.borrow();
    let (cfg, texts) = row_plan_fixture();
    let cfg = std::rc::Rc::new(cfg);
    let mut cache = None;
    let mut calls = 0;
    for pass in 0..2 {
        let before = calls;
        super::RowPlanCache::working_rows(&mut cache, 1, &cfg, 11.5, |zone, align| {
            calls += 1;
            state.collect_rows(&cfg, &texts, zone, align)
        });
        assert_eq!(calls - before, if pass == 0 { 12 } else { 0 });
    }
}

/// Each grouping or styling key input independently invalidates the plan.
#[test]
fn row_plan_rebuilt_on_generation_cfg_or_font_change() {
    let engine = crate::chartdx::ChartEngine::new(0.0, moon_core::config::ChartTheme::default());
    let data = engine.data.borrow();
    let state = data.render.borrow();
    let (cfg, texts) = row_plan_fixture();
    let cfg = std::rc::Rc::new(cfg);
    let replacement = std::rc::Rc::new((*cfg).clone());
    let mut cache = None;
    for (generation, cfg, font) in [
        (1, &cfg, 11.5),
        (2, &cfg, 11.5),
        (2, &replacement, 11.5),
        (2, &replacement, 12.5),
    ] {
        let mut calls = 0;
        super::RowPlanCache::working_rows(&mut cache, generation, cfg, font, |zone, align| {
            calls += 1;
            state.collect_rows(cfg, &texts, zone, align)
        });
        assert_eq!(calls, 12);
    }
}

/// A short pane must not permanently discard filter entries from the cached plan.
#[test]
fn filter_rows_survive_short_then_tall_pane() {
    let _locale = crate::test_locale::force("en");
    let engine = crate::chartdx::ChartEngine::new(0.0, moon_core::config::ChartTheme::default());
    let data = engine.data.borrow();
    let state = data.render.borrow();
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = moon_core::config::strategy_filters_row();
    let cfg = std::rc::Rc::new(cfg);
    let view = std::rc::Rc::new(moon_core::config::ArbViewCfg::default());
    let mut labels = super::super::labels::LabelState::default();
    labels.update(
        &cfg,
        &view,
        super::super::labels::LabelInputs {
            filter_lines: vec!["first".into(), "second".into(), "third".into()],
            ..Default::default()
        },
    );
    let mut cache = None;
    let mut short = super::RowPlanCache::working_rows(
        &mut cache,
        labels.generation,
        &cfg,
        state.label_font_px(),
        |zone, align| state.collect_rows(&cfg, &labels.texts, zone, align),
    );
    let cell = short
        .iter_mut()
        .flatten()
        .flat_map(|rows| rows.iter_mut())
        .flat_map(|row| row.cells.iter_mut())
        .find(|cell| cell.items.len() == 4)
        .expect("header and three entries");
    assert!(cell.fit_filter_header(&labels.texts, cell.items[0].block_h() * 2.0));
    assert_eq!(cell.items.len(), 2);
    let mut tall = super::RowPlanCache::working_rows(
        &mut cache,
        labels.generation,
        &cfg,
        state.label_font_px(),
        |_, _| panic!("same key must reuse"),
    );
    let cell = tall
        .iter_mut()
        .flatten()
        .flat_map(|rows| rows.iter_mut())
        .flat_map(|row| row.cells.iter_mut())
        .find(|cell| cell.items.len() == 4)
        .expect("all entries survive");
    assert!(!cell.fit_filter_header(&labels.texts, 1000.0));
    assert_eq!(cell.items.len(), 4);
}

/// Wrapped working items must not carry narrow-pane line counts or wrap indices into a wide pane.
#[test]
fn narrow_then_wide_pane_reuses_pristine_rows() {
    let engine = crate::chartdx::ChartEngine::new(0.0, moon_core::config::ChartTheme::default());
    let data = engine.data.borrow();
    let state = data.render.borrow();
    let (cfg, texts) = row_plan_fixture();
    let cfg = std::rc::Rc::new(cfg);
    let mut cache = None;
    let mut narrow = super::RowPlanCache::working_rows(&mut cache, 1, &cfg, 11.5, |zone, align| {
        state.collect_rows(&cfg, &texts, zone, align)
    });
    for item in narrow
        .iter_mut()
        .flatten()
        .flat_map(|rows| rows.iter_mut())
        .flat_map(|row| row.cells.iter_mut())
        .flat_map(|cell| cell.items.iter_mut())
    {
        item.lines = 3;
        item.wrap_ix = 42;
    }
    let wide = super::RowPlanCache::working_rows(&mut cache, 1, &cfg, 11.5, |_, _| {
        panic!("same key must reuse")
    });
    let items: Vec<_> = wide
        .iter()
        .flatten()
        .flat_map(|rows| rows.iter())
        .flat_map(|row| row.cells.iter())
        .flat_map(|cell| cell.items.iter())
        .collect();
    assert_eq!(items.len(), 60);
    assert!(
        items
            .iter()
            .all(|item| item.lines == 1 && item.wrap_ix == usize::MAX)
    );
}

/// Treating the filters header like an ordinary name places it beside the lines instead of
/// above them; retaining hidden texts prevents the following module from reclaiming height.
#[test]
fn filter_header_and_lines_share_one_stack() {
    let _locale = crate::test_locale::force("en");
    use super::super::labels::{LabelInputs, LabelState};
    use std::rc::Rc;
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = moon_core::config::strategy_filters_row();
    cfg.rows[0].flow = LabelFlow::Row;
    cfg.rows[1] = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    cfg.rows[1].push_part(ChartLabelField::Coin);
    let inputs = LabelInputs {
        ticker: "BTC".into(),
        filter_lines: vec!["first".into(), "second".into()],
        ..Default::default()
    };
    let view = Rc::new(moon_core::config::ArbViewCfg::default());
    let mut state = LabelState::default();
    state.update(&Rc::new(cfg.clone()), &view, inputs.clone());
    assert_eq!(
        group_lines(&cfg, &state.texts, LabelZone::ChartTop, LabelAlign::Left),
        vec![vec![vec![0, 1, 2]], vec![vec![3]]]
    );
    cfg.rows[0].collapsed = true;
    state.update(&Rc::new(cfg.clone()), &view, inputs);
    assert_eq!(
        group_lines(&cfg, &state.texts, LabelZone::ChartTop, LabelAlign::Left),
        vec![vec![vec![0]], vec![vec![1]]]
    );
}

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
            bar: None,
            volume_menu: false,
            action: None,
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
        bar: None,
        volume_menu: false,
        action: None,
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
            bar: None,
            volume_menu: false,
            action: None,
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

/// A module reserves ONE bar track, however many of its lines draw one.
///
/// Breakage: charged per line, the reserve depended on which line happened to be widest — so `Bv`
/// and `Sv`, being different widths, started their tracks in different places. A pair of bars that
/// do not share a vertical cannot be compared, which is the only thing a bar is for.
#[test]
fn a_module_reserves_one_bar_track_for_the_whole_column() {
    use super::model::{BAR_ZONE, Cell, Item, bar_zone};

    let bar = crate::chartdx::text::labels::VolumeBar {
        fill: 0.5,
        sell: false,
    };
    let with_bar = LabelText {
        row: 0,
        part: 0,
        text: "12.7K".into(),
        prefix: "Bv: ".into(),
        reachable: false,
        venue: None,
        sign: None,
        color: None,
        bar: Some(bar),
        volume_menu: true,
        action: None,
    };
    let plain = LabelText {
        part: 1,
        bar: None,
        ..with_bar.clone()
    };
    let item = |pos: usize| Item {
        pos,
        row: 0,
        part: pos,
        style: ChartLabelField::WindowBuyVolume.default_style(),
        plate: true,
        size: 11.0,
        wraps: false,
        lines: 1,
        wrap_ix: usize::MAX,
    };

    let both = Cell {
        gap: 0.0,
        items: vec![item(0), item(1)],
    };
    assert_eq!(
        bar_zone(&[with_bar.clone(), plain.clone()], &both),
        BAR_ZONE,
        "two lines, one of them barred: one track"
    );

    let barless = Cell {
        gap: 0.0,
        items: vec![item(1)],
    };
    assert_eq!(
        bar_zone(&[with_bar, plain], &barless),
        0.0,
        "a module with no bars reserves nothing"
    );
}

/// The two shipped buttons stand SIDE BY SIDE on one line of the plot's bottom band.
///
/// The property the whole migration rests on: two buttons that shared an anchor were drawn in a
/// row, and a reader who never touched the setting has to find them exactly where they were. It is
/// the grouping rule that decides that — not the drawing — so it is answerable without a device.
#[test]
fn the_shipped_buttons_share_one_line_along_the_bottom() {
    let cfg = ChartLabelsCfg::default();
    let rows: Vec<usize> = cfg
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.holds_action())
        .map(|(ix, _)| ix)
        .collect();
    assert_eq!(rows.len(), 2, "the shipped set places both buttons");
    let texts: Vec<LabelText> = rows
        .iter()
        .map(|row| LabelText {
            row: *row,
            part: 0,
            text: "Panic Sell".to_string(),
            prefix: String::new(),
            reachable: false,
            venue: None,
            sign: None,
            color: None,
            bar: None,
            volume_menu: false,
            action: None,
        })
        .collect();
    assert_eq!(
        group_lines(&cfg, &texts, LabelZone::ChartBottom, LabelAlign::Right),
        vec![vec![vec![0], vec![1]]],
        "one line, two columns — the pair as the old layout drew it"
    );
}

/// Plot 400 tall. The corner geometry is unused on ChartBottom / ChartTop.
fn plot_geom() -> CaptionGeomInput {
    CaptionGeomInput {
        pane_left: 0.0,
        pane_right: 1000.0,
        plot_left: 0.0,
        plot_right: 800.0,
        plot_top: 50.0,
        plot_bottom: 450.0,
        orderbook_enabled: false,
        orderbook_left: 0.0,
        scale_factor: 1.0,
        volume_band_h: 0.0,
        corner_strip_h: 0.0,
    }
}

fn corner() -> CaptionGeom {
    CaptionGeom {
        zone_left: 800.0,
        right_x: 970.0,
        top_y: 54.0,
        max_w: 170.0,
    }
}

/// Plot 400 tall, volume band 80. The corner geometry is unused on ChartBottom / ChartTop.
fn start_y(zone: LabelZone, align: LabelAlign, volume_band_h: f32, corner_strip_h: f32) -> f32 {
    let geom = CaptionGeomInput {
        volume_band_h,
        corner_strip_h,
        ..plot_geom()
    };
    zone_start_y(zone, align, &geom, &corner())
}

/// Every module in ChartBottom sits above the volume bars, not on them. This is the floor the
/// stack grows up from, so a Core name, the action buttons and the filter list all lift together.
#[test]
fn chart_bottom_sits_above_the_volume_band() {
    assert_eq!(
        start_y(LabelZone::ChartBottom, LabelAlign::Left, 80.0, 0.0),
        450.0 - 80.0 - ZONE_PAD
    );
}

/// Volumes off: ChartBottom stays on the plot floor, the way it always did.
#[test]
fn chart_bottom_without_volumes_sits_on_the_plot_floor() {
    assert_eq!(
        start_y(LabelZone::ChartBottom, LabelAlign::Left, 0.0, 0.0),
        450.0 - ZONE_PAD
    );
}

/// The control strip's floor is the plot's. The volume bars never reach it, so lifting it would
/// float ZoneBottom captions into empty plot for no reason.
#[test]
fn the_control_strip_floor_ignores_the_volume_band() {
    assert_eq!(
        start_y(LabelZone::ZoneBottom, LabelAlign::Left, 80.0, 0.0),
        450.0 - ZONE_PAD
    );
}

/// ChartTop is the plot's upper edge. A volume-band height must not walk it down the pane.
#[test]
fn chart_top_ignores_the_volume_band() {
    assert_eq!(
        start_y(LabelZone::ChartTop, LabelAlign::Left, 80.0, 0.0),
        50.0 + ZONE_PAD
    );
}

/// Breakage 1 (PROVE): the guard itself. Room exactly at the cutoff still reserves the strip;
/// one pixel short drops it entirely rather than pushing a caption's first line past
/// `plot_bottom` on a compressed stack slot.
#[test]
fn chart_top_left_guard_pins_the_room_boundary() {
    fn y(plot_bottom: f32) -> f32 {
        let geom = CaptionGeomInput {
            plot_bottom,
            corner_strip_h: 18.0,
            ..plot_geom()
        };
        zone_start_y(LabelZone::ChartTop, LabelAlign::Left, &geom, &corner())
    }
    let strip = 18.0;
    // room == plot_bottom - plot_top - ZONE_PAD; the boundary is where room first covers the
    // strip plus the tallest a caption line can ever be.
    let boundary_bottom = 50.0 + ZONE_PAD + strip + MAX_CAPTION_LINE_H;
    assert_eq!(
        y(boundary_bottom),
        50.0 + ZONE_PAD + strip,
        "room == strip + MAX_CAPTION_LINE_H: still reserves the strip"
    );
    assert_eq!(
        y(boundary_bottom - 1.0),
        50.0 + ZONE_PAD,
        "room one pixel short: reserves nothing"
    );
}

/// Breakage 2 (PROVE): the left-aligned top band must clear the 18px button band.
#[test]
fn chart_top_left_clears_the_corner_strip() {
    assert_eq!(
        start_y(LabelZone::ChartTop, LabelAlign::Left, 0.0, 18.0),
        50.0 + ZONE_PAD + 18.0
    );
}

/// Center/right ChartTop never compete with the corner buttons (left edge only).
#[test]
fn chart_top_center_and_right_ignore_the_corner_strip() {
    assert_eq!(
        start_y(LabelZone::ChartTop, LabelAlign::Center, 0.0, 18.0),
        50.0 + ZONE_PAD
    );
    assert_eq!(
        start_y(LabelZone::ChartTop, LabelAlign::Right, 0.0, 18.0),
        50.0 + ZONE_PAD
    );
}

/// The control strip keeps its own inset (`corner.top_y`); widening the gate would double it.
#[test]
fn the_control_strip_ignores_the_corner_strip() {
    assert_eq!(
        start_y(LabelZone::ZoneTop, LabelAlign::Left, 0.0, 18.0),
        54.0
    );
}

/// ChartBottom shares nothing with the corner strip (plot's top-left only).
#[test]
fn chart_bottom_ignores_the_corner_strip() {
    assert_eq!(
        start_y(LabelZone::ChartBottom, LabelAlign::Left, 0.0, 18.0),
        450.0 - ZONE_PAD
    );
}

fn wrap_item(part: usize, wraps: bool) -> super::model::Item {
    super::model::Item {
        pos: 0,
        row: 0,
        part,
        style: ChartLabelField::StrategyFilters.default_style(),
        plate: false,
        size: 11.0,
        wraps,
        lines: 1,
        wrap_ix: usize::MAX,
    }
}

/// Omitting the pre-anchor trim puts an oversized bottom module's only control above the pane.
/// The exact-height and one-pixel-short cases also protect the last complete visible entry.
#[test]
fn overflowing_filter_cells_keep_the_header_inside_the_bottom_band() {
    let _locale = crate::test_locale::force("en");
    use super::super::labels::{LabelInputs, LabelState};
    use std::rc::Rc;
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = moon_core::config::strategy_filters_row();
    cfg.rows[0].zone = LabelZone::ChartBottom;
    let mut state = LabelState::default();
    state.update(
        &Rc::new(cfg),
        &Rc::new(Default::default()),
        LabelInputs {
            filter_lines: vec!["reason".into(); 5],
            ..Default::default()
        },
    );
    let cell = || super::model::Cell {
        gap: 0.0,
        items: state
            .texts
            .iter()
            .enumerate()
            .map(|(pos, text)| {
                let mut item = wrap_item(text.part, false);
                item.pos = pos;
                item
            })
            .collect(),
    };
    for (height, expected_items) in [(45.0, 3), (44.0, 2), (15.0, 1)] {
        let mut fitted = cell();
        fitted.fit_filter_header(&state.texts, height);
        assert_eq!(fitted.items.len(), expected_items);
        assert_eq!(fitted.items[0].part, moon_core::config::FILTER_HEADER_PART);
        assert!(
            100.0 - fitted.height() >= 100.0 - height,
            "bottom anchoring must keep the header above the retained filter lines and on-pane"
        );
    }
    let mut wrapped = cell();
    wrapped.items[1].lines = 3;
    wrapped.fit_filter_header(&state.texts, 60.0);
    assert_eq!(
        wrapped.items.len(),
        2,
        "a wrapped entry consumes all three lines"
    );
    let mut ordinary = cell();
    ordinary.items.remove(0);
    ordinary.fit_filter_header(&state.texts, 15.0);
    assert_eq!(
        ordinary.items.len(),
        5,
        "other caption cells keep their existing clipping"
    );
}

/// Moving the filter control back to the row name separates it from entries after Coin,
/// disabling the bottom-band trim and pushing the only collapse target above a short pane.
#[test]
fn mixed_filter_column_keeps_its_header_above_entries_inside_a_short_pane() {
    let _locale = crate::test_locale::force("en");
    use super::super::labels::{LabelAction, LabelInputs, LabelState};
    use std::rc::Rc;
    for zone in [LabelZone::ChartBottom, LabelZone::ZoneBottom] {
        for flow in [LabelFlow::Row, LabelFlow::Column] {
            let mut cfg = ChartLabelsCfg::empty();
            let row = &mut cfg.rows[0];
            *row = ChartLabelRow::new(zone, LabelAlign::Left);
            row.flow = flow;
            row.push_part(ChartLabelField::Coin);
            row.push_part(ChartLabelField::StrategyFilters);
            let mut state = LabelState::default();
            state.update(
                &Rc::new(cfg.clone()),
                &Rc::new(Default::default()),
                LabelInputs {
                    ticker: "BTC".into(),
                    filter_lines: vec!["reason".into(); 5],
                    ..Default::default()
                },
            );
            let grouped = group_lines(&cfg, &state.texts, zone, LabelAlign::Left);
            assert_eq!(grouped, vec![vec![vec![0], vec![1, 2, 3, 4, 5, 6]]]);
            let mut column = super::model::Cell {
                gap: 0.0,
                items: grouped[0][1]
                    .iter()
                    .map(|&pos| {
                        let mut item = wrap_item(state.texts[pos].part, false);
                        item.pos = pos;
                        item
                    })
                    .collect(),
            };
            assert!(column.fit_filter_header(&state.texts, 45.0));
            assert_eq!(column.items.len(), 3);
            let header = column.items[0];
            assert_eq!(
                state.texts[header.pos].action,
                Some(LabelAction::ToggleStrategyFilters)
            );
            assert_eq!(
                super::fit::caption_style(&cfg.rows[0], header.part),
                Some(ChartLabelRow::name_style())
            );
            let pane_top = 55.0;
            let bottom_anchor = 100.0;
            let header_y = bottom_anchor - column.height();
            assert!(header_y >= pane_top);
            let first_entry_y = header_y + header.block_h();
            assert!(first_entry_y > header_y);
            assert!(first_entry_y + column.items[1].block_h() <= bottom_anchor);
        }
    }
}

/// Skip-reason lines wrap, but they must not count as elastic prose. Two wrapping bands skip the
/// detect-line split; treating the filter column as prose would print it through the core name
/// again the moment a detect line shares the zone.
#[test]
fn a_wrapping_filter_column_is_hungry_not_elastic() {
    use super::model::Cell;
    use moon_core::config::ARB_PART_BASE;

    let cell = Cell {
        gap: 0.0,
        items: vec![wrap_item(ARB_PART_BASE, true)],
    };
    assert!(cell.has_column());
    assert!(cell.has_wrap());
    assert!(
        !cell.has_prose(),
        "a skip-reason column must not make the band elastic"
    );
}

/// A detect line is still the elastic prose the zone is divided for.
#[test]
fn a_detect_line_is_elastic_prose() {
    use super::model::Cell;

    let cell = Cell {
        gap: 0.0,
        items: vec![wrap_item(0, true)],
    };
    assert!(cell.has_prose());
    assert!(cell.has_wrap());
    assert!(!cell.has_column());
}

/// A column line grows its row's wheel band; an ordinary caption registers nothing.
#[test]
fn only_column_lines_grow_a_wheel_band() {
    let mut bands = Vec::new();
    super::fit::grow_column_band(&mut bands, &wrap_item(0, true), [0.0, 0.0, 10.0, 10.0]);
    assert!(bands.is_empty());
    let line = wrap_item(super::ARB_PART_BASE, true);
    super::fit::grow_column_band(&mut bands, &line, [4.0, 10.0, 20.0, 12.0]);
    super::fit::grow_column_band(&mut bands, &line, [4.0, 22.0, 30.0, 12.0]);
    assert_eq!(bands.len(), 1);
    assert_eq!(bands[0].rect, [4.0, 10.0, 30.0, 24.0]);
}

/// Strategy-filter lines are drawn through the wrapped branch, which `continue`s before the
/// single-line registration; without its own call the list has no band and Alt+wheel pans.
/// The draw loop needs a GPU text context, so the branch is checked in the source.
#[test]
fn the_wrapped_draw_branch_registers_column_lines() {
    let src = include_str!("stack.rs");
    let start = src
        .find("if item.wrap_ix < self.caption_wraps.len() {")
        .expect("wrapped branch");
    let end = start
        + src[start..]
            .find("self.caption_wraps[item.wrap_ix] = lines;")
            .expect("wrapped branch end");
    assert!(src[start..end].contains("grow_column_band("));
}
