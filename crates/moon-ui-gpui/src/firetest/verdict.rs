//! Turning the collected samples into PASS or FAIL.
//!
//! Two shapes of threshold live here and they mean different things. An absolute ceiling
//! (`cpu_process_max`, `chart_render_per_chart`) says "this must never be that high at all". A
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
        // Guarded like every other scored phase. An empty baseline is not "no work" — it makes
        // every reference 0.0, so each delta silently becomes an absolute check and the run goes
        // red for missing data while naming a regression.
        if baseline.is_empty() {
            self.fail("no baseline diag samples");
            return;
        }
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

        // The idle floor. These are LEVELS, so each is divided by the unit it genuinely scales
        // with; the law per counter lives in `bench.rs`, which marks which laws are documented and
        // which are assumed.
        //
        // Measured on a `cores=20 charts=1 windows=5` bench: shell 5-8, orders 0, news/detached
        // 4-5 avg, backend_notify 4-5, clock 1, compact 0-1, assets 4-5, order_sync 4-8,
        // cpu 2.2-4.2%. Only that one shape has been recorded, so the margins are wide on purpose
        // and this comment names the shape rather than pretending the numbers are universal.
        //
        // News and the detached panel are gated on the AVERAGE. A burst is legitimate — the
        // arrival-tint fade `diag.rs` documents — but the idle window is short, so a burst still
        // lifts the average noticeably; the ceiling leaves room for one and not for a panel that
        // never stops.
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
        // One self-rearming ~1 Hz timer per non-empty chart STACK, per `diag.rs` — stacks are not
        // counted, and there are never many, so this stays absolute rather than divided by the
        // wrong unit.
        check_max(
            &mut fail,
            "idle_compact_tick",
            idle.max_rate("compact_tick"),
            5.0,
        );
        // Absolute, NOT per core: `flush_backend_notify` coalesces behind a 250 ms gate, so this
        // is globally capped near 4/s however many cores are connected. Dividing it by cores made
        // a check that could not fail.
        check_max(
            &mut fail,
            "idle_backend_notify",
            idle.max_rate("backend_notify"),
            8.0,
        );
        check_max(
            &mut fail,
            "idle_chart_order_sync_per_chart",
            bench.per_chart(idle.max_rate("chart_order_sync")),
            15.0,
        );
        check_max(&mut fail, "idle_cpu_process_avg", idle.avg_cpu(), 12.0);
        check_max(&mut fail, "idle_cpu_process_max", idle.max_cpu(), 20.0);
        // Idle `ChartPanel::render` measured 5-6/s per chart on the recorded bench. The ceiling is
        // far above that because the same counter has been seen in the hundreds under the storm
        // (see `chart_render_per_chart`) and the wake source is not yet identified — until it is,
        // a tight idle ceiling would fail for the same unknown reason rather than for a regression.
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
        // Each storm scored against the baseline separately and through the same function, so one
        // phase cannot quietly carry a ceiling the other lacks. Scoring them jointly used to charge
        // the static-text phase's draw cost — 10k retained labels — to "cursor-only mousemove".
        //
        // Both get the SAME ceilings: the per-present ratios below are what makes that possible.
        // The text layer's cost showed up as more frames, not as more work per frame, so once the
        // frame count is divided out there is nothing left to make an allowance for.
        check_storm(&mut fail, "", &clean_storm, &baseline, bench);
        check_storm(
            &mut fail,
            "static_text_",
            &static_text_storm,
            &baseline,
            bench,
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
/// Score one storm phase against the baseline, per present.
///
/// Called once per storm, so a ceiling added here applies to every storm by construction — the
/// drift that let the two phases be scored differently cannot come back.
///
/// EVERYTHING here is a ratio to `chart_present`, and that is the whole design. These counters
/// are bumped inside the chart's own pass, so their raw rate is "how many frames happened" times
/// "what each frame cost". Only the second factor is about the code. The first is decided by the
/// developer's saved layout — measured on one machine, one binary, consecutive runs, the same
/// `bg_draw` sat anywhere between 28/s and 246/s — and dividing it out is what makes the verdict
/// mean the same thing on a one-chart bench and a five-chart one.
///
/// It also removes a second, independent source of red: the baseline does not always reach the
/// storm's present rate, because forced present amplifies delivered frames rather than generating
/// them, and the cursor generates them. A raw delta charges that difference to the code; a ratio
/// does not. Across 13 recorded runs the raw `bg_draw` delta ranged 0..109 while the ratio delta
/// stayed within 0..0.13.
///
/// Two shapes, both needed:
///   * the DELTA over the baseline's ratio — what this storm added per frame;
///   * an ABSOLUTE ratio ceiling — what a frame may cost at all. Without it, a regression present
///     in the baseline AND the storm cancels out and is invisible, which is exactly what the
///     absolute `chart_render <= 10/s` check used to cover before it was removed for being
///     bench-dependent. Per present it is not bench-dependent.
fn check_storm(
    fail: &mut Vec<String>,
    prefix: &str,
    storm: &PhaseStats,
    baseline: &PhaseStats,
    bench: BenchShape,
) {
    let storm_present = storm.avg_rate("chart_present");
    let baseline_present = baseline.max_rate("chart_present");
    // A phase with no presents has no frames to charge anything to. Reported rather than silently
    // scored: every ratio below would otherwise be 0.0 and pass.
    if storm_present < 1.0 {
        fail.push(format!("{prefix}chart_present {storm_present:.1} < 1.0"));
        return;
    }
    let per_frame = |label: &str| storm.avg_rate(label) / storm_present;
    let baseline_per_frame = |label: &str| {
        if baseline_present < 1.0 {
            0.0
        } else {
            baseline.max_rate(label) / baseline_present
        }
    };
    let delta = |label: &str| (per_frame(label) - baseline_per_frame(label)).max(0.0);
    let named = |name: &str| format!("{prefix}{name}");

    // The GPUI view path is deliberately NOT scored per frame. These count view renders, driven by
    // entity notifications rather than by the chart's frames, so dividing them by `chart_present`
    // measures nothing — the recorded ratio wanders between 0.04 and 0.80 in the baseline alone.
    //
    // What the run promises about them is `docs/FIRETEST.md`'s "Shell/Orders/Chart GPUI render не
    // должны улетать в сотни render/s". That is an absolute statement and it stays one. Only
    // `chart_render` is divided by the chart count: it is bumped once per chart panel, so its
    // LEVEL genuinely scales with how many charts the layout restored — unlike a delta, where the
    // per-chart term cancels in the subtraction.
    //
    // The narrower contract — cursor movement must not wake the chart entity at all — is stated
    // directly by `chart_mouse_move_entity`, `chart_input_notify` and `chart_canvas_notify`, which
    // the caller checks and which read 0 on a healthy run. Those are the oracle; this is the
    // backstop for a wake arriving by some other route.
    check_max(
        fail,
        &named("chart_render_per_chart"),
        bench.per_chart(storm.max_rate("chart_render")),
        120.0,
    );
    check_max(
        fail,
        &named("shell_render"),
        storm.max_rate("shell_render"),
        120.0,
    );
    check_max(
        fail,
        &named("orders_render"),
        storm.max_rate("orders_render"),
        120.0,
    );

    // The chart's own pass. Draws are per-frame work by nature, so the absolute ratio is about
    // "one draw per frame, not five"; bakes rebuild a cached texture and should be far rarer than
    // one per frame, which is the whole point of the cache.
    for (label, delta_max, abs_max) in [
        ("chart_gpu_prepare", 0.60, 1.60),
        ("bg_draw", 0.60, 1.60),
        ("grid_draw", 0.60, 1.60),
        ("combo_draw", 0.60, 1.60),
        ("userdata_draw", 0.60, 2.00),
        ("base_bake", 0.60, 1.60),
        ("combo_bake", 0.60, 1.60),
        ("orderbook_bake", 0.60, 1.60),
    ] {
        check_max(
            fail,
            &named(&format!("{label}_per_frame_delta")),
            delta(label),
            delta_max,
        );
        check_max(
            fail,
            &named(&format!("{label}_per_frame")),
            per_frame(label),
            abs_max,
        );
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
