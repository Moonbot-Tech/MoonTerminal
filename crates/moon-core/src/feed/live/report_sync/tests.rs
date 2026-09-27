//! Report catch-up progress regression tests.

use super::*;

fn page(first_row: Option<i64>, last_rec_id: i64, max_rec_id: i64, rows: usize) -> PageFacts {
    PageFacts {
        first_row,
        last_rec_id,
        max_rec_id,
        rows,
        recreated: false,
    }
}

/// The span starts at the first delivered id, not at zero.
///
/// Breaks on: `feed/live/report_sync.rs:on_page` measuring from the request's `from_rec_id`. A
/// fresh download starts at 0 while the core's oldest retained row may be id 500 000, and the bar
/// would open at 50 % and sit there.
#[test]
fn a_fresh_download_starts_at_zero_percent() {
    let mut t = ReportSyncTracker::default();
    t.on_page(page(Some(500_001), 500_500, 501_000, 500), 10);
    let p = t.progress.unwrap();
    assert_eq!(p.first_rec_id, Some(500_001));
    // 500 of the 1 000 ids from the first delivered one to the high-water.
    assert_eq!(p.percent(), Some(50));
    assert_eq!(p.rows, 500);
    assert_eq!(
        p.started_ms, 10,
        "a page with no announcement starts the clock"
    );

    t.on_page(page(Some(500_501), 501_000, 501_000, 500), 20);
    let p = t.progress.unwrap();
    assert_eq!(p.percent(), Some(100));
    assert_eq!(p.rows, 1_000);
    assert_eq!(p.started_ms, 10);
}

/// An empty page moves nothing but the high-water.
///
/// Breaks on: `on_page` taking `last_rec_id` without its `> 0` guard — an empty page reports 0 and
/// would pin `first_rec_id` at 0, which is the 50 %-at-start defect again.
#[test]
fn an_empty_page_delivers_nothing() {
    let mut t = ReportSyncTracker::default();
    t.on_page(page(None, 0, 900, 0), 10);
    let p = t.progress.unwrap();
    assert_eq!(p.first_rec_id, None);
    assert_eq!(p.percent(), None);
}

/// A recreated database restarts the count but keeps the clock.
///
/// Breaks on: `on_page` treating a recreated page as progress. Its rows open the replacement
/// database from zero, and the ids delivered before it belong to the dead one.
#[test]
fn a_recreated_database_restarts_the_count() {
    let mut t = ReportSyncTracker::default();
    t.on_page(page(Some(1), 400, 1_000, 400), 10);
    t.on_page(
        PageFacts {
            recreated: true,
            ..page(Some(1), 50, 60, 50)
        },
        20,
    );
    let p = t.progress.unwrap();
    assert_eq!(p.first_rec_id, None);
    assert_eq!(p.rows, 0);
    assert_eq!(p.started_ms, 10);
}

/// Percent is clamped to the span and weighs a larger span more when summed.
#[test]
fn percent_stays_inside_the_span() {
    let p = ReportSyncProgress {
        first_rec_id: Some(10),
        last_rec_id: 50,
        max_rec_id: 20,
        rows: 0,
        started_ms: 0,
    };
    assert_eq!(p.done_and_span(), Some((11, 11)));
    assert_eq!(p.percent(), Some(100));
}
