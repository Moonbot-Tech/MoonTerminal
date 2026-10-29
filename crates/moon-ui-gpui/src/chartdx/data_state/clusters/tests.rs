//! Parity, stale-result and quiet-market checks for background bubble publication.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use super::worker::{Source, prepare};
use super::{ClusterInputs, FoldKey, LayoutKey, Pending, Request, refold_due, sync_clusters};
use crate::chartdx::pane_render::PaneRender;
use crate::chartdx::types::{ChartCross, ChartViewGpu, MarkerGpu, mk_of};
use moon_chart::clusters::{self, DiscRadii, PrintDisc};
use moon_chart::layers::{MARKER_SHAPE_KNOT, MarkerInstance};

/// Neutral samples include same-time ties, late trades, liquidations and unusable quantities.
fn samples() -> Vec<ChartCross> {
    [
        (0.0, 10.0, 1.0, 0),
        (1.0, 10.0, 2.0, 0),
        (1.0, 10.0, 4.0, 1),
        (120.0, 11.0, 3.0, 0),
        (110.0, 10.0, 5.0, 1),
        (500.0, 12.0, 9.0, 2),
        (700.0, 9.0, 0.0, 0),
        (900.0, 13.0, 8.0, 1),
    ]
    .into_iter()
    .map(|(time_rel, price, qty, side)| ChartCross {
        time_rel,
        price,
        qty,
        side,
    })
    .collect()
}

/// Describe a one-second visible window with its original one-window prefetch on either side.
fn request() -> Request {
    Request {
        fold: FoldKey {
            merge_ms: 100,
            small_pct: 50,
            tick_bits: 0.01f64.to_bits(),
        },
        layout: LayoutKey {
            time_to_px: 1.0,
            ppp: 1.0,
            colors: [[0.1, 0.6, 0.2], [0.8, 0.2, 0.1]],
        },
        range: (-1000.0, 2000.0),
        epoch: 0.0,
        generation: 0,
    }
}

/// Convert the captured identity back into the actual sync inputs used by the chart.
fn inputs(r: Request) -> ClusterInputs {
    ClusterInputs {
        enabled: true,
        tick: Some(f64::from_bits(r.fold.tick_bits)),
        view: ChartViewGpu {
            bounds: [0.0, 0.0, 1000.0, 600.0],
            time_to_px: r.layout.time_to_px,
            view_time0: 0.0,
            ..Default::default()
        },
        epoch_ms: r.epoch,
        ppp: r.layout.ppp,
        merge_ms: r.fold.merge_ms,
        small_pct: r.fold.small_pct,
        buy: r.layout.colors[0],
        sell: r.layout.colors[1],
    }
}

/// Frozen synchronous orchestration is the oracle for both disc geometry and GPU marker bytes.
fn synchronous(r: Request, tape: &[ChartCross]) -> (Vec<PrintDisc>, Vec<MarkerGpu>) {
    let (mut prints, mut order, mut scratch, mut discs) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    clusters::aggregate_prints(
        tape.iter()
            .filter(|c| c.side < 2)
            .map(|c| (r.epoch + f64::from(c.time_rel), c.price, c.qty, c.side == 0)),
        Some(f64::from_bits(r.fold.tick_bits)),
        f64::from(r.fold.merge_ms),
        &mut prints,
    );
    clusters::price_time_order(&prints, &mut order);
    if let Some(scale) = clusters::print_scale(&prints, r.fold.small_pct, &mut scratch) {
        clusters::merge_overlapping(
            &prints,
            &order,
            f64::from(r.layout.time_to_px),
            |v| clusters::print_radius(v, scale, DiscRadii::at(r.layout.ppp)),
            &mut discs,
        );
    }
    let [buy, sell] = r.layout.colors.map(|c| [c[0], c[1], c[2], 0.85]);
    let mut markers: Vec<_> = discs
        .iter()
        .map(|d| {
            MarkerInstance::at_price(
                (d.time_ms - r.epoch) as f32,
                d.price,
                d.radius,
                0.0,
                MARKER_SHAPE_KNOT,
                if d.buy >= d.sell { buy } else { sell },
            )
        })
        .collect();
    markers.sort_by(|a, b| a.size.total_cmp(&b.size));
    (discs, markers.iter().map(mk_of).collect())
}

