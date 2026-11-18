use super::*;

const THRESHOLD: Duration = Duration::from_millis(16);

/// Close a guard at a synthetic time to exercise exact duration boundaries.
fn close(mut guard: StallSpan, end: Instant) {
    guard.active.take().unwrap().finish(end, THRESHOLD);
}

/// Measure a completed operation without changing the process-wide diagnostics config.
fn capture(start: Instant, duration_ms: u64, kind: StallKind, name: &'static str) {
    close(
        start_span(kind, name, true, || start),
        start + Duration::from_millis(duration_ms),
    );
}

/// Catch both an eager clock read and a public guard recording while off by default.
#[test]
fn disabled_span_skips_clock_and_ring() {
    assert!(!enabled());
    drop(span(StallKind::Open, "disabled"));
    drop(
        start_span(StallKind::Render, "disabled", false, || panic!("disabled clock read"))
            .with_window(|| panic!("disabled window lookup")),
    );
    assert!(drain().is_empty());
    DEPTH.with(|depth| assert_eq!(depth.get(), 0));
}

/// The exact cutoff must be inclusive and a closed child must retain its original depth.
#[test]
fn span_cutoff_and_nesting_are_exact() {
    let now = Instant::now();
    let outer = start_span(StallKind::Open, "settings", true, || now);
    let inner_start = now + Duration::from_millis(1);
    capture(inner_start, 15, StallKind::Observe, "fast");
    capture(inner_start, 16, StallKind::Close, "child");
    close(outer, now + Duration::from_millis(22));
    let drained = drain();
    assert_eq!(
        format(&drained, now, SystemTime::UNIX_EPOCH),
        [
            "ts=1 kind=close name=child ms=16.0 depth=1",
            "ts=0 kind=open name=settings ms=22.0 depth=0",
        ]
    );
    assert!(drain().is_empty());
    DEPTH.with(|depth| assert_eq!(depth.get(), 0));
}

/// A writer lagging behind capture must keep recent entries and account for every eviction.
#[test]
fn capture_and_pending_batches_drop_oldest_at_capacity() {
    let now = Instant::now();
    capture(now, 16, StallKind::Open, "oldest");
    for _ in 0..512 {
        capture(now, 16, StallKind::Activate, "queued");
    }
    let mut batch = drain();
    assert_eq!(batch.records.len(), 512);
    assert_eq!(batch.dropped, 1);
    capture(now, 17, StallKind::Resize, "newest");
    batch.merge(drain());
    let lines = format(&batch, now, SystemTime::UNIX_EPOCH);
    assert_eq!(lines.len(), 513);
    assert!(lines.iter().all(|line| !line.contains("oldest")));
    assert!(lines[511].contains("name=newest"));
    assert_eq!(lines[512], "dropped=2");
}

/// Retain at most 256 short contributors and reset the attribution window on drain.
#[test]
fn recent_ring_is_bounded_and_cleared_by_drain() {
    let now = Instant::now();
    capture(now, 8, StallKind::Render, "evicted");
    for _ in 0..256 {
        capture(now, 1, StallKind::Observe, "kept");
    }
    CAPTURE.with_borrow(|capture| assert_eq!(capture.recent.len(), 256));
    let frame = FrameSample {
        window_id: 4,
        draw_start: now,
        draw_end: now + THRESHOLD,
    };
    record_frames([frame], THRESHOLD);
    let lines = format(&drain(), now, SystemTime::UNIX_EPOCH);
    assert!(lines[0].contains("in=[observe:kept:1.0,"));
    assert_eq!(lines[1], "dropped=1");
    record_frames([frame], THRESHOLD);
    let lines = format(&drain(), now, SystemTime::UNIX_EPOCH);
    assert_eq!(
        lines,
        ["ts=0 kind=frame name=window:4 ms=16.0 in=[unattributed]"]
    );
}

