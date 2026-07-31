//! Turning the collected samples into PASS or FAIL.
//!
//! Two shapes of threshold live here and they mean different things. An absolute ceiling
//! (`chart_render`, `cpu_process_max`) says "this must never be that high at all". A `*_delta`
//! ceiling says "the storm must not add this much OVER the already-hot baseline" — that is the one
//! that survives a live market, because the baseline phase already paid for the feed's own work.
//! Every failed check is reported, not just the first: one run should hand back the whole list.

use crate::firetest::Runtime;
use crate::firetest::bench::BenchShape;
use crate::firetest::logging::{firetest_error, firetest_info};
use crate::firetest::plan::Phase;
use crate::firetest::sample::PhaseStats;

impl Runtime {
    /// Score the run and exit the process: `0` for PASS, `2` for FAIL.
    ///
    /// The narrow order script has no perf samples to score — reaching this point at all means its
    /// one stage already passed.
    pub(super) fn evaluate_and_exit(&mut self) {
        self.finish();
        if self.config.is_order_cancel_script() {
            firetest_info("[firetest] result=PASS FIRETEST PASS order_cancel_lag");
            std::process::exit(0);
        }

        let idle = PhaseStats::of(&self.samples, Phase::IdleFloor);
        if idle.is_empty() {
            self.fail("no idle floor diag samples");
            return;
        }
        // Reported before any threshold is applied to it: the ceilings below were calibrated from
        // these numbers on a live bench, and the line is what a future recalibration reads.
        //
        // avg AND max for every rate, deliberately. A single hot second — a fading arrival tint,
        // one burst of feed — reads identically to a permanently spinning panel if only the peak
        // is printed, and the two want opposite fixes.
        let Some(bench) = self.bench else {
            self.fail("idle floor did not record the bench shape");
            return;
        };
        let idle_rate =
            |label: &str| format!("{:.0}/{:.0}", idle.avg_rate(label), idle.max_rate(label));
        firetest_info(&format!(
            "[firetest] idle_floor bench cores={} charts={} windows={}",
            bench.cores, bench.charts, bench.windows
        ));
        firetest_info(&format!(
            "[firetest] idle_floor avg/max shell={} orders={} news={} chart_render={} detached={} backend_notify={} chart_present={} clock={} order_sync={} compact={} assets={} cpu={:.1}/{:.1}%",
            idle_rate("shell_render"),
            idle_rate("orders_render"),
            idle_rate("news_render"),
            idle_rate("chart_render"),
            idle_rate("detached_render"),
            idle_rate("backend_notify"),
            idle_rate("chart_present"),
            idle_rate("clock_notify"),
            idle_rate("chart_order_sync"),
            idle_rate("compact_tick"),
            idle_rate("assets_render"),
            idle.avg_cpu(),
            idle.max_cpu(),
        ));

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
        //
        // Over the CLEAN storm, not both storms joined. Joined was a real attribution bug: the
        // static-text phase hangs 10k retained labels on the chart, so its draw rates rise for a
        // reason that has nothing to do with the cursor, and averaging the two phases charged that
        // cost to "cursor-only mousemove". A run could go red with `bg_draw_delta 45` while the
        // clean storm sat at `36 -> 35` against its baseline.
        let rate_delta =
            |label: &str| (clean_storm.avg_rate(label) - baseline.max_rate(label)).max(0.0);

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

        // Does the baseline actually reach the storm's present rate? Every `*_delta` threshold
        // assumes it does — that is the entire premise of comparing against a hot baseline rather
        // than an idle one. When it does not, every delta inflates by the difference and the run
        // goes red for a reason that has nothing to do with the code under test.
        firetest_info(&format!(
            "[firetest] baseline_vs_storm present={:.0}->{:.0}/s chart_render={:.0}->{:.0}/s bg_draw={:.0}->{:.0}/s samples={}->{}",
            baseline.max_rate("chart_present"),
            clean_storm.avg_rate("chart_present"),
            baseline.max_rate("chart_render"),
            clean_storm.avg_rate("chart_render"),
            baseline.max_rate("bg_draw"),
            clean_storm.avg_rate("bg_draw"),
            baseline.len(),
            clean_storm.len(),
        ));

        let mut fail = Vec::new();

        // The idle floor, stated PER UNIT of the bench. A run on one core with one chart and a run
        // on fifty cores with five charts must reach the same verdict, so every ceiling here is
        // divided by whatever the counter scales with before it is compared. The law per counter
        // lives in `bench.rs`, which also marks which laws are documented and which are assumed.
        //
        // The numbers are measured-then-margined, not invented: three live runs on a bench of
        // cores=1 charts=1 windows=2 gave shell 5-8, orders 0, news/detached 4-5 avg,
        // backend_notify 4-5, clock 1, compact 0-1, assets 4-5, order_sync 5-8, cpu 2.4-4.2%.
        //
        // News and the detached panel are gated on the AVERAGE only. One run peaked at 115/s for a
        // single second, which is the arrival-tint fade `diag.rs` documents — legitimate, and a max
        // ceiling would fail the run for it.
        check_max(
            &mut fail,
            "idle_shell_render_per_window",
            bench.per_window(idle.max_rate("shell_render")),
            10.0,
        );
        check_max(
            &mut fail,
            "idle_orders_render_per_window",
            bench.per_window(idle.max_rate("orders_render")),
            5.0,
        );
        check_max(
            &mut fail,
            "idle_news_render_avg_per_window",
            bench.per_window(idle.avg_rate("news_render")),
            15.0,
        );
        check_max(
            &mut fail,
            "idle_detached_render_avg_per_window",
            bench.per_window(idle.avg_rate("detached_render")),
            15.0,
        );
        check_max(
            &mut fail,
            "idle_assets_render_per_window",
            bench.per_window(idle.max_rate("assets_render")),
            10.0,
        );
        // One roughly 1 Hz timer per Shell window, per `diag.rs`. A per-window rate above that
        // means a window grew a second clock, or the clock stopped being 1 Hz.
        check_max(
            &mut fail,
            "idle_clock_notify_per_window",
            bench.per_window(idle.max_rate("clock_notify")),
            2.5,
        );
        // One self-rearming ~1 Hz compaction timer per non-empty chart stack.
        check_max(
            &mut fail,
            "idle_compact_tick_per_chart",
            bench.per_chart(idle.max_rate("compact_tick")),
            2.5,
        );
        // Feed-driven: the drain wakes the backend, and every open chart observes each wake.
        check_max(
            &mut fail,
            "idle_backend_notify_per_core",
            bench.per_core(idle.max_rate("backend_notify")),
            12.0,
        );
        check_max(
            &mut fail,
            "idle_chart_order_sync_per_chart",
            bench.per_chart(idle.max_rate("chart_order_sync")),
            15.0,
        );
        check_max(&mut fail, "idle_cpu_process_avg", idle.avg_cpu(), 12.0);
        check_max(&mut fail, "idle_cpu_process_max", idle.max_cpu(), 20.0);
        // RECORDED, NOT ENDORSED. With no input and no forced present, `ChartPanel::render` runs
        // 41-93/s per chart on this bench — more often than under the mouse storm, where the same
        // counter is held to 10/s because waking the entity on cursor movement is a known defect.
        // Whatever wakes it at idle has not been identified, so this ceiling catches that cost
        // DOUBLING rather than blessing it. Lower it once the wake source is found and fixed.
        check_max(
            &mut fail,
            "idle_chart_render_avg_per_chart",
            bench.per_chart(idle.avg_rate("chart_render")),
            150.0,
        );

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
        // Deltas over the baseline, NOT absolute ceilings. These three counters scale with the
        // bench — with how many charts the saved layout restored, and with how fast the chart is
        // presenting, which FireTest itself inflates via forced present. An absolute ceiling
        // therefore measures the developer's layout: `chart_render` sat at 5/s on a bench
        // presenting 115/s and at 235/s on one presenting 600/s, with no code change between them.
        // That is the whole of the "flaky red baseline" this run was known for.
        //
        // The baseline phase runs with the SAME forced present and the SAME charts and no cursor,
        // so the bench cancels out of the difference and what is left is what the cursor added —
        // which is the thing these checks were always meant to be about.
        // Each storm scored against the baseline separately, and both through the same function,
        // so neither can quietly lose a ceiling the other has.
        check_storm_deltas(&mut fail, "", &clean_storm, &baseline, bench, 1.0);
        // The static-text storm gets the SAME checks with a wider allowance, measured not guessed.
        // Attaching 10k retained labels costs this chart's own pass +55..110/s of draw and bake and
        // +65..191/s of userdata draw over the baseline — every one of those counters rising by the
        // same amount, which is the signature of more presents rather than more work per present.
        //
        // RECORDED, NOT ENDORSED, and newly visible: before this, the static-text phase carried no
        // draw or bake ceiling at all, and its cost was averaged into the clean storm's numbers,
        // where it read as the cursor's fault. The right end state is a per-present ratio — draws
        // divided by `chart_present` — which would separate "presents more often" from "does more
        // work per present" and let this allowance go back to 1.0. Until then it catches a doubling.
        check_storm_deltas(
            &mut fail,
            "static_text_",
            &static_text_storm,
            &baseline,
            bench,
            20.0,
        );
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

/// Every "did this storm add expensive work over the already-hot baseline" ceiling, in one place.
///
/// Called once per storm phase, so a ceiling added here applies to every storm BY CONSTRUCTION.
/// That is the point: the two storms had drifted apart, and the static-text phase carried no draw
/// or bake ceiling at all — so "a regression that only shows up under text load is not invisible",
/// which is the reason that phase exists, was false for exactly the counters it was about.
///
/// The baseline's PEAK is the reference, and the storm's AVERAGE is compared to it, so a live feed
/// that happened to be busier during one phase cannot make this red on its own.
fn check_storm_deltas(
    fail: &mut Vec<String>,
    prefix: &str,
    storm: &PhaseStats,
    baseline: &PhaseStats,
    bench: BenchShape,
    allowance: f64,
) {
    // PER CHART. Every counter below is bumped once per chart per pass, so a bench that restored
    // five charts produces five times the rate for identical code. Measured on one machine, same
    // binary, consecutive runs: charts=1 -> chart_present 119/s, charts=2 -> 341/s, charts=5 ->
    // 793/s, and the draw deltas rose with them until the run went red. The saved layout decides
    // how many charts come back, so without this division the verdict is a property of the
    // developer's desktop.
    let delta =
        |label: &str| bench.per_chart((storm.avg_rate(label) - baseline.max_rate(label)).max(0.0));
    let named = |name: &str| format!("{prefix}{name}");
    let ceiling = |base: f64| base * allowance;
    // The GPUI view path: a storm must not wake a panel that has no reason to redraw.
    check_max(
        fail,
        &named("shell_render_delta"),
        delta("shell_render"),
        ceiling(8.0),
    );
    check_max(
        fail,
        &named("orders_render_delta"),
        delta("orders_render"),
        ceiling(8.0),
    );
    check_max(
        fail,
        &named("chart_render_delta"),
        delta("chart_render"),
        ceiling(8.0),
    );
    // The chart's own pass: draws are cheap-ish, bakes rebuild a cached texture and are not.
    check_max(
        fail,
        &named("chart_gpu_prepare_delta"),
        delta("chart_gpu_prepare"),
        ceiling(8.0),
    );
    check_max(
        fail,
        &named("bg_draw_delta"),
        delta("bg_draw"),
        ceiling(12.0),
    );
    check_max(
        fail,
        &named("grid_draw_delta"),
        delta("grid_draw"),
        ceiling(12.0),
    );
    // A strict cross-platform signal, deliberately not widened: a backend that cannot hold it is a
    // reason to bring its retained/base cache up to parity, not to raise the ceiling.
    check_max(
        fail,
        &named("combo_draw_delta"),
        delta("combo_draw"),
        ceiling(12.0),
    );
    check_max(
        fail,
        &named("userdata_draw_delta"),
        delta("userdata_draw"),
        ceiling(12.0),
    );
    check_max(
        fail,
        &named("base_bake_delta"),
        delta("base_bake"),
        ceiling(8.0),
    );
    check_max(
        fail,
        &named("combo_bake_delta"),
        delta("combo_bake"),
        ceiling(8.0),
    );
    check_max(
        fail,
        &named("orderbook_bake_delta"),
        delta("orderbook_bake"),
        ceiling(8.0),
    );
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
