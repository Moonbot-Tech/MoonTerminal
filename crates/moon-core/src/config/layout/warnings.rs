//! Persisted core-warning axes and thresholds.

use super::*;

/// Chart visibility, alert sound, and detection thresholds per warning axis. Defaults are the
/// operator-tuned starting point (CPU 70%/5s, memory +15%/30s, latency ×2 yellow / ×10 red over a
/// 15 s baseline / 3 s hold); the engine's `WarnTuning::default()` constants are only a
/// pre-config fallback, so a fresh `layout.toml` opens on these numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WarnParams {
    /// Sustained system-CPU axis.
    pub cpu: CpuWarn,
    /// Rising process-memory axis.
    pub mem: MemWarn,
    /// Dropped-core connectivity axis (no thresholds, just chart + sound).
    pub conn: ConnWarn,
    /// Client↔core ping axis.
    pub ping: LatWarn,
    /// Core→exchange ping axis.
    pub exch: LatWarn,
    /// Expiring exchange API-key axis.
    pub api: ApiWarn,
    /// Exhausting API-request-quota axis.
    #[serde(default)]
    pub api_quota: ApiQuotaWarn,
}

/// CPU-warning parameters: drawn-on-chart, sound, sustained-CPU percent, and the sustain seconds.
/// (The averaging window stays a fixed internal 3 s, not a user knob.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuWarn {
    #[serde(default = "def_true")]
    pub chart: bool,
    pub sound: Option<String>,
    /// Machine CPU percent (averaged) that counts toward the warning.
    pub pct: u8,
    /// Consecutive high seconds before it fires.
    pub hold: u8,
}
impl Default for CpuWarn {
    fn default() -> Self {
        Self {
            chart: true,
            sound: None,
            pct: 70,
            hold: 5,
        }
    }
}

/// Memory-growth parameters: percent rise above the window minimum, and the observation window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemWarn {
    #[serde(default = "def_true")]
    pub chart: bool,
    pub sound: Option<String>,
    /// Percent rise above the window minimum that flags growth.
    pub pct: u8,
    /// Observation window in seconds.
    pub window: u16,
}
impl Default for MemWarn {
    fn default() -> Self {
        Self {
            chart: true,
            sound: None,
            pct: 15,
            window: 30,
        }
    }
}

/// Connectivity parameters: chart visibility and sound only (the drop rule has no numeric threshold).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnWarn {
    #[serde(default = "def_true")]
    pub chart: bool,
    pub sound: Option<String>,
}
impl Default for ConnWarn {
    fn default() -> Self {
        Self {
            chart: true,
            sound: None,
        }
    }
}

/// Latency-axis parameters (ping and exch): the baseline MULTIPLIER at which each colour/warning
/// fires, the baseline window, and the sustain seconds. Purely relative — a latency warns when it
/// reaches `red ×` its own rolling mean (default yellow ×2, red ×10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LatWarn {
    #[serde(default = "def_true")]
    pub chart: bool,
    pub sound: Option<String>,
    /// Yellow colour at this multiple of the baseline (e.g. 2 = ×2).
    pub yellow: u8,
    /// Red colour AND warning at this multiple of the baseline (e.g. 10 = ×10).
    pub red: u8,
    /// Baseline (rolling-mean) window in seconds.
    pub window: u16,
    /// Consecutive above-red seconds before it fires.
    pub hold: u8,
}
impl Default for LatWarn {
    fn default() -> Self {
        Self {
            chart: true,
            sound: None,
            yellow: 2,
            red: 10,
            window: 15,
            hold: 3,
        }
    }
}

/// Largest API-key warning horizon offered and honoured: the alert popup's stepper range, and the
/// ceiling the engine clamps a hand-edited `layout.toml` to. One constant so the two cannot drift.
pub const API_WARN_MAX_DAYS: u16 = 90;

/// Expiring-API-key parameters: the alert sound and how many days ahead the warning starts.
///
/// No `chart` field, unlike every other axis: this one has no per-second history, so a chart badge
/// would open a card with nothing to draw in it. The warning is a Core Status state, not a moment
/// in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiWarn {
    pub sound: Option<String>,
    /// The warning is on from this many days before expiration and stays on until the key is
    /// replaced. `0` warns on the key's LAST DAY and after — not only once it has expired, because
    /// the count is in whole days and reaches zero while up to a day of life remains.
    pub days: u16,
}
impl Default for ApiWarn {
    fn default() -> Self {
        Self {
            sound: None,
            days: 7,
        }
    }
}

/// Smallest API-request quota the warning can be armed at, and the popup stepper's ceiling.
///
/// The bound is `u16` rather than a round number because two structures downstream are `u16`: the
/// alert popup's stepper (`Param`) and the episode's `peak`. Raising it past that would silently
/// truncate the number an episode records about itself.
pub const API_QUOTA_WARN_MAX: u16 = u16::MAX;

/// Exhausting API-request-quota parameters: the alert sound and the quota the warning starts at.
///
/// Today only HyperLiquid cores report a quota, and the value is address-level rather than
/// per-market. No `chart` field, for the same reason as [`ApiWarn`]: the quota is a standing state
/// the terminal receives every few minutes, not a per-second series a badge could draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiQuotaWarn {
    pub sound: Option<String>,
    /// The warning is on while the remaining quota is at or below this many requests, and clears
    /// when the quota climbs back above it. Unlike the day counts of [`ApiWarn`] this number is a
    /// COUNT of requests: a HyperLiquid address earns quota with volume, so it moves both ways.
    pub min: u16,
}
impl Default for ApiQuotaWarn {
    fn default() -> Self {
        Self {
            sound: None,
            min: 5000,
        }
    }
}
