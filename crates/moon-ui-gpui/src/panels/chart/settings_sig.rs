//! Effective chart settings and initial presentation cadence.

use super::*;

#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
fn monitor_refresh_hz() -> u32 {
    unsafe {
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..DEVMODEW::default()
        };
        if EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut mode).as_bool()
            && mode.dmDisplayFrequency > 1
        {
            mode.dmDisplayFrequency
        } else {
            60
        }
    }
}

#[cfg(not(windows))]
fn monitor_refresh_hz() -> u32 {
    60
}

pub(super) fn chart_bootstrap_present_rate_hz() -> f32 {
    let refresh = monitor_refresh_hz().clamp(30, 360);
    refresh as f32
}

pub(super) const DEBUG_HISTORY_FILL_SPAN_MS: i64 = 3_600_000;

/// Effective settings that wake a chart when its appearance changes.
#[derive(Clone)]
pub(super) struct ChartSettingsSig {
    /// Normalized app-wide book width; independent of per-tab appearance overrides.
    pub(super) order_book_width_px: f32,
    pub(super) theme: ChartTheme,
    pub(super) orders: OrdersStyleSet,
    pub(super) follow: bool,
    /// The panel's EFFECTIVE chart-drawing settings. In the signature because a panel following the
    /// global default is changed from a DIFFERENT entity — the popup writes `layout.chart_graphics`
    /// — and without this, an otherwise idle chart never re-renders, while `render` is the only
    /// place the value reaches the engine.
    ///
    /// Effective rather than global so that a panel with its OWN override is not woken by every
    /// edit of a default it does not follow. Its own edits arrive through `set_chart_graphics`.
    pub(super) chart_graphics: moon_core::config::ChartGraphicsCfg,
    /// The panel's EFFECTIVE candle settings, in the signature for exactly the same reason.
    pub(super) candle_view: moon_core::market::CandleViewCfg,
    /// The panel's EFFECTIVE caption labels, in the signature for exactly the same reason: a panel
    /// following `layout.chart_labels` learns of a ⧉ press in another window only through here.
    ///
    /// Behind an `Rc` because this is ALSO the value `render` hands the engine on every frame: one
    /// allocation, re-made only when the signature is rebuilt, instead of a deep copy per render.
    pub(super) chart_labels: std::rc::Rc<moon_core::config::ChartLabelsCfg>,
    /// A core's newly-measured clock offset, in the signature for the same reason as
    /// `chart_graphics`: an idle chart must learn about it the same way it learns about a
    /// chart-graphics edit from elsewhere, instead of waiting on some unrelated repaint.
    pub(super) report_axis: moon_core::db::ReportAxis,
}

impl PartialEq for ChartSettingsSig {
    /// Hand-written for ONE field: the captions are compared by handle first.
    ///
    /// This runs on every backend notification, per chart panel, and the configuration behind that
    /// handle is sixteen rows of eight captions. `Rc`'s own `PartialEq` compares the VALUES, so the
    /// pointer shortcut has to be spelled here.
    ///
    /// It does NOT fire on the notification path — `chart_settings_sig` mints a fresh handle every
    /// time, so the deep compare still runs there. It fires where the same signature is compared
    /// against itself, which is what a re-render that rebuilt nothing does.
    fn eq(&self, other: &Self) -> bool {
        self.order_book_width_px == other.order_book_width_px
            && self.theme == other.theme
            && self.orders == other.orders
            && self.follow == other.follow
            && self.chart_graphics == other.chart_graphics
            && self.candle_view == other.candle_view
            && (std::rc::Rc::ptr_eq(&self.chart_labels, &other.chart_labels)
                || self.chart_labels == other.chart_labels)
            && self.report_axis == other.report_axis
    }
}

/// Build the settings signature for a panel whose chart-graphics override is `graphics`.
///
/// Args:
///     backend: Shared backend holding the theme, order styles and the global default.
///     graphics: The panel's per-tab override, or `None` when it follows the global default.
///
/// Returns:
///     The signature to compare against the panel's stored one.
pub(super) fn chart_settings_sig(
    backend: &Backend,
    graphics: Option<moon_core::config::ChartGraphicsCfg>,
    candles: Option<moon_core::market::CandleViewCfg>,
    labels: Option<moon_core::config::ChartLabelsCfg>,
    kind: moon_core::config::ChartTabKind,
) -> ChartSettingsSig {
    let effective = backend.preview.as_ref().unwrap_or(&backend.config);
    ChartSettingsSig {
        order_book_width_px: moon_core::config::book_width::normalize(
            effective.order_book_width_px,
        ),
        theme: effective.chart_theme().clone(),
        orders: effective.orders.clone(),
        follow: backend.follow,
        // From `layout`, not from the previewed config: layout.toml has no Settings-window draft.
        // NORMALIZED, because this value is COMPARED: a hand-edited `nan` never equals itself and
        // would make every backend notification look like a settings change.
        chart_graphics: moon_chart::normalize_chart_graphics(
            graphics.unwrap_or_else(|| backend.layout.chart_graphics_for(kind)),
        ),
        candle_view: candles.unwrap_or_else(|| backend.layout.candle_view_for(kind)),
        // SANITIZED for the same reason graphics is normalized: this value is COMPARED, and a
        // hand-edited file can state a hole between captions or a size outside the drawable range —
        // repaired on read, it would differ from the stored one on every notification.
        chart_labels: {
            let mut cfg = labels.unwrap_or_else(|| backend.layout.chart_labels_for(kind).clone());
            cfg.sanitize();
            std::rc::Rc::new(cfg)
        },
        report_axis: backend.report_axis(crate::chartdx::axes::display_zone()),
    }
}
