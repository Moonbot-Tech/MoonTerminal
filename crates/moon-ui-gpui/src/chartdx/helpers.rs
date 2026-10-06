//! Shared chart range, scale-badge, diagnostic and signature helpers.

use super::*;

/// Minimum half-width of the visible auto-focus band around the order-book midpoint when there are
/// no trades, expressed as a price fraction. The band always includes best bid and ask but is never
/// narrower than +/-0.5%, preventing absurd zoom into a tight spread while showing both sides of a
/// wide HIP-3 spread. Once trades arrive, ticks drive the range.
pub(super) const BOOK_FOCUS_HALF_FRAC: f32 = 0.005;

pub(super) fn union_range(a: Option<(f32, f32)>, b: Option<(f32, f32)>) -> Option<(f32, f32)> {
    match (a, b) {
        (Some((alo, ahi)), Some((blo, bhi))) => Some((alo.min(blo), ahi.max(bhi))),
        (Some(r), None) | (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

/// Return the whole-number percentage of the current visible Y range relative to price for the
/// scale badge beside the corner label, or `None` when there is no price to measure against.
///
/// The measured window in every price mode — Auto, a pinned step, and manual Y from drag,
/// right-click zoom or comparison lock — including when it equals the pinned step (#867). The
/// magnifier states what was PICKED, the badge what is ON SCREEN; repeating `20%` in both places is
/// the point, and hiding it left a hole exactly at the step while its neighbours printed.
pub(super) fn scale_badge_pct(view: &moon_chart::view::ChartView) -> Option<i32> {
    // Measured against the instrument's price, not the centre of the viewport: dragging the chart
    // vertically moves that centre without touching the zoom, and reporting a changed scale for a
    // scale that did not change is what this badge is least allowed to do.
    Some(view.visible_scale_percent()?.round() as i32)
}

/// Return the whole seconds the plot spans horizontally, for the time-scale badge, or `None` when
/// the plot has no width to measure.
///
/// The FULL plot width, the empty future margin right of the live edge included — the answer to
/// "what fits on the chart", read the same on a live chart and a paused one. Taken from
/// [`moon_chart::view::ChartView::visible_x`], the one source of X geometry the axis and the tick
/// culling already read, so the badge cannot disagree with the labels under it.
pub(super) fn time_scale_secs(view: &moon_chart::view::ChartView, plot_w: f32) -> Option<i64> {
    let (_, window_ms) = view.visible_x(plot_w);
    let secs = f64::from(window_ms) / 1_000.0;
    (plot_w > 0.0 && secs.is_finite() && secs >= 0.0).then(|| secs.round() as i64)
}

/// Whether the market channel is on (`channels.markets` in `cfg/diagnostics.toml`, or
/// `MOON_MARKET_DIAG`/`MOON_RENDER_DIAG`). Live, so it follows an edit without a restart.
pub(super) fn chart_market_diag_enabled() -> bool {
    moon_core::diagnostics::markets()
}

pub(super) fn chart_market_diag_due(key: impl Into<String>) -> bool {
    if !chart_market_diag_enabled() {
        return false;
    }
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let key = key.into();
    let now = Instant::now();
    let mut last = LAST
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("chart market diag lock poisoned");
    match last.get(&key).copied() {
        // The configured floor, not a literal: this throttle and `market::source`'s serve the same
        // channel, and a second copy of the number would ignore `limits.market_trace_min_interval_ms`.
        Some(prev)
            if now.duration_since(prev) < moon_core::diagnostics::market_trace_min_interval() =>
        {
            false
        }
        _ => {
            last.insert(key, now);
            true
        }
    }
}

pub(super) fn chart_market_diag(msg: impl std::fmt::Display) {
    if chart_market_diag_enabled() {
        log::info!("[chart_market_diag] {msg}");
    }
}

pub(super) fn mix_sig(mut sig: u64, value: u64) -> u64 {
    sig ^= value;
    sig = sig.wrapping_mul(0x100000001b3);
    sig
}

pub(super) fn str_sig(s: &str) -> u64 {
    let mut sig = 0xcbf29ce484222325;
    for b in s.bytes() {
        sig = mix_sig(sig, b as u64);
    }
    sig
}
