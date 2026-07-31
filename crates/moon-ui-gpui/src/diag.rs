//! Persistent render-frequency diagnostics. Global atomic counters are bumped from render,
//! observe, and notify paths. About once per second, the startup drain loop snapshots and resets
//! them, converts each value to hertz, and writes one line to `render_diag.log` and `log::info`.
//! Use the runtime log, rather than code inspection alone, to find excessive rendering.
//!
//! Important: the gate is the `MOON_RENDER_DIAG` environment variable or an explicit
//! [`force_enable`] call, not `#[cfg(debug_assertions)]`. This project's `[profile.dev]` disables
//! debug assertions in `Cargo.toml` to avoid the DX12 validation layer, so a debug-assertion gate
//! would remove the counters from normal development builds. Without the environment variable or
//! a force-enable call, diagnostics are inert in both development and release builds and do not
//! create the log file.
//!
//! This remains manual instrumentation at selected call sites, so new paths can be missed. A
//! framework checkpoint that records each view render by type would cover view rendering more
//! completely; custom own-pass layers would still require manual counters.

use std::sync::atomic::{AtomicBool, AtomicU64};

macro_rules! diag_counters {
    ($($name:ident => $label:literal),* $(,)?) => {
        $( pub static $name: AtomicU64 = AtomicU64::new(0); )*
        fn snapshot_and_reset() -> Vec<(&'static str, u64)> {
            use std::sync::atomic::Ordering;
            vec![ $( ($label, $name.swap(0, Ordering::Relaxed)) ),* ]
        }
    };
}

