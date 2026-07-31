//! Turning the collected samples into PASS or FAIL.
//!
//! Two shapes of threshold live here and they mean different things. An absolute ceiling
//! (`chart_render`, `cpu_process_max`) says "this must never be that high at all". A `*_delta`
//! ceiling says "the storm must not add this much OVER the already-hot baseline" — that is the one
//! that survives a live market, because the baseline phase already paid for the feed's own work.
//! Every failed check is reported, not just the first: one run should hand back the whole list.

use crate::firetest::Runtime;
use crate::firetest::logging::{firetest_error, firetest_info};
use crate::firetest::plan::Phase;
use crate::firetest::sample::PhaseStats;

impl Runtime {
    /// Score the run and exit the process: `0` for PASS, `2` for FAIL.
    ///
    /// The narrow order script has no perf samples to score — reaching this point at all means its
    /// one stage already passed.
    pub(super) fn evaluate_and_exit(&mut self) {
        self.set_phase(Phase::Done);
        if self.config.is_order_cancel_script() {
            firetest_info("[firetest] result=PASS FIRETEST PASS order_cancel_lag");
            std::process::exit(0);
        }

        let baseline = PhaseStats::of(&self.samples, Phase::Baseline);
        let clean_storm = PhaseStats::of(&self.samples, Phase::Storm);
        let static_text_storm = PhaseStats::of(&self.samples, Phase::StaticTextStorm);
        let storm = PhaseStats::joined(&clean_storm, &static_text_storm);
        if storm.is_empty() {
            self.fail("no storm diag samples");
            return;
        }
        if clean_storm.is_empty() {
            self.fail("no clean mouse storm diag samples");
            return;
        }
        if static_text_storm.is_empty() {
            self.fail("no static text storm diag samples");
            return;
        }

        let avg_rate = |label: &str| storm.avg_rate(label);
        let max_rate = |label: &str| storm.max_rate(label);
        let static_text_avg_rate = |label: &str| static_text_storm.avg_rate(label);
        let static_text_max_rate = |label: &str| static_text_storm.max_rate(label);
        // The storm's average above the baseline's PEAK: the one comparison a live feed cannot make
        // red on its own.
        let rate_delta = |label: &str| (storm.avg_rate(label) - baseline.max_rate(label)).max(0.0);

        let baseline_cpu = baseline.avg_cpu();
        let avg_cpu = storm.avg_cpu();
        let max_cpu = storm.max_cpu();
        let cpu_delta = (avg_cpu - baseline_cpu).max(0.0);
        let static_text_avg_cpu = static_text_storm.avg_cpu();
        let static_text_max_cpu = static_text_storm.max_cpu();
        let static_text_cpu_delta = (static_text_avg_cpu - baseline_cpu).max(0.0);

        let baseline_gpu_process = baseline.avg_gpu_process();
        let avg_gpu_process = storm.avg_gpu_process();
        let max_gpu_process = storm.max_gpu_process();
        let gpu_process_delta = (avg_gpu_process - baseline_gpu_process).max(0.0);
        let static_text_avg_gpu_process = static_text_storm.avg_gpu_process();
        let static_text_max_gpu_process = static_text_storm.max_gpu_process();
        let static_text_gpu_process_delta =
            (static_text_avg_gpu_process - baseline_gpu_process).max(0.0);

        let avg_gpu_frame_ms = storm.avg_gpu_frame_ms();
        let max_gpu_frame_ms = storm.max_gpu_frame_ms();
        let static_text_avg_gpu_frame_ms = static_text_storm.avg_gpu_frame_ms();
        let static_text_max_gpu_frame_ms = static_text_storm.max_gpu_frame_ms();
        let mem_growth = storm.mem_growth();

        let chart_mouse_min = chart_mouse_min_hz(avg_rate("chart_present"));
        let static_text_chart_mouse_min = chart_mouse_min_hz(static_text_avg_rate("chart_present"));

        let mut fail = Vec::new();
        check_min(
            &mut fail,
            "firetest_mouse_sent",
            avg_rate("firetest_mouse_sent"),
            1000.0,
        );
        check_min(
            &mut fail,
            "chart_mouse_move",
            avg_rate("chart_mouse_move"),
            chart_mouse_min,
        );
        let chart_mouse = avg_rate("chart_mouse_move");
        let fast_mouse = avg_rate("chart_mouse_move_fast");
        if chart_mouse > 1.0 {
            check_min(
                &mut fail,
                "chart_mouse_fast_coverage",
                fast_mouse / chart_mouse,
                0.90,
            );
        }
        check_max(
            &mut fail,
            "chart_mouse_move_entity",
            max_rate("chart_mouse_move_entity"),
            5.0,
        );
        check_max(&mut fail, "shell_render", max_rate("shell_render"), 10.0);
        check_max(&mut fail, "orders_render", max_rate("orders_render"), 10.0);
        check_max(&mut fail, "chart_render", max_rate("chart_render"), 10.0);
        check_max(
            &mut fail,
            "chart_input_notify",
            max_rate("chart_input_notify"),
            5.0,
        );
        check_max(
            &mut fail,
            "chart_canvas_notify",
            max_rate("chart_canvas_notify"),
            5.0,
        );
        check_max(
            &mut fail,
            "chart_gpu_prepare_delta",
            rate_delta("chart_gpu_prepare"),
            8.0,
        );
        check_max(&mut fail, "bg_draw_delta", rate_delta("bg_draw"), 12.0);
        check_max(&mut fail, "grid_draw_delta", rate_delta("grid_draw"), 12.0);
        // A strict cross-platform signal, deliberately not widened: cursor-only movement must not
        // add expensive combo draws over the baseline. A backend that cannot hold this is a reason
        // to bring its retained/base cache up to parity, not to raise the ceiling.
        check_max(
            &mut fail,
            "combo_draw_delta",
            rate_delta("combo_draw"),
            12.0,
        );
        check_max(
            &mut fail,
            "userdata_draw_delta",
            rate_delta("userdata_draw"),
            12.0,
        );
        check_max(&mut fail, "base_bake_delta", rate_delta("base_bake"), 8.0);
        check_max(&mut fail, "combo_bake_delta", rate_delta("combo_bake"), 8.0);
        check_max(
            &mut fail,
            "orderbook_bake_delta",
            rate_delta("orderbook_bake"),
            8.0,
        );
        if self.config.text_labels > 0 {
            check_max(
                &mut fail,
                "firetest_text_cold",
                max_rate("firetest_text_cold"),
                100.0,
            );
            check_min(
                &mut fail,
                "static_text_firetest_mouse_sent",
                static_text_avg_rate("firetest_mouse_sent"),
                1000.0,
            );
            check_min(
                &mut fail,
                "static_text_chart_mouse_move",
                static_text_avg_rate("chart_mouse_move"),
                static_text_chart_mouse_min,
            );
            let static_text_chart_mouse = static_text_avg_rate("chart_mouse_move");
            let static_text_fast_mouse = static_text_avg_rate("chart_mouse_move_fast");
            if static_text_chart_mouse > 1.0 {
                check_min(
                    &mut fail,
                    "static_text_chart_mouse_fast_coverage",
                    static_text_fast_mouse / static_text_chart_mouse,
                    0.90,
                );
            }
            check_max(
                &mut fail,
                "static_text_chart_mouse_move_entity",
                static_text_max_rate("chart_mouse_move_entity"),
                5.0,
            );
            check_max(
                &mut fail,
                "static_text_chart_input_notify",
                static_text_max_rate("chart_input_notify"),
                5.0,
            );
            check_max(
                &mut fail,
                "static_text_chart_canvas_notify",
                static_text_max_rate("chart_canvas_notify"),
                5.0,
            );
            check_max(
                &mut fail,
                "static_text_cpu_process_avg",
                static_text_avg_cpu,
                25.0,
            );
            check_max(
                &mut fail,
                "static_text_cpu_process_delta",
                static_text_cpu_delta,
                12.0,
            );
            check_max(
                &mut fail,
                "static_text_cpu_process_max",
                static_text_max_cpu,
                40.0,
            );
            if static_text_max_gpu_process > 0.1 {
                check_max(
                    &mut fail,
                    "static_text_gpu_process_avg",
                    static_text_avg_gpu_process,
                    35.0,
                );
                check_max(
                    &mut fail,
                    "static_text_gpu_process_delta",
                    static_text_gpu_process_delta,
                    25.0,
                );
                check_max(
                    &mut fail,
                    "static_text_gpu_process_max",
                    static_text_max_gpu_process,
                    70.0,
                );
            }
            if static_text_max_gpu_frame_ms > 0.01 {
                check_max(
                    &mut fail,
                    "static_text_gpu_frame_ms_avg",
                    static_text_avg_gpu_frame_ms,
                    6.0,
                );
                check_max(
                    &mut fail,
                    "static_text_gpu_frame_ms_max",
                    static_text_max_gpu_frame_ms,
                    16.0,
                );
            }
        }
        check_max(&mut fail, "cpu_process_avg", avg_cpu, 25.0);
        check_max(&mut fail, "cpu_process_delta", cpu_delta, 12.0);
        check_max(&mut fail, "cpu_process_max", max_cpu, 40.0);
        if max_gpu_process > 0.1 {
            check_max(&mut fail, "gpu_process_avg", avg_gpu_process, 35.0);
            check_max(&mut fail, "gpu_process_delta", gpu_process_delta, 25.0);
            check_max(&mut fail, "gpu_process_max", max_gpu_process, 70.0);
        }
        if max_gpu_frame_ms > 0.01 {
            check_max(&mut fail, "gpu_frame_ms_avg", avg_gpu_frame_ms, 6.0);
            check_max(&mut fail, "gpu_frame_ms_max", max_gpu_frame_ms, 16.0);
        }
        // Memory growth is reported below but not gated: a live multi-core config legitimately
        // grows by gigabytes during the smoke run as cores stream balance snapshots, candles, and
        // order books, so a fixed-MB growth ceiling only produces false failures against real data.

        let summary = format!(
            "mouse_sent={:.0}/s chart_mouse={:.0}/s fast={:.0}/s entity={:.0}/s fast_stop={:.0}/s shell={:.0}/s orders={:.0}/s chart_render={:.0}/s input_notify={:.0}/s text_draw={:.0}/s text_cold={:.0}/s static_text_labels={} static_text_chart_mouse={:.0}/s static_text_fast={:.0}/s static_text_cpu_avg={:.1}% static_text_gpu_proc_avg={:.1}% static_text_text_draw={:.0}/s static_text_text_cold={:.0}/s cpu_avg={:.1}% cpu_delta={:.1}% gpu_proc_avg={:.1}% gpu_proc_delta={:.1}% gpu_proc_max={:.1}% gpu_frame_avg={:.3}ms gpu_frame_max={:.3}ms mem_growth={:.1}MB present={:.0}/s cam_step={:.0}/s gpu_prepare={:.0}/s(+{:.0}) bg_draw={:.0}/s(+{:.0}) combo_draw={:.0}/s(+{:.0}) base_bake={:.0}/s(+{:.0}) combo_bake={:.0}/s(+{:.0}) book_bake={:.0}/s(+{:.0})",
            avg_rate("firetest_mouse_sent"),
            avg_rate("chart_mouse_move"),
            avg_rate("chart_mouse_move_fast"),
            avg_rate("chart_mouse_move_entity"),
            avg_rate("chart_mouse_fast_stop"),
            avg_rate("shell_render"),
            avg_rate("orders_render"),
            avg_rate("chart_render"),
            avg_rate("chart_input_notify"),
            avg_rate("firetest_text_draw"),
            avg_rate("firetest_text_cold"),
            self.config.text_labels,
            static_text_avg_rate("chart_mouse_move"),
            static_text_avg_rate("chart_mouse_move_fast"),
            static_text_avg_cpu,
            static_text_avg_gpu_process,
            static_text_avg_rate("firetest_text_draw"),
            static_text_avg_rate("firetest_text_cold"),
            avg_cpu,
            cpu_delta,
            avg_gpu_process,
            gpu_process_delta,
            max_gpu_process,
            avg_gpu_frame_ms,
            max_gpu_frame_ms,
            mem_growth,
            avg_rate("chart_present"),
            avg_rate("chart_cam_step"),
            avg_rate("chart_gpu_prepare"),
            rate_delta("chart_gpu_prepare"),
            avg_rate("bg_draw"),
            rate_delta("bg_draw"),
            avg_rate("combo_draw"),
            rate_delta("combo_draw"),
            avg_rate("base_bake"),
            rate_delta("base_bake"),
            avg_rate("combo_bake"),
            rate_delta("combo_bake"),
            avg_rate("orderbook_bake"),
            rate_delta("orderbook_bake"),
        );
        if fail.is_empty() {
            firetest_info(&format!("[firetest] result=PASS FIRETEST PASS {summary}"));
            std::process::exit(0);
        }
        firetest_error(&format!(
            "[firetest] result=FAIL FIRETEST FAIL {summary} reasons={}",
            fail.join("; ")
        ));
        std::process::exit(2);
    }
}

/// The floor for chart mouse-move throughput, derived from how fast the chart is presenting.
///
/// Windows drives a fixed high-rate storm and can demand an absolute floor. macOS and Linux post
/// events through the compositor, which coalesces them, so there the floor is tied to the present
/// rate instead of a constant that would fail for platform reasons rather than regressions.
fn chart_mouse_min_hz(present_hz: f64) -> f64 {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        (present_hz * 0.5).clamp(20.0, 60.0)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = present_hz;
        100.0
    }
}

/// Record a failure when `got` is below the floor.
fn check_min(fail: &mut Vec<String>, label: &str, got: f64, min: f64) {
    if got < min {
        fail.push(format!("{label} {got:.1} < {min:.1}"));
    }
}

/// Record a failure when `got` is above the ceiling.
fn check_max(fail: &mut Vec<String>, label: &str, got: f64, max: f64) {
    if got > max {
        fail.push(format!("{label} {got:.1} > {max:.1}"));
    }
}
