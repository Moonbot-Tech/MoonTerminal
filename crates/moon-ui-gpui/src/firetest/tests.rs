//! Contract tests for the shape of a FireTest run.
//!
//! They guard one decision: `chart-smoke` is ONE ordered scenario, not a bag of side tests. The
//! expected stage names are written out by hand on purpose — deriving them from the table would
//! compare the table with itself and catch nothing. This list is the oracle, and it is the same
//! list `docs/FIRETEST.md` shows the developer.

use super::config::{Config, Script};
use super::plan::{CHART_SMOKE, DONE_STAGE_NAME, ORDER_CANCEL_LAG_PLAN, Phase, StageDef, plan_for};

/// Every stage line a run of `plan` writes, in order — its rows, then the closing line. The
/// closing name is included because it is the one `stage=` line no row owns, so nothing else
/// would pin it.
fn names(plan: &'static [StageDef]) -> Vec<&'static str> {
    plan.iter()
        .map(|stage| stage.name)
        .chain(std::iter::once(DONE_STAGE_NAME))
        .collect()
}

#[test]
fn chart_smoke_runs_every_phase() {
    // Deliberately `chart-smoke` alone, not the union of both tables: a check that only ever runs
    // under the narrow order script is a side test, which is exactly what the general run exists
    // to prevent. `order-cancel-lag` may only reuse phases chart-smoke already has.
    let placed: Vec<Phase> = CHART_SMOKE.iter().map(|stage| stage.phase).collect();
    let mut unique: Vec<usize> = placed.iter().map(|phase| *phase as usize).collect();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        Phase::StageCount as usize,
        "every FireTest phase must appear in chart-smoke; a phase placed anywhere else is a side \
         test the general run never exercises"
    );
    assert!(
        !placed.contains(&Phase::StageCount),
        "the phase-count sentinel is not a runtime stage"
    );
}

#[test]
fn no_script_repeats_a_phase() {
    for plan in [CHART_SMOKE, ORDER_CANCEL_LAG_PLAN] {
        let mut seen: Vec<usize> = plan.iter().map(|stage| stage.phase as usize).collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            total,
            "a script must not run the same phase twice: its samples would merge into one bucket"
        );
    }
}

#[test]
fn chart_smoke_stage_plan_is_one_contiguous_scenario() {
    assert_eq!(
        names(CHART_SMOKE),
        vec![
            "start",
            "open_chart",
            "wait_chart_probe",
            "settle_live_chart",
            "idle_floor",
            "baseline",
            "mouse_storm",
            "static_text_gap",
            "static_text_warmup",
            "static_text_storm",
            "command_error_contract",
            "tool_windows_open",
            "tool_windows_verify_open",
            "tool_windows_dedup",
            "tool_windows_verify_dedup",
            "root_overlay_contract",
            "locale_switch",
            "locale_switch_verify",
            "price_scale_50",
            "price_scale_20",
            "price_scale_auto",
            "price_scale_verify_auto",
            "order_cancel_lag",
            "cooldown",
            "result",
        ],
        "chart-smoke must remain one ordered run; do not add side tests outside this stage plan"
    );
}

#[test]
fn order_cancel_lag_script_is_a_narrow_order_only_run() {
    let config = Config::from_args([
        "moonterminal".to_string(),
        "--debug-script".to_string(),
        "order-cancel-lag".to_string(),
    ])
    .expect("order-cancel-lag args must parse")
    .expect("order-cancel-lag must create FireTest config");

    assert_eq!(config.script, Script::OrderCancelLag);
    assert!(
        config.order_cancel_lag,
        "order-cancel-lag must enable the real order-lag stage itself"
    );
    assert_eq!(
        names(plan_for(config.script)),
        vec![
            "start",
            "open_chart",
            "wait_chart_probe",
            "settle_live_chart",
            "order_cancel_lag",
            "cooldown",
            "result",
        ],
        "order-cancel-lag must remain a focused order-path diagnostic"
    );
}

#[test]
fn the_storms_and_their_baseline_are_all_equally_hot() {
    // The comparison the verdict is built on only means something if the storms and the baseline
    // they are measured against ran in the SAME present mode. `idle_floor` is scored too, but by
    // absolute ceilings rather than by a delta, and it is cold BY DESIGN — so it is named here as
    // an exception rather than left to look like an oversight.
    for stage in CHART_SMOKE {
        match stage.phase {
            Phase::Baseline | Phase::Storm | Phase::StaticTextStorm => assert!(
                stage.present_pressure,
                "{} is compared against its baseline, so it must run under forced present \
                 pressure: comparing a storm against a chart that was not equally hot measures \
                 the market, not the cursor",
                stage.name
            ),
            Phase::IdleFloor => assert!(
                !stage.present_pressure,
                "the idle floor exists to measure the app when nothing forces it to work"
            ),
            _ => {}
        }
    }
}

