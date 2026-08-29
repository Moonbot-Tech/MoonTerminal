//! Publication of a parsed configuration into the atomics every call site reads.
//!
//! These are the only tests in the crate that touch this module's globals, so they are written as
//! one sequence rather than several parallel cases: two tests storing different values into the
//! same statics would race each other rather than the code.

use super::*;

#[test]
fn a_file_that_cannot_be_read_is_never_written_over() {
    // The case that costs a user their configuration: an existing file that `read_to_string`
    // rejects — locked by an editor, denied by permissions, or saved as UTF-16 by Notepad. Folding
    // it into "absent" makes `ensure_file` render the template straight over their settings.
    let state = FileState::Unusable("saved as UTF-16".to_string());
    assert!(
        plan_write(&state, None).is_none(),
        "an unreadable file must be left exactly as it is"
    );
    assert!(
        plan_write(&state, Some("[log]\nbalances = true\n")).is_none(),
        "not even with text in hand — we could not parse it, so we do not own it"
    );
}

#[test]
fn an_absent_file_is_created_and_a_complete_one_is_left_alone() {
    let (text, _) = plan_write(&FileState::Absent, None).expect("a first launch writes the file");
    assert!(text.contains("[channels]"), "the template must be written");

    let complete = template::render();
    assert!(
        plan_write(&FileState::Parsed(DiagCfg::default()), Some(&complete)).is_none(),
        "an already-complete file must not be rewritten; that would only churn its mtime"
    );
}

#[test]
fn the_early_and_late_paths_point_at_the_same_file() {
    // The early read happens before the logger exists and must not create directories; the write
    // happens later and may. If the two ever diverged, the terminal would read one file and write
    // another, and every edit would look ignored.
    assert_eq!(
        paths::diagnostics_path_no_create(),
        paths::diagnostics_path()
    );
}

#[test]
fn applying_a_config_publishes_and_retracts_every_switch() {
    let mut cfg = DiagCfg::default();
    cfg.channels.render = true;
    cfg.channels.detect = true;
    cfg.channels.assets = true;
    cfg.channels.markets = true;
    cfg.channels.hl_limit = true;
    cfg.channels.orders = "GateF/BTC".to_string();
    cfg.limits.log_ring_lines = 12_345;
    cfg.limits.balance_repeat_window_sec = 9;
    cfg.limits.market_trace_min_interval_ms = 250;
    apply(&cfg);

    assert!(render() && detect() && assets() && markets() && hl_limit() && orders());
    assert_eq!(
        with_orders_selector(|s| s.to_string()).as_deref(),
        Some("GateF/BTC")
    );
    assert_eq!(ring_lines(), 12_345);
    assert_eq!(balance_repeat_window(), Duration::from_secs(9));
    assert_eq!(market_trace_min_interval(), Duration::from_millis(250));

    // Retraction matters as much as publication: a file edited back to `false` has to switch the
    // channel off, which a one-way `force_enable`-style flag could not do.
    apply(&DiagCfg::default());
    assert!(!render() && !detect() && !assets() && !markets() && !hl_limit() && !orders());
    assert!(
        with_orders_selector(|_| ()).is_none(),
        "an off channel must not hand out a selector at all, so callers cannot accidentally follow \
         a stale target"
    );

    // Whitespace is what a user leaves behind when clearing the value by hand; treating it as a
    // live selector would follow every market on every core.
    let mut blank = DiagCfg::default();
    blank.channels.orders = "   ".to_string();
    apply(&blank);
    let blank_is_off = !orders();
    apply(&DiagCfg::default());
    assert!(blank_is_off);
}
