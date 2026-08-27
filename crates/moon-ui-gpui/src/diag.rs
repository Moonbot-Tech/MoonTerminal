//! Persistent render-frequency diagnostics. Global atomic counters are bumped from render,
//! observe, and notify paths. About once per second, the startup drain loop snapshots and resets
//! them, converts each value to hertz, and writes one line to `logs/render_diag.log` and
//! `log::info`. Use the runtime log, rather than code inspection alone, to find excessive rendering.
//!
//! Important: the gate is `channels.render` in `cfg/diagnostics.toml` (equivalently
//! `MOON_RENDER_DIAG`) or an explicit [`force_enable`] call, not `#[cfg(debug_assertions)]`. This
//! project's `[profile.dev]` disables debug assertions in `Cargo.toml` to avoid the DX12 validation
//! layer, so a debug-assertion gate would remove the counters from normal development builds.
//! Without one of those switches, diagnostics are inert in both development and release builds and
//! do not create the log file.
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
    // The bottom volume band is a SECOND draw on the candle layer with its own on/off switch,
    // so it gets its own counter: folded into `candle_draw` a disabled band would be
    // indistinguishable from an enabled one and the reuse comparison would say nothing.
    CHART_CANDLE_VOLUME_DRAW => "candle_volume_draw",
    CHART_HISTORY_RESET_ROWS => "history_reset_rows",
    CHART_HISTORY_RESET_MS => "history_reset_ms",
    // Microseconds per second inside `read_chart_history_into`, over EVERY call — the reset pair
    // above covers only the resetting subset. A non-resetting read still copies the visible window
    // into the automatic-Y price-scan buffer, so without this a change that removes resets reads
    // as free while that copy goes on unmeasured.
    CHART_HISTORY_READ_US => "history_read_us",
    // Durable CLOSED-TRADE history reads started per second — a different subject from the three
    // counters above, which measure the LIVE trade buffer. Each one is an SQLite connection to the
    // report replica, and every chart tile now owns a target, so this is what says whether a detect
    // burst or a report generation turned into a read storm. Read it against the number of open
    // charts: one read per newly shown market is expected, a multiple of that is not.
    CHART_TRADE_HISTORY_READS => "trade_history_reads",
    // Volume-caption reads started per second, and what they cost in microseconds. One read per
    // distinct PERIOD per charted market, throttled to the arbitrage column's clock — so a stack of
    // eight panes on one coin printing one volume block should show about four a second, not
    // thirty-two. `volume_read_us` is the answer to "is the block what made the chart hitch": a
    // period the protocol's own buckets serve costs a struct copy, one the track serves costs a
    // walk over its buckets, and anything longer is walked out of retained aggregates.
    CHART_VOLUME_READS => "volume_reads",
    CHART_VOLUME_READ_US => "volume_read_us",
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
    // Drag moves whose notify the pacer dropped. Without it, "the drag was cheap" and "there was no
    // drag" print the same near-zero `chart_input_notify` and nothing tells them apart. The two are
    // NOT a partition: the move that ends a gesture is counted here when it is dropped and again in
    // `chart_input_notify` when the release settles it, so the sum runs one high per settled
    // gesture. Read `chart_input_notify` against `shell_render` — one notify repaints the whole
    // window, so those two moving in step is the cost this pacing exists to bound.
    CHART_INPUT_NOTIFY_PACED => "chart_input_notify_paced",
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
    // Configured chart captions actually DRAWN per second, summed over every pane, and how many of
    // those had to be re-formatted because their inputs moved.
    //
    // Read them against each other. `chart_caption_draw` divided by `chart_present` is how many
    // caption LINES each frame paints — a wrapped detect line counts once per line — where near
    // zero means the corner is empty, which the drawn picture
    // cannot tell apart from "the pane has nothing to say". `chart_caption_rebuild` is the
    // expensive half: it counts REVISIONS that changed a string, so it must stay orders of
    // magnitude below the draw count. The two moving together means a caption is being rebuilt on
    // every frame — a value formatted below its own printed precision, and a reshape of its
    // retained GPU run each time.
    //
    // One legitimate FLOOR was added in 2026-08: a chart carrying a countdown caption
    // ("До закрытия ТФ", funding) re-formats that pane once per clock step — once a second while a
    // countdown is inside its last hour, once a minute otherwise. Subtract `chart_countdown_tick`
    // before reading the rule above; what is left is the caption cost the rule is about.
    CHART_CAPTION_DRAW => "chart_caption_draw",
    CHART_CAPTION_REBUILD => "chart_caption_rebuild",
    // Panes whose captions were re-formatted by the COUNTDOWN clock rather than by a market
    // revision, per second. It is the floor `chart_caption_rebuild` carries on a chart that prints
    // a countdown, and the only way to tell that floor from a caption genuinely thrashing.
    //
    // Its ceiling is one per active pane per second, and it is reached whenever any drawn countdown
    // is inside its last hour — which for a 1м/5м/30м/1ч timeframe is always. Higher than that
    // means the clock quantum stopped working.
    CHART_COUNTDOWN_TICK => "chart_countdown_tick",
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
    // Log panel. An open Log tab used to re-read and re-parse its whole source on every backend
    // revision — whether or not a single row survived the errors-only filter — and nothing here
    // could see it: the panel works inside `render`, so it moved no counter and the cost surfaced
    // only as process CPU, mixed in with the charts.
    //
    // ALL of these are process-wide sums over every ACTIVE Log panel — the dock tab plus each
    // detached and group window — so three open panels triple the event rates. A panel behind
    // another dock tab is inactive and contributes nothing, which is the intended zero.
    //
    // None of them measures milliseconds except `log_ingest_us`; the rest are frequencies and
    // volumes. Read them as a set:
    //
    //   * `log_render` — panel renders, the same way `orders_render` and `news_render` read. Without
    //     it a per-frame cost inside the element tree is indistinguishable from a per-revision one.
    //   * `log_pull` — calls that reached the incremental read. NOT comparable to `backend_notify`:
    //     that one is a 250 ms-throttled whole-backend notify, while this fires only when the
    //     SELECTED source's signature moved, so a quiet source under a busy backend correctly prints
    //     `log_pull=0 backend_notify=4`.
    //   * `log_lines_parsed` — lines turned into rows. In the steady state this equals the rate the
    //     cores are writing at; a burst up to the 5000-row view limit is legitimate whenever
    //     `log_reload` moved in the same sample. A batch bigger than the buffer cap is truncated
    //     before parsing, so on a catch-up this reports rows KEPT, not lines the feed handed over.
    //   * `log_rows_filtered` — rows put through the filter predicate. Together with
    //     `log_lines_parsed` this is the pass/fail pair: an arrival filters the new rows plus, when a
    //     row landed out of order, the tail after it, so the two should stay the same order of
    //     magnitude. Thousands against tens is a whole-buffer pass — legitimate if `log_refilter`
    //     moved too (that is a keystroke), a regression if it did not.
    //   * `log_rows_evicted` — rows dropped past the cap. Tracks the arrival rate once the buffer is
    //     full, which is the ordinary state; it deliberately does NOT count the rows that stayed,
    //     because that number would sit at the cap forever and read as a fault in the correct state.
    //   * `log_refilter` — whole-buffer filter passes, one per keystroke in the search box or
    //     toggled filter, so typing legitimately prints several per sample. Any of these while
    //     nobody is touching the panel is a defect: no revision path reaches it.
    //   * `log_reload` — full source re-reads, which re-parse everything they read. Legitimate on
    //     the first load of a tab, a source or file change, an exchange losing a member, a selected
    //     core leaving the store, and a core added, removed or renamed — so a handful in a row while
    //     the user clicks around is normal. A steady rate with nobody clicking means something
    //     forces a reload per revision and the incremental path is dead.
    //   * `log_work_us` — microseconds per second spent on the panel's revision path: resolving the
    //     source list, reading the cursors, parsing, filtering and evicting, plus a full re-read when
    //     one happens. The direct answer to "what does an open Log tab cost", and the only one here
    //     a time regression cannot hide from. It excludes the element tree — that is `log_render`.
    LOG_RENDER => "log_render",
    LOG_PULL => "log_pull",
    LOG_LINES_PARSED => "log_lines_parsed",
    LOG_ROWS_FILTERED => "log_rows_filtered",
    LOG_ROWS_EVICTED => "log_rows_evicted",
    LOG_REFILTER => "log_refilter",
    LOG_RELOAD => "log_reload",
    LOG_WORK_US => "log_work_us",
    // Strategies window. Its cost is invisible from the outside for the same reason the Log panel's
    // was: everything happens inside `render`, so the only symptom is process CPU. Two different
    // repaints reach this window and they cost wildly different amounts, which is why they are
    // counted apart:
    //
    //   * `strat_render` — whole-window renders of `StrategiesView`. EVERY one of these rebuilds the
    //     tree adapter, the versions pane, the schema sections and the parameter model from scratch.
    //     A mouse press refreshes the window unconditionally (GPUI `div.rs` arms `window.refresh()`
    //     on mouse-down), and a backend revision notifies it, so a few per second while the user
    //     works is expected; a steady rate with nobody touching the window is not.
    //   * `strat_row_render` — individual tree ROW renders, driven by MoonTree's own repaint. Hover
    //     moves between rows notify the tree state only, so these can run hot while `strat_render`
    //     stays flat. Divide by the number of visible rows (roughly the tree's height in rows) to
    //     read it as a tree repaint rate. This is the counter that says whether moving the mouse
    //     costs the whole window or just the list.
    //   * `strat_tree_us` — microseconds per second on the tree adapter, covering BOTH the cache
    //     signature every frame pays and the rebuild only a miss does. Divide by `strat_render` for
    //     the per-frame cost, and read it together with `strat_tree_build` below.
    //   * `strat_tree_build` — actual rebuilds, i.e. cache MISSES. `strat_tree_build / strat_render`
    //     is the miss rate: near 1 means the cache never helps and some input churns every frame,
    //     which is a defect in the signature rather than in the tree. Data changes at most a few
    //     times a second, so a mouse sweep should show a handful of builds against ~85 renders.
    //   * `strat_tree_nodes` — nodes put into the side map per second, counted ONLY on a rebuild.
    //     Divided by `strat_tree_build` (not by `strat_render`) it gives the visible node count,
    //     which is what makes the rebuild cost interpretable: 400 µs over 3000 expanded rows is a
    //     different verdict than 400 µs over 12.
    //   * `strat_tree_push` — forest pushes actually handed to MoonTree, after the shape-signature
    //     gate. Each one deep-clones the item forest twice inside MoonTree, so this should stay near
    //     zero while only data changes; tracking `strat_render` means the gate is defeated.
    //
    // The remaining four split the element tree by pane, because the first measurement showed the
    // panes costing three times what the tree adapter does — and a single figure for "the panes"
    // cannot say which of them to fix. Together they account for everything after the adapter:
    //
    //   * `strat_sig_store` / `strat_sig_view` — which HALF of the tree signature moved on a miss:
    //     the cores (strategy snapshots, open-order counts) or the user (filter, expansion,
    //     selection, staging). They answer the only question a bare miss rate raises. `store`
    //     climbing while nobody touches the window means an input churns faster than the tree it
    //     describes actually changes — a signature defect, not a data one.
    //   * `strat_versions_us` — the versions pane plus the gated deleted/version cache checks that
    //     run just before it.
    //   * `strat_treepane_us` — the LEFT pane: its own element tree (search box, kind and direction
    //     combos, the create dropdown and the action bar) PLUS the cache lookup that now feeds it.
    //     Read it with `strat_pane_build` beside it, or a rise is unattributable — the two costs
    //     inside this one number move for opposite reasons.
    //   * `strat_pane_build` — left-pane derivations actually rebuilt, i.e. cache misses: the kinds
    //     list, the Start/Stop plan and the footer label measurement, so at most three per frame.
    //     Read against `strat_render`: a hover sweep should hold this near zero while renders run at
    //     monitor rate. The two climbing together means a key churns per frame, and the cost above
    //     is the old per-frame walk back under a new name.
    //   * `strat_sections_us` / `strat_params_us` — the schema-section list and the parameter rows.
    //   * `strat_model_us` — the COMPUTED half of those two: dependency values and the parameter
    //     model, both of which are pure functions of the selection and could be cached the way the
    //     tree is. The two above are then the element trees, which cannot: GPUI only reuses a view
    //     it was told to cache, and these panes are functions returning elements, not views. So
    //     this split is the go/no-go for caching them — a large `strat_model_us` is worth a cache,
    //     a small one means the cost is in element construction and only a real view would help.
    STRAT_RENDER => "strat_render",
    STRAT_ROW_RENDER => "strat_row_render",
    STRAT_TREE_US => "strat_tree_us",
    STRAT_TREE_BUILD => "strat_tree_build",
    STRAT_TREE_NODES => "strat_tree_nodes",
    STRAT_TREE_PUSH => "strat_tree_push",
    STRAT_SIG_STORE => "strat_sig_store",
    STRAT_SIG_VIEW => "strat_sig_view",
    STRAT_VERSIONS_US => "strat_versions_us",
    STRAT_TREEPANE_US => "strat_treepane_us",
    STRAT_PANE_BUILD => "strat_pane_build",
    STRAT_SECTIONS_US => "strat_sections_us",
    STRAT_PARAMS_US => "strat_params_us",
    STRAT_MODEL_US => "strat_model_us",
);

