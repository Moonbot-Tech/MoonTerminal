//! Pure save-admission proofs and shutdown-ownership guards.

use std::time::{Duration, Instant};

use super::{SAVE_INTERVAL, SaveGate};

/// Moving a dirty clear outside admission loses trailing edits before either saver can write.
#[test]
fn persistence_clears_dirty_flags_only_after_admission() {
    let source: String = include_str!("../boot.rs")
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(source.contains("ifb.chart_specs_dirty{letadmitted=chart_gate.admit(now);ifadmitted{chart_persist::save_all(&b.chart_specs);b.chart_specs_dirty=false;}"));
    assert!(source.contains("ifb.config_dirty&&config_gate.admit(now){"));
    let persistence = braced_source(&source, "ifb.persist_allowed");
    let chart = braced_source(persistence, "ifb.chart_specs_dirty");
    let admitted = braced_source(chart, "ifadmitted");
    assert!(admitted.contains("b.chart_specs_dirty=false;"));
    assert_eq!(chart.matches("b.chart_specs_dirty=false;").count(), 1);
    let config = braced_source(persistence, "ifb.config_dirty&&config_gate.admit(now)");
    assert!(config.contains("b.config_dirty=false;"));
    assert!(persistence.contains("chart_gate.admit(now)"));
    assert!(persistence.contains("config_gate.admit(now)"));
}

/// Isolate the balanced admission block so a later unrelated clear cannot satisfy its assertion.
fn braced_source<'a>(source: &'a str, marker: &str) -> &'a str {
    let tail = &source[source.find(marker).unwrap() + marker.len()..];
    let start = tail.find('{').unwrap();
    let mut depth = 0;
    for (offset, ch) in tail[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &tail[start + 1..start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced source block")
}

/// Delaying the first admission would leave an isolated edit waiting unnecessarily.
#[test]
fn save_gate_writes_the_first_edit_immediately() {
    let mut gate = SaveGate::new(Duration::from_secs(1));
    assert!(gate.admit(Instant::now()));
}

/// Removing the one-second window would restore ten disk-write admissions per second.
#[test]
fn save_gate_admits_one_write_per_second_under_a_10hz_dirty_stream() {
    let start = Instant::now();
    let mut gate = SaveGate::new(SAVE_INTERVAL);
    let writes = (0..30)
        .filter(|tick| gate.admit(start + Duration::from_millis(tick * 100)))
        .count();
    assert_eq!(writes, 3);
}

/// Clearing a rejected dirty edit would lose the last edit in a burst.
#[test]
fn save_gate_trailing_edit_is_written_after_the_window() {
    let start = Instant::now();
    let mut gate = SaveGate::new(Duration::from_secs(1));
    assert!(gate.admit(start));
    let mut dirty = true;
    for ms in [100, 999, 1_000] {
        if dirty && gate.admit(start + Duration::from_millis(ms)) {
            dirty = false;
        }
        assert_eq!(dirty, ms < 1_000);
    }
    assert!(
        gate.admit(start + Duration::from_secs(3)),
        "an edit after idle writes immediately"
    );
}

/// Measures admissions over synthetic dirty streams without touching storage.
#[test]
#[ignore]
fn bench_save_gate_10hz_dirty_stream() {
    let start = Instant::now();
    let began = Instant::now();
    let iterations = 100_000;
    let mut writes = 0;
    for _ in 0..iterations {
        let mut gate = SaveGate::new(std::hint::black_box(SAVE_INTERVAL));
        for tick in 0..30 {
            writes += usize::from(std::hint::black_box(
                gate.admit(start + Duration::from_millis(tick * 100)),
            ));
        }
    }
    println!(
        "bench_save_gate_10hz_dirty_stream: {} ns/iter, {} writes/30 ticks",
        began.elapsed().as_nanos() / iterations,
        writes / iterations as usize
    );
    assert_eq!(writes / iterations as usize, 3);
}

/// Restoring the old `return (Vec::new(), true)` shutdown branch in `boot.rs:on_window_closed` must
/// fail. Detached panel windows are OS-owned children of the group window: when the LAST one closes
/// they die with the application, and a release that still finds itself in
/// `Backend::detached_panel_windows` queues a repin. That repin is indistinguishable from the user
/// closing the window by hand — it docks the panel and deletes its `DetachedSpec` — so the final
/// save persists every panel docked and the next launch opens with the detachment gone. The branch
/// that closes one of SEVERAL group windows already unregisters them for exactly this reason.
#[test]
fn the_last_group_window_unregisters_detached_panels_before_quitting() {
    let source = include_str!("../boot.rs");
    let branch = source
        .split("if last_group_window {")
        .nth(1)
        .and_then(|tail| tail.split("// Otherwise close detached charts").next())
        .expect("the shutdown branch of on_window_closed must exist");

    let quitting = branch
        .find("b.quitting = true;")
        .expect("the shutdown branch must mark the exit before any window is released");
    let taken = branch
        .find("detached::take_windows(b, |_| true)")
        .expect("the shutdown branch must unregister every detached panel window");
    let pruned = branch
        .find("detached::prune_requests(b, |_| true)")
        .expect("the shutdown branch must drop queued repin and detach requests");
    let returned = branch
        .find("return (")
        .expect("the shutdown branch must return the quit decision");

    assert!(quitting < taken && taken < pruned && pruned < returned);
    assert!(
        !branch.contains("return (Vec::new(), true)"),
        "the taken handles must be closed, not discarded"
    );
}