diag_counters!(
    ORDERS_RENDER     => "orders_render",
    // The News feed repaints on a news revision change — plus, while a just-arrived card's arrival
    // tint fades, at `crate::pulse::PULSE_TICK`. This counter is what tells the two apart; compare
    // it against `pulse_tick` to see which of the two is driving.
    NEWS_RENDER       => "news_render",
    // View repaints requested by `crate::pulse` to advance a decorative fade. Its only user is the
    // News arrival tint: a timer that re-renders the owning PANEL, so if it ever fails to stop it
    // looks exactly like a mysterious idle floor.
    //
    // Deliberately NOT shared with the chart arrival flash — that one costs presents rather than
    // view renders, and one counter answering for two mechanisms answers for neither. See
    // `chart_arrival_pulse`.
    PULSE_TICK        => "pulse_tick",
    // Presents requested by the chart's own pass to advance the new-chart border flash
    // (`chartdx::render_state`). Zero is the normal state; a run with no chart arrival says nothing
    // about the flash's cost, which is exactly why this is counted separately. A value that never
    // returns to zero means the flash failed to expire and this canvas is presenting forever.
    CHART_ARRIVAL_PULSE => "chart_arrival_pulse",
    // Order-line hover enter/leave inside the fast mouse-move branch. Each one is a `cx.notify()`
    // that dirties the view AND every ancestor, and a re-rendered root bypasses every descendant's
    // cache — so one of these repaints the entire window. `chart_input_notify` and
    // `chart_canvas_notify` do NOT cover this path, which is why storm-time window repaints have
    // gone unexplained: the gate watched counters blind to the branch that fires.
    CHART_HOVER_NOTIFY => "chart_hover_notify",
    SHELL_RENDER      => "shell_render",
    CHART_RENDER      => "chart_render",
    DETACHED_RENDER   => "detached_render",
    BACKEND_NOTIFY    => "backend_notify",
    CHART_PREPARE     => "chart_prepare",
    CHART_FRAME       => "chart_frame",
    CHART_FRAME_REQUEST => "chart_frame_request",
    CHART_FRAME_SKIP_NOT_PRESENTABLE => "chart_frame_skip_not_presentable",
    CHART_FRAME_SKIP_IDLE => "chart_frame_skip_idle",
    CHART_GPU_PREPARE => "chart_gpu_prepare",
    // `CHART_PRESENT` counts actual gpu_canvas draw calls. `CHART_CAM_STEP` counts active-pane
    // camera advances that moved at least one pixel during frame decisions. Compare the rates to
    // assess pixel-threshold suppression, while accounting for multiple active panes per present;
    // finer zoom levels normally suppress more subpixel camera advances.
    CHART_PRESENT     => "chart_present",
    CHART_CAM_STEP    => "chart_cam_step",
    // Per-layer gpu_canvas counters required by the AGENTS.md UI Render Diagnostics contract.
    // The canvas runs outside GPUI view rendering, so each platform backend bumps these counters
    // manually. `*_DRAW` and `*_BLIT` count actual layer operations; `*_BAKE` counts texture-cache
    // rebuilds for cached layers such as combo and order book. Rebuilds should normally follow
    // data or view changes rather than every draw; similar BAKE and DRAW rates indicate poor reuse.
    CHART_BG_DRAW     => "bg_draw",
    CHART_GRID_DRAW   => "grid_draw",
    CHART_CURSOR_DRAW => "cursor_draw",
    CHART_BASE_BAKE   => "base_bake",
    CHART_BASE_BLIT   => "base_blit",
    CHART_COMBO_DRAW  => "combo_draw",
    CHART_COMBO_BAKE  => "combo_bake",
    // The candle layer draws during each base pass. `UPLOAD_LEN` counts rows uploaded after a
    // candle-series revision; live-edge trade batches advance that revision, which is expected and
    // inexpensive because the instance buffer contains only hundreds of rows.
    CHART_CANDLE_DRAW => "candle_draw",
    CHART_CANDLE_UPLOAD_LEN => "candle_upload_len",
    CHART_HISTORY_RESET_ROWS => "history_reset_rows",
    CHART_HISTORY_RESET_MS => "history_reset_ms",
    CHART_COMBO_UPLOAD_LEN => "combo_upload_len",
    CHART_PRICE_LINE_UPLOAD_LEN => "price_line_upload_len",
    CHART_BOOK_DRAW   => "orderbook_draw",
    CHART_BOOK_BAKE   => "orderbook_bake",
    CHART_USER_DRAW   => "userdata_draw",
    ORDERS_OBS_FIRE   => "orders_obs_fire",
    ORDERS_OBS_NOTIFY => "orders_obs_notify",
    SHELL_OBS_FIRE    => "shell_obs_fire",
    SHELL_OBS_NOTIFY  => "shell_obs_notify",
    CHART_OBS_FIRE    => "chart_obs_fire",
    CHART_OBS_NOTIFY  => "chart_obs_notify",
    CHART_OPEN_NOTIFY => "chart_open_notify",
    CHART_TTL_NOTIFY  => "chart_ttl_notify",
    CHART_INPUT_NOTIFY => "chart_input_notify",
    CHART_CANVAS_NOTIFY => "chart_canvas_notify",
    CHART_MOUSE_MOVE => "chart_mouse_move",
    CHART_MOUSE_MOVE_FAST => "chart_mouse_move_fast",
    CHART_MOUSE_MOVE_ENTITY => "chart_mouse_move_entity",
    CHART_MOUSE_FAST_STOP => "chart_mouse_fast_stop",
    CHART_CURSOR_UPDATE => "chart_cursor_update",
    // Comparison-mode ghost crosshair updates: successful price changes written to sibling charts
    // per second. Each change requests a present, although multiple writes can coalesce before the
    // draw. Compare this with `chart_cursor_update` on the hovered chart.
    CHART_GHOST_UPDATE => "chart_ghost_update",
    FIRETEST_MOUSE_SENT => "firetest_mouse_sent",
    FIRETEST_MOUSE_POST_FAIL => "firetest_mouse_post_fail",
    FIRETEST_TEXT_DRAW => "firetest_text_draw",
    FIRETEST_TEXT_COLD => "firetest_text_cold",
    // Background CPU investigation counters added in 2026-07. Each rate shows which path wakes
    // without changed input; the diagnostic line also records CPU, window count, and chart count
    // so the log identifies what was open and ticking during a CPU increase.
    // The header clock has one roughly 1 Hz timer per Shell window, so its rate approximates the
    // number of open Shell windows.
    CLOCK_NOTIFY => "clock_notify",
    // Asset snapshots collected by feed threads, measured as the summed `assets_rev` delta across
    // all cores. A positive rate while the Assets window is closed indicates work without a UI
    // consumer.
    ASSETS_APPLY => "assets_apply",
    // Assets-window renders; a positive rate means the window is open and redrawing.
    ASSETS_RENDER => "assets_render",
    // Screener rebuilds, each a full pass over all markets; a positive rate means it is open.
    SCREENER_REBUILD => "screener_rebuild",
    // Actual core-detect scans in `play_detect_sounds`, after its `detects_rev` gate. Counting before
    // that gate would show the feed-drain wake rate of roughly 250 Hz; this counts only revisions.
    DETECT_SCAN => "detect_scan",
    // `sync_orders_from_backend_notify` calls from chart-panel observers, multiplied by the number
    // of open charts observing each backend notification.
    CHART_ORDER_SYNC => "chart_order_sync",
    // Self-rearming roughly 1 Hz chart-stack compaction timers, one per non-empty Add/Custom stack.
    COMPACT_TICK => "compact_tick",
);