/// Moving stages across threads must preserve every disc and the full stable GPU painter order.
#[test]
fn background_bubbles_match_synchronous_pixels() {
    let tape = samples();
    for merge in [0, 100, 1000] {
        for small in [0, 50, 90] {
            let mut r = request();
            r.fold.merge_ms = merge;
            r.fold.small_pct = small;
            r.layout.ppp = 1.5;
            r.layout.time_to_px = 0.05;
            let expected = synchronous(r, &tape);
            let pending = Pending::spawn(r, Source::Tape(tape.clone(), None)).unwrap();
            let actual = pending.receiver.recv().unwrap().unwrap();
            assert_eq!(actual.discs, expected.0);
            assert_eq!(
                bytemuck::cast_slice::<MarkerGpu, u8>(&actual.markers),
                bytemuck::cast_slice::<MarkerGpu, u8>(&expected.1)
            );
        }
    }
}

/// Rejecting only the merge key would publish a stale window, zoom, density, colour or history.
#[test]
fn completed_request_must_match_every_current_input() {
    let r = request();
    let visible = (0.0, 1000.0);
    assert!(r.matches(r, visible));
    assert!(
        r.matches(r, (500.0, 1500.0)),
        "pans inside the prefetched reach reuse exactly the same fold"
    );
    assert!(!r.matches(r, (-1001.0, 0.0)));
    assert!(!r.matches(r, (1000.0, 2001.0)));
    let variants = [
        Request {
            fold: FoldKey {
                merge_ms: 50,
                ..r.fold
            },
            ..r
        },
        Request {
            fold: FoldKey {
                small_pct: 90,
                ..r.fold
            },
            ..r
        },
        Request {
            fold: FoldKey {
                tick_bits: 0.1f64.to_bits(),
                ..r.fold
            },
            ..r
        },
        Request {
            layout: LayoutKey {
                time_to_px: 2.0,
                ..r.layout
            },
            ..r
        },
        Request {
            layout: LayoutKey {
                ppp: 2.0,
                ..r.layout
            },
            ..r
        },
        Request {
            layout: LayoutKey {
                colors: [[0.0; 3]; 2],
                ..r.layout
            },
            ..r
        },
        Request { epoch: 1.0, ..r },
        Request { generation: 1, ..r },
    ];
    for current in variants {
        assert!(!r.matches(current, visible), "stale identity: {current:?}");
    }
}

/// New data arriving during a job must remain dirty; readiness must wake a quiet market.
#[test]
fn pending_fold_keeps_previous_bubbles_and_publishes_on_quiet_market() {
    let r = request();
    let input = inputs(r);
    let now = Instant::now();
    let tape = samples();
    let old = prepare(r, Source::Tape(tape[..4].to_vec(), None));
    let replacement = prepare(r, Source::Tape(tape, None));
    let previous = old.discs.clone();
    let expected = replacement.discs.clone();
    assert_ne!(previous, expected);
    let mut pane = PaneRender::new();
    pane.cluster_on = true;
    pane.cluster_uploaded = true;
    pane.cluster_request = Some(r);
    pane.cluster_folded = Some(old.folded.clone());
    pane.cluster_discs = previous.clone();
    pane.cluster_built_at = Some(now);
    let (sender, receiver) = mpsc::channel();
    let ready = Arc::new(AtomicBool::new(false));
    pane.cluster_pending = Some(Pending {
        ready: ready.clone(),
        receiver,
    });
    assert!(!sync_clusters(&mut pane, &input, now));
    assert_eq!(
        pane.cluster_discs, previous,
        "the published fold stays visible while pending"
    );
    pane.cluster_dirty = true;
    assert!(
        !refold_due(&pane, now + Duration::from_secs(1)),
        "no second job while pending"
    );
    sender
        .send(Ok(replacement))
        .unwrap_or_else(|_| panic!("controlled result receiver dropped"));
    ready.store(true, Ordering::Release);
    assert!(
        refold_due(&pane, now),
        "completion wakes a market with no new revisions"
    );
    assert!(sync_clusters(&mut pane, &input, now));
    assert_eq!(pane.cluster_discs, expected);
    assert!(
        pane.cluster_dirty,
        "appends after the snapshot still need a later fold"
    );
    assert!(pane.cluster_pending.is_none());
    assert!(!refold_due(&pane, now));
    assert!(refold_due(&pane, now + Duration::from_millis(200)));
}