/// Starts a stopwatch, but only when diagnostics are on.
///
/// Lazy on purpose: an ordinary run pays one relaxed atomic load and never reads the clock.
pub fn timer() -> Option<std::time::Instant> {
    is_enabled().then(std::time::Instant::now)
}

/// Adds a stopwatch's elapsed time to a microsecond counter.
pub fn record_us(counter: &AtomicU64, timer: Option<std::time::Instant>) {
    if let Some(timer) = timer {
        bump_by(counter, timer.elapsed().as_micros() as u64);
    }
}

#[derive(Clone, Debug)]
pub struct DiagRate {
    pub label: &'static str,
    pub hz: f64,
}

static FORCE_ON: AtomicBool = AtomicBool::new(false);
static GPU_FRAME_US_SUM: AtomicU64 = AtomicU64::new(0);
static GPU_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Return whether render diagnostics are on.
///
/// Two ways in: [`force_enable`], used by FireTest, which cannot be turned off again for the life
/// of the run; and `channels.render` in `cfg/diagnostics.toml` (or `MOON_RENDER_DIAG`), which is
/// live and can be flipped either way while the terminal runs. Without either, counters and
/// `logs/render_diag.log` stay inactive in every build profile. This cannot use
/// `cfg(debug_assertions)` because the development profile disables debug assertions to avoid the
/// DX12 validation layer.
fn enabled() -> bool {
    FORCE_ON.load(std::sync::atomic::Ordering::Relaxed) || moon_core::diagnostics::render()
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
/// Returns `None` while render diagnostics are off — see [`enabled`].
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

/// Write a sample to the application log and append it to `logs/render_diag.log` when possible.
///
/// `ctx` describes the sampling moment, including process/system CPU and open window/chart counts.
/// It is inserted after the diagnostic prefix so the log shows what was open at the measured load.
pub fn write_sample(elapsed_ms: f64, sample: &[DiagRate], ctx: &str) {
    let mut line = format_sample(elapsed_ms, sample);
    if !ctx.is_empty() {
        let head_end = line.find(']').map(|i| i + 1).unwrap_or(0);
        line.insert_str(head_end, &format!(" {ctx}"));
    }
    log::info!("{line}");
    moon_core::diagnostics::channel_line("render_diag.log", &line);
}
