//! Each core's PriceDown step lag, calibrated off the archived Exit lines of the rows an axis
//! load read, and held for the whole process: every path that replays a row — the load, the
//! fetch job, the startup autoload — reads it from here, so a row replayed anywhere steps on
//! the clock the last load found for its core. Only the load calibrates — a fetch walks one
//! cluster at a time, too few lines to take a median from — so a replay before the first load
//! of the axis in this process runs its core on the plain schedule; the load's own stage C
//! replays every row after calibrating, which is what the table and the variants then read.
//! The calibration itself is `moon_core::db::tuner::ticks::calibrate`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use moon_core::db::tuner::strategy_values_at;
use moon_core::db::tuner::ticks::{calibrate, params};

use super::load::ArchivedLines;
use super::state::DealRow;

/// Core uid → its step lag, milliseconds.
static LAGS: OnceLock<Mutex<HashMap<u64, f64>>> = OnceLock::new();

fn lags() -> std::sync::MutexGuard<'static, HashMap<u64, f64>> {
    LAGS.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Calibrate every core of `rows` off the archived Exit lines in `traces`, each row on the
/// strategy parameters as of its buy. A core with too few samples in these rows keeps what an
/// earlier load found for it; a core never calibrated steps on the plain schedule.
///
/// Args:
///     rows: The rows of the load.
///     traces: Their archived lines, by `reportuid`.
///     defaults: The strategy-field defaults of the live schema.
pub(super) fn calibrate_from(
    rows: &[DealRow],
    traces: &HashMap<i64, ArchivedLines>,
    defaults: &HashMap<String, f64>,
) {
    let keys = params::param_keys();
    let mut samples: HashMap<u64, Vec<i64>> = HashMap::new();
    for row in rows {
        // Two steps at least, after the take: nothing shorter holds a pair of steps.
        let Some(points) = traces
            .get(&row.deal.report_uid)
            .and_then(|lines| lines.exit_points.as_deref())
            .filter(|points| points.len() >= 3)
        else {
            continue;
        };
        let Some(values) = strategy_values_at(
            row.deal.strategy_id,
            Some(row.deal.core_uid),
            row.deal.buy_ms,
            &keys,
        ) else {
            continue;
        };
        let exit = params::exit_params(&params::StrategyValues {
            values: &values,
            defaults,
        });
        samples
            .entry(row.deal.core_uid)
            .or_default()
            .extend(calibrate::step_lag_samples(&row.deal, &exit, points));
    }
    let mut store = lags();
    for (core, mut core_samples) in samples {
        if let Some(lag) = calibrate::median_step_lag(&mut core_samples) {
            store.insert(core, lag);
        }
    }
}

/// The step lag of `core`, milliseconds; 0 when no load has calibrated it.
pub(super) fn step_lag_of(core: u64) -> f64 {
    lags().get(&core).copied().unwrap_or(0.0)
}