/// Disabling invalidates an in-flight request even when the reader re-enables identical settings.
#[test]
fn disabling_bubbles_invalidates_pending_publication() {
    let r = request();
    let now = Instant::now();
    let mut pane = PaneRender::new();
    pane.cluster_on = true;
    pane.cluster_uploaded = true;
    let (sender, receiver) = mpsc::channel();
    let ready = Arc::new(AtomicBool::new(false));
    pane.cluster_pending = Some(Pending {
        ready: ready.clone(),
        receiver,
    });
    let mut input = inputs(r);
    input.enabled = false;
    assert!(sync_clusters(&mut pane, &input, now));
    assert!(!pane.cluster_on);
    assert!(
        pane.cluster_pending.is_some(),
        "the job slot stays occupied until completion"
    );
    let current = Request {
        generation: pane.cluster_generation,
        ..r
    };
    assert!(!r.matches(current, (0.0, 1000.0)));
    sender
        .send(Ok(prepare(r, Source::Tape(samples(), None))))
        .unwrap_or_else(|_| panic!("controlled result receiver dropped"));
    ready.store(true, Ordering::Release);
    input.enabled = true;
    assert!(
        !sync_clusters(&mut pane, &input, now),
        "obsolete fold is never uploaded"
    );
    assert!(!pane.cluster_on);
    let pending = pane.cluster_pending.take().unwrap();
    let fresh = pending.receiver.recv().unwrap().unwrap();
    assert_eq!(fresh.request.generation, pane.cluster_generation);
}

/// Hysteresis must keep the old big scale but lower its small cutoff; relayout reuses the fold.
#[test]
fn background_refold_preserves_hysteresis_and_layout_reuse() {
    let r = request();
    let tape = samples();
    let first = prepare(r, Source::Tape(tape.clone(), None));
    let fresh = first.folded.scale.unwrap();
    let old = clusters::PrintScale {
        small: fresh.small * 1.05,
        big: fresh.big * 1.05,
    };
    let held = prepare(r, Source::Tape(tape, Some(old)));
    assert_eq!(
        held.folded.scale,
        Some(clusters::PrintScale {
            small: fresh.small,
            big: old.big
        })
    );
    let mut zoomed = r;
    zoomed.layout.time_to_px *= 0.5;
    let relayout = prepare(zoomed, Source::Fold(held.folded.clone()));
    assert!(
        Arc::ptr_eq(&relayout.folded, &held.folded),
        "layout-only work never recopies or refolds retained prints"
    );
}

/// A quiet chart must publish a completed fold through the real frame-driven market pull.
#[test]
fn quiet_chart_frame_publishes_a_completed_background_fold() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(quiet_chart_publication)
        .unwrap()
        .join()
        .unwrap();
}

/// Exercise ChartEngine's real quiet-source gate without a GPU, live feed or timed wait.
fn quiet_chart_publication() {
    use gpui::{Bounds, GpuFrameInfo, point, px, size};
    use moon_core::config::{ChartGraphicsCfg, ChartTheme, TradeKind};
    use moon_core::market::{MarketDataSource, MarketStore};
    let _window = crate::test_support::SharedWindowGuard::take();
    let mut engine = crate::chartdx::ChartEngine::new(0.0, ChartTheme::default());
    engine.open(42, "XALPHAUSDT");
    engine.set_historical(true);
    engine.set_follow(false, 1000.0);
    engine.set_chart_graphics(ChartGraphicsCfg {
        trade_kind: TradeKind::Bubbles,
        ..Default::default()
    });
    engine.show_time_range(0.0, 1000.0, 0.0);
    engine.set_scene_visible(true);
    engine.set_market_source(Some(MarketDataSource::new(MarketStore::shared(0.0))));
    let mut data = engine.data.borrow_mut();
    data.frame(GpuFrameInfo {
        now: Instant::now(),
        bounds: Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(1000.0), px(600.0)),
        },
        scale_factor: 1.0,
        content_zoom: 1.0,
        presentable: true,
    });
    let pending = data.render.borrow_mut().panes[0]
        .cluster_pending
        .take()
        .unwrap();
    let prepared = pending.receiver.recv().unwrap().unwrap();
    assert!(!data.view_dirty);
    assert!(
        !data.pull_market_source_if_visible(),
        "the unchanged source alone must stay idle"
    );
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Ok(prepared))
        .unwrap_or_else(|_| panic!("controlled result receiver dropped"));
    data.render.borrow_mut().panes[0].cluster_pending = Some(Pending {
        ready: Arc::new(AtomicBool::new(true)),
        receiver,
    });
    assert!(
        data.pull_market_source_if_visible(),
        "a completed job must wake the actual quiet-source gate"
    );
    let render = data.render.borrow();
    assert!(render.panes[0].cluster_on);
    assert!(render.panes[0].cluster_request.is_some());
    assert!(render.panes[0].cluster_pending.is_none());
}

