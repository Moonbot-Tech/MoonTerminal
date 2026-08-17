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
use crate::firetest::sample::{PhaseStats, Sample};

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
            "[firetest] idle_floor avg/max shell={} orders={} news={} chart_render={} detached={} backend_notify={} chart_present={} clock={} order_sync={} compact={} assets={} arrival_pulse={} cpu={:.1}/{:.1}%",
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
            idle_rate("chart_arrival_pulse"),
            idle.avg_cpu(),
            idle.max_cpu(),
        ));
        if let Some(reason) = report_arrival_flash(&self.samples, &idle, bench, self.flash_charts) {
            self.fail(&reason);
            return;
        }

        let baseline = PhaseStats::of(&self.samples, Phase::Baseline);
        let clean_storm = PhaseStats::of(&self.samples, Phase::Storm);
        let static_text_storm = PhaseStats::of(&self.samples, Phase::StaticTextStorm);
        let flash_storm = PhaseStats::of(&self.samples, Phase::FlashStorm);
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
        if flash_storm.is_empty() {
            self.fail("no flash storm diag samples");
            return;
        }

        // What remains here feeds the SUMMARY line, not the gates: every threshold now lives in
        // `check_storm_input` / `check_storm` / `check_storm_load`, one call per storm phase. The
        // summary still reads the joined storms, so the headline numbers keep meaning what they
        // meant in every previously recorded run.
        let avg_rate = |label: &str| storm.avg_rate(label);
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

        let avg_cpu = storm.avg_cpu();
        let cpu_delta = (avg_cpu - baseline.avg_cpu()).max(0.0);
        let static_text_avg_cpu = static_text_storm.avg_cpu();

        let avg_gpu_process = storm.avg_gpu_process();
        let max_gpu_process = storm.max_gpu_process();
        let gpu_process_delta = (avg_gpu_process - baseline.avg_gpu_process()).max(0.0);
        let static_text_avg_gpu_process = static_text_storm.avg_gpu_process();

        let avg_gpu_frame_ms = storm.avg_gpu_frame_ms();
        let max_gpu_frame_ms = storm.max_gpu_frame_ms();
        let mem_growth = storm.mem_growth();

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

        // What the two costs are TOGETHER, against the cursor alone. The gates below already hold
        // the flash storm to every storm ceiling, but a ceiling only speaks when it is breached —
        // and the question this phase exists to answer ("does moving the mouse over a flashing
        // border cost more than the sum of the two?") needs the number on a passing run too.
        firetest_info(&format!(
            "[firetest] flash_storm clean->flash present={:.0}->{:.0}/s chart_render={:.0}->{:.0}/s per_frame bg_draw={:.2}->{:.2} base_bake={:.2}->{:.2} gpu_prepare={:.2}->{:.2} cpu={:.1}->{:.1}% entity={:.0}->{:.0}/s input_notify={:.0}->{:.0}/s pulse={:.1}/s samples={}->{}",
            clean_storm.avg_rate("chart_present"),
            flash_storm.avg_rate("chart_present"),
            clean_storm.avg_rate("chart_render"),
            flash_storm.avg_rate("chart_render"),
            per_present(&clean_storm, "bg_draw"),
            per_present(&flash_storm, "bg_draw"),
            per_present(&clean_storm, "base_bake"),
            per_present(&flash_storm, "base_bake"),
            per_present(&clean_storm, "chart_gpu_prepare"),
            per_present(&flash_storm, "chart_gpu_prepare"),
            clean_storm.avg_cpu(),
            flash_storm.avg_cpu(),
            clean_storm.max_rate("chart_mouse_move_entity"),
            flash_storm.max_rate("chart_mouse_move_entity"),
            clean_storm.max_rate("chart_input_notify"),
            flash_storm.max_rate("chart_input_notify"),
            flash_storm.avg_rate("chart_arrival_pulse"),
            clean_storm.len(),
            flash_storm.len(),
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
        // Idle `ChartPanel::render`, retightened 2026-08-08 from 150 to 30 on ten pooled runs
        // (`docs-internal/firetest-runs/arrival-flash-*.csv`): the per-chart idle average sat at
        // 4.4-5.9 in every one of them, so 150 was 25x above anything observed and could only ever
        // have caught a catastrophe.
        //
        // The 150 was set because the flash was suspected of waking this counter and the wake
        // source was "not yet identified". It has now been measured directly — the `arrival_flash`
        // phase moves this counter by a median of +0.15/s, i.e. not at all — so the reason for the
        // slack is gone. 30 keeps the same margin over the observed peak that Shell and Orders
        // carry, and matches `chart_render_per_chart` under the storm.
        check_max(
            &mut fail,
            "idle_chart_render_avg_per_chart",
            bench.per_chart(idle.avg_rate("chart_render")),
            30.0,
        );
        // The stuck-flash gate the storm ceiling could not express, now that the numbers exist.
        //
        // A lit flash asks for a present every 100 ms, so a canvas that never expires one averages
        // 10/s for the whole window. One REAL arrival is 2.6 s of a 5 s window — 5.2/s — and is
        // legitimate. Ten recorded idle windows saw a real arrival exactly zero times, so 7 sits
        // above the legitimate case, below the stuck one, and far above anything measured.
        //
        // On the AVERAGE, deliberately: what separates a burst from a permanent floor is duration,
        // not rate. A flash stuck at its design rate has a perfectly normal peak.
        check_max(
            &mut fail,
            "idle_chart_arrival_pulse_avg_per_chart",
            bench.per_chart(idle.avg_rate("chart_arrival_pulse")),
            7.0,
        );

        // Every storm through the SAME three functions, so a phase cannot quietly carry a ceiling
        // another lacks — the drift that once left the static-text phase with no draw or bake
        // ceiling at all, i.e. blind to exactly what it exists to catch.
        //
        // Per storm, never joined: joined was a real attribution bug. The static-text phase hangs
        // 10k retained labels on the chart, so its rates rise for a reason that has nothing to do
        // with the cursor, and averaging the phases charged that cost to "cursor-only mousemove".
        // Every ceiling below survives the split because the per-present ratios divide the frame
        // count out.
        for (prefix, phase) in [
            ("", &clean_storm),
            ("static_text_", &static_text_storm),
            ("flash_storm_", &flash_storm),
        ] {
            check_storm_input(&mut fail, prefix, phase);
            check_storm(&mut fail, prefix, phase, &baseline, bench);
            check_storm_load(&mut fail, prefix, phase, &baseline);
        }
        if self.config.text_labels > 0 {
            // The one gate that belongs to a single phase: only the text storm has a text layer.
            check_max(
                &mut fail,
                "firetest_text_cold",
                static_text_max_rate("firetest_text_cold"),
                100.0,
            );
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

/// Report what the arrival flash cost, against the idle floor that ran under the same conditions.
///
/// Still a REPORT, not a ceiling, and now for a measured reason rather than an absent one. Ten
/// pooled runs (2026-08-08) put the flash's cost at roughly +10 presents/s per flashing chart —
/// exactly the 10 Hz the own-pass paces it at — while `chart_render` moved by a median +0.15/s and
/// the per-frame own-pass work went DOWN (`bg_draw` 0.74 -> 0.59 per present: the extra frames
/// reuse the base cache). So the flash costs frames, not work per frame, and a ceiling here would
/// gate the pacing constant rather than a regression. What would earn one is a rate that is not
/// 10 Hz per chart — and `pulse_per_chart` below already says that number out loud.
///
/// What it DOES fail on is the phase not measuring what it claims: with the flash enabled it must
/// actually have pulsed, and with `MOON_ARRIVAL_FLASH=0` it must not have pulsed at all. Both are
/// oracles independent of the code under test — one says the A ran, the other says the B really was
/// a B — and without them a run with a broken hook would pool in as "the flash is free".
///
/// Returns the failure reason, or `None` when the phase is sound.
fn report_arrival_flash(
    samples: &[Sample],
    idle: &PhaseStats,
    bench: BenchShape,
    flash_charts: Option<usize>,
) -> Option<String> {
    let flash = PhaseStats::of(samples, Phase::ArrivalFlash);
    if flash.is_empty() {
        return Some("no arrival flash diag samples".into());
    }
    // Each phase divided by ITS OWN chart count, not both by the bench shape. The shape is captured
    // when the idle floor ends, and live detects open charts after that: measured over ten runs,
    // four grew during the flash window and one went 1 → 4, which reported that phase's per-chart
    // rates up to four times too high. Falls back to the bench only if the stage never reported.
    let flash_charts = flash_charts.unwrap_or(bench.charts).max(1);
    let per_flash_chart = |rate: f64| rate / flash_charts as f64;

    // Per chart for the LEVELS — a five-chart layout flashes five borders — and per present for
    // the own-pass work, for the same reason the storm is scored that way: how many frames happened
    // is the bench, what each frame cost is the code. Both printed, because the flash is expected
    // to move the first and the whole question is whether it also moves the second.
    let pair = |label: &str| {
        format!(
            "{:.1}->{:.1}",
            bench.per_chart(idle.avg_rate(label)),
            per_flash_chart(flash.avg_rate(label))
        )
    };
    let frame_pair = |label: &str| {
        format!(
            "{:.2}->{:.2}",
            per_present(idle, label),
            per_present(&flash, label)
        )
    };

    let pulse_avg = per_flash_chart(flash.avg_rate("chart_arrival_pulse"));
    let pulse_max = per_flash_chart(flash.max_rate("chart_arrival_pulse"));
    firetest_info(&format!(
        "[firetest] arrival_flash idle->flash charts={}->{flash_charts} per_chart present={} chart_render={} shell_render={} per_frame bg_draw={} grid_draw={} base_bake={} combo_bake={} userdata_draw={} gpu_prepare={} cpu={:.1}->{:.1}% cpu_max={:.1}->{:.1}% gpu_frame_ms={:.3}->{:.3} pulse_per_chart={pulse_avg:.1}/{pulse_max:.1} samples={}->{}",
        bench.charts,
        pair("chart_present"),
        pair("chart_render"),
        pair("shell_render"),
        frame_pair("bg_draw"),
        frame_pair("grid_draw"),
        frame_pair("base_bake"),
        frame_pair("combo_bake"),
        frame_pair("userdata_draw"),
        frame_pair("chart_gpu_prepare"),
        idle.avg_cpu(),
        flash.avg_cpu(),
        idle.max_cpu(),
        flash.max_cpu(),
        idle.avg_gpu_frame_ms(),
        flash.avg_gpu_frame_ms(),
        idle.len(),
        flash.len(),
    ));

    if crate::chartdx::arrival_flash_enabled() {
        // A lit flash bumps the counter once per `ARRIVAL_PULSE_TICK`, i.e. 10/s per canvas. The
        // floor is half that rather than 10: the first and last sampled seconds are partial, and a
        // present the pacer skipped is a frame not counted, neither of which is a broken hook.
        if pulse_avg < 5.0 {
            return Some(format!(
                "arrival_flash pulse_per_chart {pulse_avg:.1} < 5.0: the flash never lit, so this \
                 run measures nothing about it"
            ));
        }
    } else if pulse_max > 0.0 {
        return Some(
            "MOON_ARRIVAL_FLASH=0 but the flash still pulsed; the control side of the comparison \
             is not a control"
                .into(),
        );
    }
    None
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
    // What the run promises about them is absolute, and it stays absolute. Only `chart_render` is
    // divided by the chart count: it is a GLOBAL counter, bumped once per chart panel per render,
    // so its LEVEL genuinely scales with how many charts the layout restored — unlike a delta,
    // where the per-chart term cancels in the subtraction.
    //
    // The narrower contract — cursor movement must not wake the chart entity at all — is stated
    // directly by `chart_mouse_move_entity`, `chart_input_notify` and `chart_canvas_notify`, which
    // the caller checks and which read 0 on a healthy run. Those are the oracle; this is the
    // backstop for a wake arriving by some other route.
    //
    // Shell and Orders are held at 30/s: measured across 93 sample-seconds, including every storm
    // second, they never left 5.7/s average and 11/s peak. The chart ceiling was loosened to 120
    // for ONE known behaviour: a newly arrived chart pulsed its border for 2600 ms, notifying the
    // owning stack at frame rate and re-rendering every panel in it, ~59/s per chart.
    //
    // That pulse now lives in the chart's OWN PASS (`chartdx::render_state`) and costs presents
    // instead of view renders, so the reason for the loosening is gone entirely.
    //
    // Retightened to match Shell, from a distribution rather than a guess: six consecutive runs,
    // FOUR of which actually fired the flash (`chart_arrival_pulse` 22-24, i.e. one full 2.6 s
    // arrival each), peaked at 9 renders/s per chart. The runs WITH an arrival were
    // indistinguishable from the two without — which is the measurement that earns this number.
    // 30 keeps roughly the same margin over the observed peak that Shell and Orders carry.
    check_max(
        fail,
        &named("chart_render_per_chart"),
        bench.per_chart(storm.max_rate("chart_render")),
        30.0,
    );
    // NOT gated here, deliberately: `chart_present` is the DENOMINATOR of every per-frame ratio
    // below, so a decoration that forgets to stop presenting inflates it and quietly loosens all of
    // them at once. An absolute per-chart ceiling was tried and removed — twice wrong. The number
    // was miscalibrated (max present and max chart count come from different seconds, so dividing
    // one by the other understated it by 2x), and more importantly a ceiling cannot catch the
    // failure it was aimed at: a stuck flash presents at exactly its design rate.
    //
    // What separates a 2.6 s burst from a permanent floor is DURATION, not rate — so the gate
    // belongs in `idle_floor` as an AVERAGE of `chart_arrival_pulse`: a burst inside a longer idle
    // window averages near zero, a flash that never expires averages 10. Not yet written.
    check_max(
        fail,
        &named("shell_render"),
        storm.max_rate("shell_render"),
        30.0,
    );
    check_max(
        fail,
        &named("orders_render"),
        storm.max_rate("orders_render"),
        30.0,
    );

    // The chart's own pass. Draws are per-frame work by nature, so the absolute ratio is about
    // "one draw per frame, not five"; bakes rebuild a cached texture and should be far rarer than
    // one per frame, which is the whole point of the cache.
    for (label, delta_max, abs_max) in [
        // 0.80 rather than the 0.60 the other layers get, because a userdata rebuild is what
        // legitimately answers a hover change: moving the pointer over figures or order lines
        // re-bakes the layer, and that is the work, not waste. Under the storm the pointer visits
        // thousands of positions a second across a chart carrying figures, so the hover flips tens
        // of times a second and the rebuild rate rises with it — measured at 0.70 per frame on the
        // fixture bench. The ceiling still catches the failure this gate exists for: a rebuild
        // driven by the FRAME rather than by a change sits at 1.0 and above.
        ("chart_gpu_prepare", 0.80, 1.60),
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

/// One phase's rate for a counter, divided by the frames it happened over.
///
/// The only honest way to compare own-pass counters across phases that present at different rates:
/// a raw rate mixes "how many frames" with "what each frame cost", and only the second is about the
/// code. Zero when the phase barely presented, so a near-empty phase cannot report a huge ratio.
fn per_present(stats: &PhaseStats, label: &str) -> f64 {
    let present = stats.avg_rate("chart_present");
    if present < 1.0 {
        0.0
    } else {
        stats.avg_rate(label) / present
    }
}

/// The input-path contract every storm phase carries: the storm really happened, the chart saw it
/// on the fast path, and none of it woke the entity layer.
///
/// One function for every storm, so a phase added later cannot arrive without these. The wake
/// counters are the narrow oracle — they read 0 on a healthy run — and `check_storm`'s per-frame
/// ratios are the backstop for a wake arriving by another route.
fn check_storm_input(fail: &mut Vec<String>, prefix: &str, storm: &PhaseStats) {
    let named = |name: &str| format!("{prefix}{name}");
    check_min(
        fail,
        &named("firetest_mouse_sent"),
        storm.avg_rate("firetest_mouse_sent"),
        1000.0,
    );
    let chart_mouse = storm.avg_rate("chart_mouse_move");
    check_min(
        fail,
        &named("chart_mouse_move"),
        chart_mouse,
        chart_mouse_min_hz(storm.avg_rate("chart_present")),
    );
    // Guarded: with no mouse moves at all the ratio is 0/0, and the floor above already reported
    // the real problem — a second failure naming coverage would point at the wrong thing.
    if chart_mouse > 1.0 {
        check_min(
            fail,
            &named("chart_mouse_fast_coverage"),
            storm.avg_rate("chart_mouse_move_fast") / chart_mouse,
            0.90,
        );
    }
    for label in [
        "chart_mouse_move_entity",
        "chart_input_notify",
        "chart_canvas_notify",
    ] {
        check_max(fail, &named(label), storm.max_rate(label), 5.0);
    }
}

/// The process-load contract every storm phase carries: CPU, process GPU and GPU frame time.
///
/// The deltas are against the baseline's average, which is the same hot chart without a cursor, so
/// a busy market cannot produce them on its own. GPU checks are skipped when the platform reports
/// nothing rather than asserting on a fabricated zero.
fn check_storm_load(
    fail: &mut Vec<String>,
    prefix: &str,
    storm: &PhaseStats,
    baseline: &PhaseStats,
) {
    let named = |name: &str| format!("{prefix}{name}");
    check_max(fail, &named("cpu_process_avg"), storm.avg_cpu(), 25.0);
    check_max(
        fail,
        &named("cpu_process_delta"),
        (storm.avg_cpu() - baseline.avg_cpu()).max(0.0),
        12.0,
    );
    check_max(fail, &named("cpu_process_max"), storm.max_cpu(), 40.0);
    if storm.max_gpu_process() > 0.1 {
        check_max(
            fail,
            &named("gpu_process_avg"),
            storm.avg_gpu_process(),
            35.0,
        );
        check_max(
            fail,
            &named("gpu_process_delta"),
            (storm.avg_gpu_process() - baseline.avg_gpu_process()).max(0.0),
            25.0,
        );
        check_max(
            fail,
            &named("gpu_process_max"),
            storm.max_gpu_process(),
            70.0,
        );
    }
    if storm.max_gpu_frame_ms() > 0.01 {
        check_max(
            fail,
            &named("gpu_frame_ms_avg"),
            storm.avg_gpu_frame_ms(),
            6.0,
        );
        check_max(
            fail,
            &named("gpu_frame_ms_max"),
            storm.max_gpu_frame_ms(),
            16.0,
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
