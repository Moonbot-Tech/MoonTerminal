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

/// Breakage: `candles/window.rs` `CandleWindow::draw_slice` starts at the first visible row
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

/// Breakage: `candles/window.rs` `CandleWindow::patch` owes a full upload for a tail patch.
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