/// Time the actual snapshot/enqueue and publication paths separately from background latency.
#[test]
#[ignore = "manual dev-profile heavy-user asynchronous bubble timing probe"]
fn bench_goal_b2_async_clusters_revision() {
    let tape: Vec<_> = (0..100_000)
        .map(|i| ChartCross {
            time_rel: i as f32 * 10.0,
            price: 10.0 + ((i * 73) % 1000) as f32 * 0.01,
            qty: 1.0 + (i % 37) as f32,
            side: (i % 2) as u32,
        })
        .collect();
    let mut input = inputs(request());
    input.view.bounds[2] = 10_000.0;
    input.view.time_to_px = 0.01;
    let mut r = request();
    r.layout.time_to_px = input.view.time_to_px;
    r.range = (-1_000_000.0, 2_000_000.0);
    let start = Instant::now();
    for _ in 0..20 {
        std::hint::black_box(synchronous(r, &tape));
    }
    println!(
        "goal_b2_ui_synchronous_pipeline: {} ns/iter",
        start.elapsed().as_nanos() / 20
    );
    let mut pane = PaneRender::new();
    pane.layers.set_combo_capacity(tape.len(), 1);
    pane.layers.reset_combo(tape);
    let mut copies = [Duration::ZERO; 2];
    for repetition in 0..30 {
        for arm in [repetition % 2, 1 - repetition % 2] {
            let start = Instant::now();
            let snapshot: Vec<_> = if arm == 0 {
                pane.layers
                    .tick_samples(r.range.0, r.range.1)
                    .copied()
                    .collect()
            } else {
                pane.layers.tick_snapshot(r.range.0, r.range.1)
            };
            assert_eq!(snapshot.len(), 100_000);
            std::hint::black_box(snapshot);
            copies[arm] += start.elapsed();
        }
    }
    println!(
        "goal_b2_snapshot_paired_ui_iterator: {} ns/iter",
        copies[0].as_nanos() / 30
    );
    println!(
        "goal_b2_snapshot_paired_optimized: {} ns/iter",
        copies[1].as_nanos() / 30
    );
    let (mut enqueue, mut publish, mut latency) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    for _ in 0..30 {
        pane.cluster_dirty = true;
        pane.cluster_built_at = None;
        let now = Instant::now();
        assert!(!sync_clusters(&mut pane, &input, now));
        enqueue += now.elapsed();
        let pending = pane.cluster_pending.take().unwrap();
        let prepared = pending.receiver.recv().unwrap().unwrap();
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Ok(prepared))
            .unwrap_or_else(|_| panic!("controlled result receiver dropped"));
        pane.cluster_pending = Some(Pending {
            ready: Arc::new(AtomicBool::new(true)),
            receiver,
        });
        let start = Instant::now();
        assert!(sync_clusters(&mut pane, &input, now));
        publish += start.elapsed();
        latency += now.elapsed();
        assert_eq!(pane.cluster_discs.len(), 100_000);
        std::hint::black_box(&pane.cluster_discs);
    }
    println!(
        "goal_b2_async_snapshot_enqueue: {} ns/iter",
        enqueue.as_nanos() / 30
    );
    println!(
        "goal_b2_async_publication: {} ns/iter",
        publish.as_nanos() / 30
    );
    println!(
        "goal_b2_async_end_to_end: {} ns/iter",
        latency.as_nanos() / 30
    );
}