/// A slow window draw keeps only its four longest overlaps, even when all labels are short.
#[test]
fn frames_use_the_draw_interval_and_rank_overlaps() {
    let now = Instant::now();
    let at = |ms| now + Duration::from_millis(ms);
    capture(at(0), 10, StallKind::Render, "touches_start");
    capture(at(2), 10, StallKind::Observe, "partial");
    capture(at(11), 4, StallKind::Input, "fourth");
    capture(at(12), 5, StallKind::Observe, "third");
    capture(at(13), 7, StallKind::Render, "first");
    capture(at(14), 6, StallKind::ChartFrame, "second");
    capture(at(26), 6, StallKind::Render, "touches_end");
    capture(at(10), 14, StallKind::Activate, "not_a_contributor");
    close(
        start_span(StallKind::Render, "other_window", true, || at(10)).with_window(|| 4),
        at(25),
    );
    record_frames(
        [
            FrameSample {
                window_id: 3,
                draw_start: at(10),
                draw_end: at(26),
            },
            FrameSample {
                window_id: 4,
                draw_start: at(40),
                draw_end: at(65),
            },
            FrameSample {
                window_id: 5,
                draw_start: at(70),
                draw_end: at(85),
            },
        ],
        THRESHOLD,
    );
    assert_eq!(
        format(&drain(), at(90), SystemTime::UNIX_EPOCH + Duration::from_secs(30)),
        [
            "ts=29920 kind=frame name=window:3 ms=16.0 in=[render:first:7.0,chartframe:second:6.0,observe:third:5.0,input:fourth:4.0]",
            "ts=29950 kind=frame name=window:4 ms=25.0 in=[unattributed]",
        ]
    );
}

/// A label must stay within a single ASCII field and timestamps must use the span start.
#[test]
fn format_escapes_labels_and_uses_record_start() {
    let start = Instant::now();
    capture(start, 37, StallKind::Open, "setup\nколонка");
    let lines = format(
        &drain(),
        start + Duration::from_millis(240),
        SystemTime::UNIX_EPOCH + Duration::from_secs(60),
    );
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert!(line.is_ascii());
    assert!(!line.contains('\n'));
    assert!(line.starts_with("ts=59760 kind=open name=setup\\u{a}"));
    assert!(line.ends_with(" ms=37.0 depth=0"));
}

/// Pinned GPUI resets the trace cursor on disable, so a fresh collector sees the first new draw.
#[test]
fn gpui_collector_sees_first_draw_after_trace_is_reenabled() {
    let now = Instant::now();
    let frame = gpui::FrameTiming {
        window_id: gpui::WindowId::from(3),
        dirty_at: None,
        invalidations: 0,
        draw_start: now,
        draw_end: now + THRESHOLD,
    };
    gpui::set_frame_trace_enabled(true);
    let mut old = gpui::FrameTimingCollector::new();
    gpui::record_frame_timing(frame);
    let before = old.collect_unseen();
    gpui::set_frame_trace_enabled(false);
    gpui::set_frame_trace_enabled(true);
    let mut fresh = gpui::FrameTimingCollector::new();
    gpui::record_frame_timing(frame);
    let stale = old.collect_unseen();
    let after = fresh.collect_unseen();
    gpui::set_frame_trace_enabled(false);

    assert_eq!(before.len(), 1);
    assert!(stale.is_empty());
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].window_id, frame.window_id);
}

/// A stale collector after an off/on cycle would skip frames at the start of the new trace.
#[test]
fn boot_folds_frames_before_draining_and_recreates_collector_on_enable() {
    let boot = include_str!("../../startup/boot.rs");
    let (_, after_transition) = boot.split_once("&& trace_wanted").unwrap();
    let (enable_branch, after_collect) = after_transition.split_once("if trace_wanted {").unwrap();
    assert!(enable_branch.contains("frame_timings = gpui::FrameTimingCollector::new()"));
    let (fold, flush) = after_collect.split_once("stall::drain()").unwrap();
    assert!(fold.contains("stall::note_frames("));
    assert!(flush.contains("stall_writer.submit(&executor, drained)"));
}
