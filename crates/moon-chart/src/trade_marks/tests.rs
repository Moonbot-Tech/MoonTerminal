//! Unit tests for closed-trade-history geometry.

use super::*;

/// `trade_marks.rs:fold_cluster` — replacing the shared `cluster_weight` clamp with the raw
/// quantity in the `total` accumulator lets a cluster holding a positive and a negative quantity
/// divide the (still clamped) numerators by an unclamped denominator, placing the aggregate
/// marker outside the range of its own members — a price at which nothing traded.
#[test]
fn fold_cluster_aggregate_stays_inside_member_range_with_mixed_sign_qty() {
    let higher = TradeAction {
        t_ms: 1_000,
        price: 10.0,
        qty: 2.0,
        buy: true,
        is_short: false,
        mark: 0,
    };
    let lower = TradeAction {
        t_ms: 500,
        price: 5.0,
        qty: -1.0,
        buy: true,
        is_short: false,
        mark: 1,
    };
    let group = [&higher, &lower];
    let cluster = fold_cluster(&group, (true, false));

    let (t_lo, t_hi) = (lower.t_ms as f64, higher.t_ms as f64);
    let (p_lo, p_hi) = (lower.price, higher.price);
    assert!(
        cluster.t_ms >= t_lo && cluster.t_ms <= t_hi,
        "aggregate t_ms {} escaped member range [{t_lo}, {t_hi}]",
        cluster.t_ms
    );
    assert!(
        cluster.price >= p_lo && cluster.price <= p_hi,
        "aggregate price {} escaped member range [{p_lo}, {p_hi}]",
        cluster.price
    );
}

/// `trade_marks.rs:hit_trade_marks` must apply the same arrow-size multiplier as
/// `build_trade_geometry`; omitting it leaves the enlarged arrow's lower body unclickable.
#[test]
fn trade_hit_area_grows_with_the_drawn_arrow_scale() {
    let mark = TradeMark {
        buy_ms: 1_000,
        close_ms: 2_000,
        buy_price: 100.0,
        sell_price: 101.0,
        qty: 1.0,
        is_short: false,
        show_entry: true,
        show_exit: true,
    };
    let base_ctx = TradeGeometryCtx {
        epoch_ms: 0.0,
        long_rgb: [1, 2, 3],
        short_rgb: [4, 5, 6],
        scale: 1.0,
        px_per_ms: 1.0,
        px_per_price: 1.0,
        arrow_scale: 1.0,
        connector_thickness: CONNECTOR_THICKNESS,
        hovered: None,
    };
    let mut base_markers = Vec::new();
    let mut base_segs = Vec::new();
    build_trade_geometry(&[mark], &base_ctx, &mut base_markers, &mut base_segs);

    let enlarged_ctx = TradeGeometryCtx {
        arrow_scale: 2.0,
        ..base_ctx
    };
    let mut enlarged_markers = Vec::new();
    let mut enlarged_segs = Vec::new();
    build_trade_geometry(
        &[mark],
        &enlarged_ctx,
        &mut enlarged_markers,
        &mut enlarged_segs,
    );
    let base_arrow = base_markers
        .iter()
        .find(|marker| marker.shape == MARKER_SHAPE_ARROW_UP)
        .expect("a long entry must draw an up arrow");
    let enlarged_arrow = enlarged_markers
        .iter()
        .find(|marker| marker.shape == MARKER_SHAPE_ARROW_UP)
        .expect("a long entry must draw an up arrow");
    assert_eq!(enlarged_arrow.size, base_arrow.size * 2.0);
    assert_eq!(enlarged_arrow.thickness, base_arrow.thickness * 2.0);

    let drawn_arrow = TradeMarkAt {
        x: 0.0,
        apex_y: 0.0,
        buy: true,
        count: 1,
    };
    let cursor_in_only_the_enlarged_body = (0.0, 20.0);
    assert_eq!(
        hit_trade_marks([drawn_arrow], cursor_in_only_the_enlarged_body, 1.0, 1.0,),
        None,
        "the cursor must be beyond the default-size arrow body"
    );
    assert_eq!(
        hit_trade_marks([drawn_arrow], cursor_in_only_the_enlarged_body, 1.0, 2.0,)
            .expect("the drawn two-times arrow must reach the same cursor")
            .stack,
        vec![0]
    );
}

/// The shipped graphics settings must survive their own normalizer.
///
/// `normalize_chart_graphics` exists because this value is COMPARED — a chart re-bakes its base
/// texture when its settings differ from the ones it drew with — so a value that the normalizer
/// still moves differs from itself on every notification, which is a re-bake per frame rather than
/// a wrong pixel. The defaults are handed straight to a chart by
/// `WindowLayout::reset_chart_graphics_default`, without passing the normalizer on the way, and
/// this is what keeps that shortcut honest: a `def_*` that ever drifts outside its own clamp fails
/// here rather than in the frame loop.
#[test]
fn the_shipped_graphics_survive_their_own_normalizer() {
    let shipped = ChartGraphicsCfg::default();
    assert_eq!(
        normalize_chart_graphics(shipped),
        shipped,
        "a shipped graphics default sits outside the range its own normalizer accepts"
    );
}

