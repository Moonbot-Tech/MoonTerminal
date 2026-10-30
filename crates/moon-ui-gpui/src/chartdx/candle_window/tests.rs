use super::*;

fn row(t: f32, tf: f32) -> CandleGpu {
    CandleGpu {
        t_open_rel: t,
        open: 1.0,
        high: 1.0,
        low: 1.0,
        close: 1.0,
        volume: 1.0,
        tf_rel: tf,
    }
}

fn series(from: usize, to: usize) -> Vec<CandleGpu> {
    (from..to).map(|i| row(i as f32, 1.0)).collect()
}

/// Breakage: `chartdx/candle_window.rs` `CandleWindow::draw_slice` starts at the first visible row
/// instead of one before it. Consequence: the volume hill into the first visible candle vanishes
/// at the left edge. Numbers: 2880 rows, a 100-candle view submitted 2880 candles + 2879 hills
/// before, 101 + 101 now.
#[test]
fn draw_slice_submits_only_the_visible_rows_plus_the_left_neighbour() {
    let mut w = CandleWindow::new(4096);
    w.set(&series(0, 2880));
    // Rows 1000..=1099 intersect the view; the hill into row 1000 is instance 999's.
    let s = w.draw_slice(1000.5, 1099.5);
    assert_eq!(
        s,
        CandleDrawSlice {
            start: 999,
            candles: 101,
            hills: 101,
        }
    );
}

/// A wide filler that starts before the view but ends inside it is drawn.
#[test]
fn draw_slice_keeps_a_wide_filler_reaching_into_the_view() {
    let mut full = series(0, 900);
    full.push(row(900.0, 300.0));
    full.extend(series(1000, 2000));
    let mut w = CandleWindow::new(4096);
    w.set(&full);
    let s = w.draw_slice(1150.0, 1160.0);
    // The filler (index 900) spans [900, 1200) and covers the view.
    assert_eq!(s.start, 899);
    assert!(s.start as usize + s.candles as usize > 900);
}

/// Breakage: `chartdx/candle_window.rs` `CandleWindow::patch` owes a full upload for a tail patch.
/// Consequence: every live batch rewrites the whole 4096-instance GPU buffer. Numbers: slots
/// written per same-bucket batch were min(N, 4096) before, 1 now.
#[test]
fn patching_the_tail_uploads_only_the_changed_slots() {
    let mut w = CandleWindow::new(4096);
    let mut full = series(0, 100);
    w.set(&full);
    assert_eq!(w.take_upload().0, CandleUpload::Full);
    full[99].close = 2.0;
    w.patch(99, &full);
    assert_eq!(w.take_upload().0, CandleUpload::Range { first: 99, len: 1 });

    let mut full = series(0, 5000);
    w.set(&full);
    let (_, _, dropped) = w.take_upload();
    assert_eq!(dropped, 5000 - 4096);
    full[4999].close = 2.0;
    w.patch(4999, &full);
    assert_eq!(
        w.take_upload().0,
        CandleUpload::Range {
            first: 4095,
            len: 1
        }
    );

    full.push(row(5000.0, 1.0));
    w.patch(5000, &full);
    let (upload, _, dropped) = w.take_upload();
    assert_eq!(upload, CandleUpload::Full, "the window moved");
    assert_eq!(dropped, 5001 - 4096);
}

/// Breakage: an uncapped window (wgpu and Metal keep every row) treats a tail patch as a full
/// replace. Consequence: a live trade copies and uploads the whole history. Numbers: a 100_000-row
/// series owes a 1-slot upload, not 100_000.
#[test]
fn uncapped_tail_patch_uploads_only_the_new_rows() {
    let mut w = CandleWindow::new(usize::MAX);
    let mut full = series(0, 100_000);
    w.set(&full);
    assert_eq!(w.take_upload().0, CandleUpload::Full);
    assert_eq!(w.len(), 100_000);
    full[99_999].close = 3.0;
    full.push(row(100_000.0, 1.0));
    w.patch(99_999, &full);
    assert_eq!(w.len(), 100_001);
    assert_eq!(
        w.take_upload().0,
        CandleUpload::Range {
            first: 99_999,
            len: 2
        }
    );
    assert_eq!(w.len(), full.len());
    assert!(!w.is_empty());
}

/// Breakage: the Linux dispatch still replaces the candle buffer, or wgpu has no tail patch.
/// Consequence: every live candle copies the whole series on Linux while DX11 and Metal do not.
#[test]
fn tail_patch_is_wired_on_every_backend() {
    let backend = include_str!("../backend.rs");
    assert!(backend.contains("self.wgpu.patch_candles(from, full)"));
    assert!(backend.contains("self.metal.patch_candles(from, full)"));
    assert!(backend.contains("self.wgpu.patch_path_candles(from, full)"));
    assert!(backend.contains("self.metal.patch_path_candles(from, full)"));
    let wgpu = include_str!("../wgpu_backend/layers.rs");
    assert!(wgpu.contains("self.candles.patch(from, full)"));
    assert!(wgpu.contains("self.path_candles.patch(from, full)"));
    assert!(wgpu.contains("CandleWindow::new(usize::MAX)"));
    let metal = include_str!("../metal_backend.rs");
    assert!(metal.contains("self.candles.patch(from, full)"));
    assert!(metal.contains("self.path_candles.patch(from, full)"));
}

/// Scale probe for a live tail against a full replace of 100_000 candles.
/// Ignored: it allocates the series repeatedly and prints timings.
#[test]
#[ignore = "scale probe: full candle replace vs tail patch"]
fn tail_patch_scale_probe() {
    const N: usize = 100_000;
    const RUNS: usize = 11;
    let mut full = series(0, N);
    let mut warm = Vec::new();
    warm.extend_from_slice(&full);
    let mut w = CandleWindow::new(usize::MAX);
    w.set(&full);
    let _ = w.take_upload();
    let mut replace_ms = Vec::with_capacity(RUNS);
    let mut patch_ms = Vec::with_capacity(RUNS);
    for i in 0..RUNS {
        let start = std::time::Instant::now();
        warm.clear();
        warm.extend_from_slice(&full);
        std::hint::black_box(&warm);
        replace_ms.push(start.elapsed().as_secs_f64() * 1_000.0);

        full[N - 1].close = 1.0 + i as f32;
        let start = std::time::Instant::now();
        w.patch(N - 1, &full);
        let upload = w.take_upload().0;
        std::hint::black_box(upload);
        patch_ms.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    replace_ms.sort_by(|a, b| a.total_cmp(b));
    patch_ms.sort_by(|a, b| a.total_cmp(b));
    println!(
        "tail_patch_scale full_replace_ms {:.3} tail_patch_ms {:.3}",
        replace_ms[RUNS / 2],
        patch_ms[RUNS / 2]
    );
}
