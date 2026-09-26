//! The model's settings (`moon_core::db::tuner::ticks::ModelSettings`), held for the whole
//! process like the step lags (`lags.rs`): every path that replays a row — the load, the fetch
//! job, the startup autoload — and the variant columns and the search read them from here, so a
//! row's ✓ never depends on which path replayed it. Seeded from the saved layout when an
//! analytics view opens; written by the model settings popover.
//!
//! Also the popover's field list: which setting, under which caption, in which unit.

use std::sync::{Mutex, OnceLock};

use moon_core::db::tuner::ticks::ModelSettings;

/// The settings in force.
static MODEL: OnceLock<Mutex<ModelSettings>> = OnceLock::new();

fn store() -> std::sync::MutexGuard<'static, ModelSettings> {
    MODEL
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The settings every replay runs on, sanitized.
pub(in crate::analytics) fn current() -> ModelSettings {
    *store()
}

/// Put `settings` in force; answers whether anything changed.
pub(in crate::analytics) fn replace(settings: ModelSettings) -> bool {
    let settings = settings.sanitized();
    let mut slot = store();
    let changed = *slot != settings;
    *slot = settings;
    changed
}

/// Which part of the model a setting belongs to — the popover's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Section {
    Entry,
    Take,
    Stop,
    Line,
    Verdict,
}

impl Section {
    /// The locale key of the section's heading.
    pub(super) fn title_key(self) -> &'static str {
        match self {
            Section::Entry => "analytics.ticks.model_sec_entry",
            Section::Take => "analytics.ticks.model_sec_take",
            Section::Stop => "analytics.ticks.model_sec_stop",
            Section::Line => "analytics.ticks.model_sec_line",
            Section::Verdict => "analytics.ticks.model_sec_verdict",
        }
    }
}

/// One numeric setting of the popover.
pub(super) struct ModelField {
    /// The input box's cache key and element id suffix.
    pub(super) id: &'static str,
    /// Locale keys of the caption and its tooltip.
    pub(super) label: &'static str,
    pub(super) tip: &'static str,
    pub(super) section: Section,
    /// Whole milliseconds (`true`) or a per-cent tolerance.
    pub(super) whole: bool,
    pub(super) get: fn(&ModelSettings) -> f64,
    pub(super) set: fn(&mut ModelSettings, f64),
}

/// Every numeric setting, in the popover's order. The entry method is not here: it changes how
/// a variant is replayed, never the ✓ of the fact, and sits with the search settings.
pub(super) const MODEL_FIELDS: &[ModelField] = &[
    ModelField {
        id: "latency",
        label: "analytics.ticks.model_latency",
        tip: "analytics.ticks.model_latency_tip",
        section: Section::Entry,
        whole: true,
        get: |m| m.latency_ms,
        set: |m, v| m.latency_ms = v,
    },
    ModelField {
        id: "replace-window",
        label: "analytics.ticks.model_replace_window",
        tip: "analytics.ticks.model_replace_window_tip",
        section: Section::Entry,
        whole: true,
        get: |m| m.replace_window_ms as f64,
        set: |m, v| m.replace_window_ms = v as i64,
    },
    ModelField {
        id: "shift-window",
        label: "analytics.ticks.model_shift_window",
        tip: "analytics.ticks.model_shift_window_tip",
        section: Section::Entry,
        whole: true,
        get: |m| m.shift_window_ms as f64,
        set: |m, v| m.shift_window_ms = v as i64,
    },
    ModelField {
        id: "pre-spike",
        label: "analytics.ticks.model_pre_spike",
        tip: "analytics.ticks.model_pre_spike_tip",
        section: Section::Take,
        whole: true,
        get: |m| m.pre_spike_lookback_ms as f64,
        set: |m, v| m.pre_spike_lookback_ms = v as i64,
    },
    ModelField {
        id: "ticker",
        label: "analytics.ticks.model_ticker",
        tip: "analytics.ticks.model_ticker_tip",
        section: Section::Stop,
        whole: true,
        get: |m| m.ticker_period_ms as f64,
        set: |m, v| m.ticker_period_ms = v as i64,
    },
    ModelField {
        id: "series",
        label: "analytics.ticks.model_series",
        tip: "analytics.ticks.model_series_tip",
        section: Section::Stop,
        whole: true,
        get: |m| m.series_tick_ms as f64,
        set: |m, v| m.series_tick_ms = v as i64,
    },
    ModelField {
        id: "step-floor",
        label: "analytics.ticks.model_step_floor",
        tip: "analytics.ticks.model_step_floor_tip",
        section: Section::Line,
        whole: true,
        get: |m| m.step_floor_ms as f64,
        set: |m, v| m.step_floor_ms = v as i64,
    },
    ModelField {
        id: "pump-lag",
        label: "analytics.ticks.model_pump_lag",
        tip: "analytics.ticks.model_pump_lag_tip",
        section: Section::Line,
        whole: true,
        get: |m| m.pump_move_lag_ms as f64,
        set: |m, v| m.pump_move_lag_ms = v as i64,
    },
    ModelField {
        id: "pump-peak",
        label: "analytics.ticks.model_pump_peak",
        tip: "analytics.ticks.model_pump_peak_tip",
        section: Section::Line,
        whole: true,
        get: |m| m.pump_peak_lookback_ms as f64,
        set: |m, v| m.pump_peak_lookback_ms = v as i64,
    },
    ModelField {
        id: "point-time",
        label: "analytics.ticks.model_point_time",
        tip: "analytics.ticks.model_point_time_tip",
        section: Section::Verdict,
        whole: true,
        get: |m| m.point_time_ms as f64,
        set: |m, v| m.point_time_ms = v as i64,
    },
    ModelField {
        id: "book-stop-time",
        label: "analytics.ticks.model_book_stop_time",
        tip: "analytics.ticks.model_book_stop_time_tip",
        section: Section::Verdict,
        whole: true,
        get: |m| m.book_stop_time_ms as f64,
        set: |m, v| m.book_stop_time_ms = v as i64,
    },
    ModelField {
        id: "price",
        label: "analytics.ticks.model_price",
        tip: "analytics.ticks.model_price_tip",
        section: Section::Verdict,
        whole: false,
        get: |m| m.price_pct,
        set: |m, v| m.price_pct = v,
    },
    ModelField {
        id: "stop-price",
        label: "analytics.ticks.model_stop_price",
        tip: "analytics.ticks.model_stop_price_tip",
        section: Section::Verdict,
        whole: false,
        get: |m| m.stop_price_pct,
        set: |m, v| m.stop_price_pct = v,
    },
    ModelField {
        id: "fill-better",
        label: "analytics.ticks.model_fill_better",
        tip: "analytics.ticks.model_fill_better_tip",
        section: Section::Verdict,
        whole: false,
        get: |m| m.fill_improvement_pct,
        set: |m, v| m.fill_improvement_pct = v,
    },
];

/// A setting's value as its box shows it: whole milliseconds without a fraction, a tolerance
/// with as many decimals as it needs.
pub(super) fn field_text(field: &ModelField, settings: &ModelSettings) -> String {
    let value = (field.get)(settings);
    if field.whole {
        format!("{value:.0}")
    } else {
        let text = format!("{value:.4}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// A typed value, read as the model reads it: a number with a comma or a point, never negative;
/// `None` for anything else, which leaves the setting as it was.
pub(super) fn parse_field(text: &str) -> Option<f64> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

#[cfg(test)]
mod tests;