/// `trade_marks.rs:normalize_chart_graphics` — the two stored band fields come out as ONE switch:
/// a profile on bars, or on hills with the split off, or on the split over an OFF candle half, is
/// hills with the split on; a profile with the band off stays off. Reading either field without
/// the other would carry a half-state into the renderer and the popup.
#[test]
fn normalize_folds_the_band_fields_onto_the_one_switch() {
    use moon_core::market::candles::{
        VOLUME_STYLE_HILLS, VOLUME_STYLE_LEGACY_BARS, VOLUME_STYLE_OFF,
    };
    let mut cfg = ChartGraphicsCfg::default();
    cfg.candle_volume_style = VOLUME_STYLE_LEGACY_BARS;
    cfg.candle_volume_sides = false;
    let out = normalize_chart_graphics(cfg);
    assert_eq!(out.candle_volume_style, VOLUME_STYLE_HILLS);
    assert!(out.candle_volume_sides);

    cfg.candle_volume_style = VOLUME_STYLE_OFF;
    cfg.candle_volume_sides = true;
    let out = normalize_chart_graphics(cfg);
    assert_eq!(out.candle_volume_style, VOLUME_STYLE_HILLS);
    assert!(out.candle_volume_sides);

    cfg.candle_volume_style = VOLUME_STYLE_OFF;
    cfg.candle_volume_sides = false;
    let out = normalize_chart_graphics(cfg);
    assert_eq!(out.candle_volume_style, VOLUME_STYLE_OFF);
    assert!(!out.candle_volume_sides);

    // The shipped default is the switch ON, and it is already normal.
    let def = ChartGraphicsCfg::default();
    assert_eq!(normalize_chart_graphics(def), def);
    assert!(def.candle_volume_sides);
}

/// Seeded xorshift so every fixture is reproducible.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

/// Random round trips plus a dense chain of entries every 3 px, spanning many window widths.
fn culling_fixture(rng: &mut Rng, ppm: f64) -> Vec<TradeMark> {
    let mut marks = Vec::new();
    for _ in 0..1_500 {
        let buy_ms = 1_000_000 + rng.below(4_000_000) as i64;
        let close_ms = buy_ms + rng.below(200_000) as i64;
        marks.push(TradeMark {
            buy_ms,
            close_ms,
            buy_price: 100.0 + rng.below(40) as f64 * 0.25,
            sell_price: 100.0 + rng.below(40) as f64 * 0.25,
            qty: 1.0 + rng.below(5) as f64,
            is_short: rng.below(2) == 0,
            show_entry: true,
            show_exit: rng.below(8) != 0,
        });
    }
    // 3 px apart at one price: every member is within reach of its neighbour, so where a group
    // starts depends only on where the sweep started the chain.
    let step = (3.0 / ppm) as i64;
    for i in 0..2_000 {
        marks.push(TradeMark {
            buy_ms: 2_000_000 + i * step,
            close_ms: 9_000_000,
            buy_price: 120.0,
            sell_price: 120.0,
            qty: 1.0,
            is_short: false,
            show_entry: true,
            show_exit: false,
        });
    }
    marks
}

/// Times of the actions a cluster was built from.
fn member_times<'a>(c: &'a TradeCluster, marks: &'a [TradeMark]) -> impl Iterator<Item = i64> + 'a {
    c.members.iter().map(move |&m| {
        let mark = &marks[m];
        // An entry buys for a long; an exit buys for a short.
        if c.buy != mark.is_short {
            mark.buy_ms
        } else {
            mark.close_ms
        }
    })
}

fn bitwise_eq(a: &TradeCluster, b: &TradeCluster) -> bool {
    a.members == b.members
        && a.buy == b.buy
        && a.is_short == b.is_short
        && a.t_ms.to_bits() == b.t_ms.to_bits()
        && a.price.to_bits() == b.price.to_bits()
}