#[derive(Clone, Debug)]
pub struct DiagRate {
    pub label: &'static str,
    pub hz: f64,
}

static FORCE_ON: AtomicBool = AtomicBool::new(false);
static GPU_FRAME_US_SUM: AtomicU64 = AtomicU64::new(0);
static GPU_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Return whether render diagnostics have been enabled explicitly.
///
/// [`force_enable`] takes effect immediately. Otherwise, the first call caches whether
/// `MOON_RENDER_DIAG` exists; any value enables diagnostics. Without either mechanism, counters
/// and `render_diag.log` remain inactive in every build profile. This cannot use
/// `cfg(debug_assertions)` because the development profile disables debug assertions to avoid the
/// DX12 validation layer.
fn enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    FORCE_ON.load(std::sync::atomic::Ordering::Relaxed)
        || *ON.get_or_init(|| std::env::var_os("MOON_RENDER_DIAG").is_some())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn is_enabled() -> bool {
    enabled()
}

pub fn force_enable() {
    FORCE_ON.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[inline]
pub fn bump(c: &AtomicU64) {
    if enabled() {
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[inline]
pub fn bump_by(c: &AtomicU64, n: u64) {
    if enabled() && n > 0 {
        c.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Snapshot and reset all counters for the elapsed interval, returning their rates in hertz.
///
/// Returns `None` without `MOON_RENDER_DIAG` or [`force_enable`].
pub fn take_sample(elapsed_ms: f64) -> Option<Vec<DiagRate>> {
    if !enabled() {
        return None;
    }
    let snap = snapshot_and_reset();
    let hz = |c: u64| c as f64 * 1000.0 / elapsed_ms.max(1.0);
    Some(
        snap.into_iter()
            .map(|(label, count)| DiagRate {
                label,
                hz: hz(count),
            })
            .collect(),
    )
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn record_gpu_frame_ms(ms: f64) {
    if !enabled() || !ms.is_finite() || ms <= 0.0 {
        return;
    }
    let us = (ms * 1000.0).round().clamp(1.0, u64::MAX as f64) as u64;
    GPU_FRAME_US_SUM.fetch_add(us, std::sync::atomic::Ordering::Relaxed);
    GPU_FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn take_gpu_frame_ms() -> f64 {
    let sum = GPU_FRAME_US_SUM.swap(0, std::sync::atomic::Ordering::Relaxed);
    let count = GPU_FRAME_COUNT.swap(0, std::sync::atomic::Ordering::Relaxed);
    if count == 0 {
        0.0
    } else {
        sum as f64 / count as f64 / 1000.0
    }
}

pub fn format_sample(elapsed_ms: f64, sample: &[DiagRate]) -> String {
    let mut line = format!("[diag {:.0}ms]", elapsed_ms);
    for rate in sample {
        line.push_str(&format!(" {}={:.0}", rate.label, rate.hz));
    }
    line
}

/// Write a sample to the application log and append it to `render_diag.log` when possible.
///
/// `ctx` describes the sampling moment, including process/system CPU and open window/chart counts.
/// It is inserted after the diagnostic prefix so the log shows what was open at the measured load.
pub fn write_sample(elapsed_ms: f64, sample: &[DiagRate], ctx: &str) {
    use std::io::Write;
    let mut line = format_sample(elapsed_ms, sample);
    if !ctx.is_empty() {
        let head_end = line.find(']').map(|i| i + 1).unwrap_or(0);
        line.insert_str(head_end, &format!(" {ctx}"));
    }
    log::info!("{line}");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("render_diag.log")
    {
        let _ = writeln!(f, "{line}");
    }
}
