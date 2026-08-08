//! Which script to run and every knob it accepts.
//!
//! Only the script name is a CLI argument; everything else is an environment variable, so a run
//! can be re-aimed (market, storm length, order size) without changing the command the docs and CI
//! spell out. Every knob is validated here rather than at the point of use: a nonsensical value
//! falls back to the default instead of reaching a stage that would fail for the wrong reason.

use std::time::Duration;

const DEFAULT_MARKET: &str = "BTCUSDT";
const DEFAULT_MOUSE_HZ: f64 = 5000.0;
const DEFAULT_STORM: Duration = Duration::from_millis(5000);
const STATIC_TEXT_LABELS: usize = 10_000;
/// Default length of the arrival-flash window. The same 5000 ms the idle floor measures over, so
/// the two phases are compared over equal windows and one cannot be quieter merely for being
/// shorter.
const DEFAULT_FLASH: Duration = Duration::from_millis(5000);

/// The scenario a run executes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Script {
    ChartSmoke,
    OrderCancelLag,
}

/// Everything a run needs beyond the live app: which market, how hard to storm, and the opt-in
/// order-path parameters.
#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(super) script: Script,
    pub(super) market: String,
    pub(super) storm: Duration,
    pub(super) mouse_hz: f64,
    pub(super) text_labels: usize,
    /// How long the arrival-flash stage keeps every live chart flashing.
    pub(super) flash: Duration,
    pub(super) order_cancel_lag: bool,
    pub(super) order_cancel_size: Option<f64>,
    pub(super) order_cancel_quote_size: Option<f64>,
    pub(super) order_cancel_price_mult: f64,
    pub(super) order_cancel_max_display_lag_ms: f64,
}

impl Config {
    /// Parse `--debug-script <name>` out of the process arguments and read the environment knobs.
    ///
    /// Returns `Ok(None)` for a normal app launch with no `--debug-script`, and an error only for
    /// a malformed or unknown script name.
    pub(crate) fn from_args<I>(args: I) -> anyhow::Result<Option<Self>>
    where
        I: IntoIterator<Item = String>,
    {
        let mut args = args.into_iter();
        let mut script = None;
        while let Some(arg) = args.next() {
            if arg != "--debug-script" {
                continue;
            }
            let Some(value) = args.next() else {
                anyhow::bail!("--debug-script requires a script name");
            };
            script = Some(match value.as_str() {
                "chart-smoke" => Script::ChartSmoke,
                "order-cancel-lag" => Script::OrderCancelLag,
                other => anyhow::bail!("unknown --debug-script {other:?}"),
            });
        }

        let Some(script) = script else {
            return Ok(None);
        };
        let market = std::env::var("MOON_FIRETEST_MARKET")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MARKET.to_string());
        let mouse_hz = std::env::var("MOON_FIRETEST_MOUSE_HZ")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 100.0)
            .unwrap_or(DEFAULT_MOUSE_HZ);
        let storm = std::env::var("MOON_FIRETEST_STORM_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .filter(|v| *v >= Duration::from_millis(1000))
            .unwrap_or(DEFAULT_STORM);
        let flash = std::env::var("MOON_FIRETEST_FLASH_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .filter(|v| *v >= Duration::from_millis(1000))
            .unwrap_or(DEFAULT_FLASH);
        let text_labels = std::env::var("MOON_FIRETEST_TEXT_LABELS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(STATIC_TEXT_LABELS);
        let order_cancel_lag =
            script_enables_order_cancel(script, env_flag("MOON_FIRETEST_ORDER_CANCEL"));
        let order_cancel_size = std::env::var("MOON_FIRETEST_ORDER_SIZE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0);
        let order_cancel_quote_size = std::env::var("MOON_FIRETEST_ORDER_QUOTE_SIZE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0);
        let order_cancel_price_mult = std::env::var("MOON_FIRETEST_ORDER_PRICE_MULT")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0 && *v < 1.0)
            .unwrap_or(0.98);
        let order_cancel_max_display_lag_ms =
            std::env::var("MOON_FIRETEST_ORDER_CANCEL_MAX_DISPLAY_MS")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(750.0);
        Ok(Some(Self {
            script,
            market,
            storm,
            mouse_hz,
            text_labels,
            flash,
            order_cancel_lag,
            order_cancel_size,
            order_cancel_quote_size,
            order_cancel_price_mult,
            order_cancel_max_display_lag_ms,
        }))
    }

    /// Whether this run is the narrow order-only script rather than the full perf run.
    pub(super) fn is_order_cancel_script(&self) -> bool {
        matches!(self.script, Script::OrderCancelLag)
    }
}

/// Whether the real place/cancel order stage runs: the narrow script always enables it, the
/// general run only on an explicit opt-in, because the stage sends a real trading command.
fn script_enables_order_cancel(script: Script, env_enabled: bool) -> bool {
    env_enabled || matches!(script, Script::OrderCancelLag)
}

/// Read a boolean environment flag; anything other than an explicit truthy value is `false`.
fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}
