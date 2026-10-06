//! Persisted setting bounds and their clamping rules.

/// Narrowest persisted Auto-workspace rail width in logical pixels.
pub const AUTO_WORKSPACE_RAIL_WIDTH_MIN: f32 = 52.0;
/// Widest persisted Auto-workspace rail width in logical pixels.
pub const AUTO_WORKSPACE_RAIL_WIDTH_MAX: f32 = 560.0;
/// First-run Auto-workspace rail width in logical pixels.
pub const AUTO_WORKSPACE_RAIL_WIDTH_DEFAULT: f32 = 340.0;

/// Floor for the Strategies tree's local text-size step. Zero, not negative: a negative step would
/// let the user re-create, as a supported setting, the sub-`t_caption` defect the step's own fix
/// pass corrects (`strategies/tree/moon.rs`).
pub const STRATEGIES_TREE_TEXT_STEP_MIN: f32 = 0.0;
/// Ceiling for the step. Four is where `fit_height`'s line-height term still leaves headroom over
/// its `ui()` term at every global Font-slider setting; higher pushes the row's UI-scaled chrome
/// (checkbox, disclosure caret) past reading as part of the same row.
pub const STRATEGIES_TREE_TEXT_STEP_MAX: f32 = 4.0;
/// Shipped step: the pane renders at exactly the theme base, unchanged by this setting until the
/// user raises it.
pub const STRATEGIES_TREE_TEXT_STEP_DEFAULT: f32 = 0.0;

/// Seconds a strategy-less figure-alert card stays on the Detects feed when the Alerts panel
/// duration has never been stored. MoonBot's `AlertConfig.KeepTime` default is 30; the unwired
/// stepper used to initialise to 20 and reset on every rebuild.
pub const ALERT_DURATION_S_DEFAULT: u32 = 30;
/// Narrowest duration the Alerts panel stepper will store, in seconds.
pub const ALERT_DURATION_S_MIN: u32 = 5;
/// Widest duration the Alerts panel stepper will store, in seconds.
pub const ALERT_DURATION_S_MAX: u32 = 600;
/// Times a figure-alert sound plays when the Alerts panel repeat has never been stored.
pub const ALERT_REPEAT_DEFAULT: u32 = 2;
/// Most times the Alerts panel will enqueue one figure-alert clip.
pub const ALERT_REPEAT_MAX: u32 = 20;

/// Clamp a persisted or runtime Auto rail width to the globally usable range.
///
/// Args:
///     width: Logical-pixel width from persistence or a resize event.
///
/// Returns:
///     A finite width within the supported range, or the first-run default for non-finite input.
pub fn clamp_auto_workspace_rail_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(AUTO_WORKSPACE_RAIL_WIDTH_MIN, AUTO_WORKSPACE_RAIL_WIDTH_MAX)
    } else {
        AUTO_WORKSPACE_RAIL_WIDTH_DEFAULT
    }
}

/// Seconds a strategy-less figure-alert card stays on the Detects feed.
///
/// `None` is an upgrade from a `layout.toml` that never stored the field: that file must not
/// flash the card for a second, so the MoonBot `KeepTime` default stands. A stored value is
/// clamped to the stepper range, including a hand-edited zero, which would otherwise expire the
/// card on the ingest pass that built it.
///
/// Args:
///     stored: Value from [`WindowLayout::alert_duration_s`].
///
/// Returns:
///     A duration in `ALERT_DURATION_S_MIN..=ALERT_DURATION_S_MAX`.
pub fn resolve_alert_duration_s(stored: Option<u32>) -> u32 {
    stored
        .unwrap_or(ALERT_DURATION_S_DEFAULT)
        .clamp(ALERT_DURATION_S_MIN, ALERT_DURATION_S_MAX)
}

/// How many times the detect player should enqueue one figure-alert clip.
///
/// `None` is the panel's shipped repeat. Zero is a stored choice (play nothing); values above
/// the stepper ceiling clamp so a hand-edited layout cannot flood the sound queue.
///
/// Args:
///     stored: Value from [`WindowLayout::alert_repeat`].
///
/// Returns:
///     A play count in `0..=ALERT_REPEAT_MAX`.
pub fn resolve_alert_repeat(stored: Option<u32>) -> u32 {
    stored.unwrap_or(ALERT_REPEAT_DEFAULT).min(ALERT_REPEAT_MAX)
}

/// Clamp a persisted or runtime Strategies tree text step to the supported integer range.
///
/// The `round()` is load-bearing: the stepper control only ever emits integers, and a
/// hand-written fractional value must not produce a half-step the UI cannot represent or
/// return to.
///
/// Args:
///     value: Step from persistence or a stepper change event.
///
/// Returns:
///     A whole-number step within `STRATEGIES_TREE_TEXT_STEP_MIN..=STRATEGIES_TREE_TEXT_STEP_MAX`,
///     or the shipped default for non-finite input.
pub fn clamp_strategies_tree_text_step(value: f32) -> f32 {
    if value.is_finite() {
        value
            .round()
            .clamp(STRATEGIES_TREE_TEXT_STEP_MIN, STRATEGIES_TREE_TEXT_STEP_MAX)
    } else {
        STRATEGIES_TREE_TEXT_STEP_DEFAULT
    }
}