#[test]
fn the_columns_match_the_run_they_replaced() {
    // Hand-counted from the dispatcher this table replaced, not read back off the table: these are
    // the stages that used to set present pressure at the top of their arm, the seven the old
    // `observe_probe` matched, the one baseline warm-up carve-out, and the three arms that had a
    // deadline. A row that silently loses a `.hot()` or a `.probes()` still runs and still logs the
    // same line — this is the only thing that would notice.
    let hot: Vec<&str> = CHART_SMOKE
        .iter()
        .filter(|stage| stage.present_pressure)
        .map(|stage| stage.name)
        .collect();
    assert_eq!(
        hot,
        vec![
            "settle_live_chart",
            "baseline",
            "mouse_storm",
            "static_text_warmup",
            "static_text_storm",
        ],
        "the idle floor must NOT appear here: it is the one measured stage that runs cold"
    );

    let probing: Vec<&str> = CHART_SMOKE
        .iter()
        .filter(|stage| stage.wants_probe)
        .map(|stage| stage.name)
        .collect();
    assert_eq!(
        probing,
        vec![
            "wait_chart_probe",
            "settle_live_chart",
            "baseline",
            // `idle_floor` sits between settle and baseline and is deliberately absent: it aims
            // no storm, so it has no reason to accept a fresh probe.
            "mouse_storm",
            "static_text_gap",
            "static_text_warmup",
            "static_text_storm",
        ]
    );

    let warming: Vec<&str> = CHART_SMOKE
        .iter()
        .filter(|stage| !stage.sample_warmup.is_zero())
        .map(|stage| stage.name)
        .collect();
    assert_eq!(warming, vec!["idle_floor", "baseline"]);

    // The reason too, not just which rows carry one: swapping two deadlines' messages would send
    // the developer looking at the wrong stage.
    let deadlined: Vec<(&str, &str)> = CHART_SMOKE
        .iter()
        .filter_map(|stage| stage.timeout.map(|(_, reason)| (stage.name, reason)))
        .collect();
    assert_eq!(
        deadlined,
        vec![
            ("open_chart", "no active visible core/window to open chart"),
            (
                "wait_chart_probe",
                "chart opened but no chart bounds probe arrived"
            ),
            ("order_cancel_lag", "order_cancel_lag timed out"),
        ]
    );
}

#[test]
fn the_narrow_script_reuses_chart_smokes_rows_rather_than_copying_them() {
    // The lists above only inspect `chart-smoke`. This is what keeps that from being a hole: every
    // row of the narrow script must be the same stage as chart-smoke's, down to each column — so a
    // copy that drifted in its name, its present mode or its deadline fails here instead of
    // quietly measuring something else under a familiar log line.
    for stage in ORDER_CANCEL_LAG_PLAN {
        let twin = CHART_SMOKE
            .iter()
            .find(|other| other.phase == stage.phase)
            .unwrap_or_else(|| panic!("{} is not a stage chart-smoke runs", stage.name));
        assert_eq!(stage.name, twin.name, "{:?} renamed", stage.phase);
        assert_eq!(stage.min_dwell, twin.min_dwell, "{:?} dwell", stage.phase);
        assert_eq!(stage.timeout, twin.timeout, "{:?} deadline", stage.phase);
        assert_eq!(
            stage.present_pressure, twin.present_pressure,
            "{:?} present mode",
            stage.phase
        );
        assert_eq!(
            stage.wants_probe, twin.wants_probe,
            "{:?} probe window",
            stage.phase
        );
        assert_eq!(
            stage.sample_warmup, twin.sample_warmup,
            "{:?} sample warmup",
            stage.phase
        );
    }
}

#[test]
fn a_stage_that_can_wait_forever_declares_a_timeout() {
    // A zero `min_dwell` is the shape of a stage that polls the live app every tick — the shape
    // that hangs when it never gets what it waits for. Those must carry a deadline; the two storms
    // are the exception, bounded by `config.storm` inside `await_storm`.
    //
    // The guard's limit, stated rather than hidden: it cannot see a DWELLING row whose `run`
    // answers `Stay` forever, because "can this function wait" is not in the table. Today none
    // does — every dwelling row answers `Next` or `Fail` on its first acting tick.
    for stage in CHART_SMOKE.iter().chain(ORDER_CANCEL_LAG_PLAN.iter()) {
        if stage.min_dwell.is_zero() && stage.timeout.is_none() {
            assert!(
                matches!(stage.phase, Phase::Storm | Phase::StaticTextStorm),
                "{} polls the app with neither a dwell nor a deadline; a stuck run would hang \
                 instead of going red",
                stage.name
            );
        }
    }
}