/// `trade_marks.rs:cluster_sorted` starting its windowed sweep at the first action inside the
/// window (dropping the backward walk to a gap the full sweep also cuts at) regroups the clusters
/// at the window's edge, so arrows jump or split as the user pans. Every cluster with a member in
/// the window must be bitwise the one the unculled sweep builds, and a pan by one cell must leave
/// the clusters both windows see unchanged.
#[test]
fn culled_clusters_equal_the_full_sweep_across_the_window_edges() {
    let ppm = 0.01_f32;
    let ppp = 4.0_f32;
    let mut rng = Rng(0x5EED_1234);
    let marks = culling_fixture(&mut rng, f64::from(ppm));
    let sorted = sort_actions(&explode_actions(&marks));
    take_cluster_visited();
    let full = cluster_sorted(&sorted, None, ppm, ppp, 1.0);
    let before = take_cluster_visited();
    let width = 150_000.0;
    let cell = 30_000.0;
    let mut chain_edges = 0;
    for q in 0..300 {
        let lo = 900_000.0 + rng.below(5_200_000) as f64 + if q % 2 == 0 { 0.5 } else { 0.0 };
        let hi = lo + width;
        let a = cluster_sorted(&sorted, Some((lo, hi)), ppm, ppp, 1.0);
        let after = take_cluster_visited();
        let b = cluster_sorted(&sorted, Some((lo + cell, hi + cell)), ppm, ppp, 1.0);
        take_cluster_visited();
        for c in &full {
            let times: Vec<i64> = member_times(c, &marks).collect();
            let in_a = times.iter().any(|&t| t as f64 >= lo && t as f64 <= hi);
            let in_b = times
                .iter()
                .any(|&t| t as f64 >= lo + cell && t as f64 <= hi + cell);
            if in_a {
                assert!(
                    a.iter().any(|x| bitwise_eq(x, c)),
                    "window [{lo}, {hi}] lost or regrouped {c:?}"
                );
                if times.iter().any(|&t| (t as f64) < lo) && c.price == 120.0 {
                    chain_edges += 1;
                }
            }
            if in_a && in_b {
                assert!(a.iter().chain(&b).filter(|x| bitwise_eq(x, c)).count() == 2);
            }
        }
        if q % 100 == 0 {
            println!("[cluster] before={before} after={after}");
        }
    }
    assert!(
        chain_edges > 0,
        "no window edge fell inside a chain cluster"
    );
}

/// `trade_marks.rs:hit_trade_marks_windowed` / `hit_indexed` dropping the ascending sort of
/// `stack`, or reaching only as far as an ungrown arrow (the `GROWTH_CAP` term), makes hover pick
/// a different arrow or miss the edge of a big cluster. Compared with the full forward scan of
/// `hit_trade_marks` over every cluster.
#[test]
fn windowed_hit_equals_the_full_scan() {
    let ppm = 0.02_f32;
    let ppp = 3.0_f32;
    let scale = 1.25_f32;
    let arrow_scale = 1.0_f32;
    let mut rng = Rng(0xABCD_EF01);
    let mut marks = culling_fixture(&mut rng, f64::from(ppm));
    // Buy and sell arrows stacked on one spot, emitted far apart in cluster order.
    for i in 0..200 {
        let t = 1_500_000 + i * 20_000;
        marks.push(TradeMark {
            buy_ms: t,
            close_ms: t + 50,
            buy_price: 110.0,
            sell_price: 110.0,
            qty: 1.0,
            is_short: i % 2 == 0,
            show_entry: true,
            show_exit: true,
        });
    }
    // Ten entries on one spot: a cluster drawn at the growth cap.
    for g in 0..40 {
        for _ in 0..10 {
            marks.push(TradeMark {
                buy_ms: 1_200_000 + g * 97_000,
                close_ms: 9_500_000,
                buy_price: 95.0,
                sell_price: 95.0,
                qty: 1.0,
                is_short: false,
                show_entry: true,
                show_exit: false,
            });
        }
    }
    let clusters = cluster_actions(&explode_actions(&marks), ppm, ppp, scale);
    let order = order_by_time(&clusters);
    let x_of = |t: f64| ((t - 1_000_000.0) * f64::from(ppm)) as f32;
    let y_of = |p: f64| (500.0 - (p - 100.0) * f64::from(ppp)) as f32;
    let all: Vec<TradeMarkAt> = clusters
        .iter()
        .map(|c| TradeMarkAt {
            x: x_of(c.t_ms),
            apex_y: y_of(c.price),
            buy: c.buy,
            count: c.members.len() as u32,
        })
        .collect();
    let mut cursors = Vec::new();
    for (c, at) in clusters.iter().zip(&all) {
        // The far x edge of the resting glyph, where only a fully grown reach still hits.
        let reach =
            ARROW_HALF_W * scale * cluster_growth(c.members.len() as u32) + HIT_SLACK * scale;
        let dy = if c.buy { 2.0 } else { -2.0 };
        cursors.push((at.x + reach * 0.98, at.apex_y + dy));
        cursors.push((at.x - reach * 0.98, at.apex_y + dy));
        cursors.push((at.x, at.apex_y + dy));
    }
    for _ in 0..3_000 {
        cursors.push((
            rng.below(90_000) as f32 * 0.1,
            rng.below(3_000) as f32 * 0.25,
        ));
    }
    let (mut hits, mut multi, mut big_edge) = (0, 0, 0);
    for (k, &cursor) in cursors.iter().enumerate() {
        take_hit_visited();
        let want = hit_trade_marks(all.iter().copied(), cursor, scale, arrow_scale);
        let before = take_hit_visited();
        let got =
            hit_trade_marks_windowed(&clusters, &order, x_of, y_of, cursor, scale, arrow_scale);
        let after = take_hit_visited();
        assert_eq!(got, want, "cursor {cursor:?}");
        if let Some(hit) = &want {
            hits += 1;
            multi += usize::from(hit.stack.len() > 1);
            big_edge += usize::from(k % 3 == 0 && clusters[hit.nearest].members.len() >= 8);
        }
        if k % 1_000 == 0 {
            println!("[hit] before={before} after={after}");
        }
    }
    assert!(
        hits > 0 && multi > 0 && big_edge > 0,
        "{hits} {multi} {big_edge}"
    );
}
