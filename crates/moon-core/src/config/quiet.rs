//! Quiet mode ("do not disturb" / sleep): a terminal-wide switch that silences detect and
//! core-status alert sounds, either on a schedule or on demand, with explicit bypasses for the
//! events the operator must still be woken by.
//!
//! Every wall-clock field here is MINUTES SINCE LOCAL MIDNIGHT in the zone of the header clock —
//! the one zone the terminal shows anywhere. This module is deliberately zone-free: the caller
//! converts "now" into a minute-of-day and passes it in, so the schedule logic is a pure function
//! that can be tested without a clock.
//!
//! Quiet mode silences SOUND ONLY. Detect cards, chart tabs, warning episodes and badges keep
//! working, so a night spent asleep is still fully visible in the morning.

use serde::{Deserialize, Serialize};

/// Minutes in a day, the modulus for every wall-clock value in this module.
pub const DAY_MINUTES: u16 = 1440;

/// Per-axis bypass switches for the core-status warning sounds.
///
/// A `true` axis keeps sounding THROUGH quiet mode. The axes mirror
/// `WarnAxesCfg`/`WarnParams`: a bypass is meaningless for an axis that is switched off entirely,
/// and the backend checks both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietWarnBypass {
    /// Sustained machine system-CPU warning.
    pub cpu: bool,
    /// Rising process-memory warning.
    pub mem: bool,
    /// Dropped-core connectivity warning — "the core went away", the one alarm that is worth
    /// waking up for by default.
    pub conn: bool,
    /// Sustained client↔core ping warning.
    pub ping: bool,
    /// Sustained core→exchange order-API latency warning.
    pub exch: bool,
    /// Expiring exchange API-key warning.
    pub api: bool,
}

impl Default for QuietWarnBypass {
    /// Only the dropped-core axis wakes the operator; the rest can wait until morning.
    fn default() -> Self {
        Self {
            cpu: false,
            mem: false,
            conn: true,
            ping: false,
            exch: false,
            api: false,
        }
    }
}

/// Terminal-wide quiet-mode configuration and its persisted runtime state.
///
/// The runtime half ([`Self::manual_on`], [`Self::manual_until_ms`], [`Self::wake_override`]) is
/// persisted on purpose: a terminal restarted at 3 a.m. must come back asleep, and a restart right
/// after a manual "wake me up now" must not immediately fall asleep again inside the same window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietCfg {
    /// Quiet mode switched on by hand. Outlives the schedule window and is cleared at
    /// [`Self::manual_until_ms`] when a schedule is configured.
    pub manual_on: bool,
    /// Unix milliseconds at which a manual sleep ends — the next [`Self::to_min`] after the switch
    /// was flipped, or `None` when no schedule defines an end and it lasts until switched off.
    ///
    /// An ABSOLUTE instant rather than a time of day, because the terminal is not always running:
    /// a wall-clock rule can only be applied by a process that was awake to see the minute pass,
    /// so a manual sleep started at 23:00 and left overnight with the app closed would still be
    /// silencing everything the next afternoon. An instant is simply compared to "now" at startup.
    pub manual_until_ms: Option<i64>,
    /// The operator woke the terminal up by hand INSIDE a schedule window; suppresses the
    /// schedule until that window ends, so the next night sleeps again without being re-armed.
    pub wake_override: bool,
    /// Whether the from/to window is in effect at all.
    pub schedule_on: bool,
    /// Window start, minutes since local midnight.
    pub from_min: u16,
    /// Window end, minutes since local midnight. Equal to `from_min` means "no window".
    pub to_min: u16,
    /// Core-status warning axes that keep sounding while asleep.
    pub warn_bypass: QuietWarnBypass,
    /// Whether drawn-figure alert triggers (`DETECT_KIND_ALERT`) keep sounding while asleep.
    pub figure_alerts_bypass: bool,
    /// `AddToChart` tab numbers whose detects keep sounding while asleep — the strategies worth
    /// waking up for. Empty means "nothing bypasses by chart number".
    pub chart_bypass: Vec<u32>,
}

impl Default for QuietCfg {
    /// Off, with a plausible night window pre-filled so enabling the schedule needs no typing.
    fn default() -> Self {
        Self {
            manual_on: false,
            manual_until_ms: None,
            wake_override: false,
            schedule_on: false,
            from_min: 23 * 60,
            to_min: 7 * 60,
            warn_bypass: QuietWarnBypass::default(),
            figure_alerts_bypass: false,
            chart_bypass: Vec::new(),
        }
    }
}

impl QuietCfg {
    /// Window start normalized into `0..DAY_MINUTES`, so a hand-edited file cannot escape the day.
    pub fn from_min_norm(&self) -> u16 {
        self.from_min % DAY_MINUTES
    }

    /// Window end normalized into `0..DAY_MINUTES`.
    pub fn to_min_norm(&self) -> u16 {
        self.to_min % DAY_MINUTES
    }

    /// Whether the SCHEDULE alone would have the terminal asleep at `now_min`.
    pub fn scheduled_at(&self, now_min: u16) -> bool {
        self.schedule_on && in_window(now_min, self.from_min_norm(), self.to_min_norm())
    }

    /// Whether the terminal is asleep at `now_min`, manual switch and wake-override included.
    pub fn sleeping_at(&self, now_min: u16) -> bool {
        if self.manual_on {
            return true;
        }
        self.scheduled_at(now_min) && !self.wake_override
    }

    /// Apply one click of the header toggle at `now_min`.
    ///
    /// Awake → asleep by hand, ending at `manual_until_ms`. Asleep → awake, and if the sleep came
    /// from the schedule the wake-override is armed so this window stays awake without disarming
    /// the schedule itself.
    ///
    /// Args:
    ///     now_min: Current minute of day.
    ///     manual_until_ms: Instant the manual sleep should end (the next `to_min`), or `None` for
    ///         "until switched off" — the caller resolves it because only it owns the time zone.
    pub fn toggle_at(&mut self, now_min: u16, manual_until_ms: Option<i64>) {
        if self.sleeping_at(now_min) {
            self.manual_on = false;
            self.manual_until_ms = None;
            self.wake_override = self.scheduled_at(now_min);
        } else {
            self.manual_on = true;
            // Only a configured window defines an end; without one the switch is the only way out.
            self.manual_until_ms = self
                .schedule_on
                .then_some(manual_until_ms)
                .flatten()
                .filter(|_| self.from_min_norm() != self.to_min_norm());
            self.wake_override = false;
        }
    }

    /// Advance persisted runtime state to `now_ms` / `now_min`.
    ///
    /// Two transitions live here: a manual sleep expires at its deadline (the "act until the
    /// configured off-time" rule), and a wake-override is spent as soon as its window is over.
    /// Both are stated as comparisons against the CURRENT instant rather than as edges, so a
    /// transition the terminal was not running for still takes effect on the next start.
    ///
    /// Args:
    ///     now_ms: Current Unix milliseconds.
    ///     now_min: Current minute of day.
    ///
    /// Returns:
    ///     Whether any persisted field changed, so the caller can save and repaint only then.
    pub fn tick(&mut self, now_ms: i64, now_min: u16) -> bool {
        let mut changed = false;
        if self.manual_on && self.manual_until_ms.is_some_and(|until| now_ms >= until) {
            self.manual_on = false;
            self.manual_until_ms = None;
            changed = true;
        }
        // The override belongs to ONE window. Clearing it the moment the window is over is what
        // makes tomorrow night sleep again on its own.
        if self.wake_override && !self.scheduled_at(now_min) {
            self.wake_override = false;
            changed = true;
        }
        changed
    }

    /// Whether one arriving detect keeps its sound while asleep.
    ///
    /// Args:
    ///     is_alert: Whether this is a drawn-figure alert trigger rather than a strategy detect.
    ///     add_to_chart: The detect's `AddToChart` tab number (`0` = none).
    ///
    /// Returns:
    ///     Whether the sound survives quiet mode.
    pub fn allows_detect_sound(&self, is_alert: bool, add_to_chart: u32) -> bool {
        if is_alert && self.figure_alerts_bypass {
            return true;
        }
        add_to_chart != 0 && self.chart_bypass.contains(&add_to_chart)
    }
}

/// Whether `now_min` falls inside the half-open window `[from_min, to_min)`, wrapping midnight.
///
/// `from == to` is an EMPTY window, not a whole day: the two fields start out equal in a
/// hand-cleared config, and reading that as "asleep forever" would silence the terminal with no
/// visible cause.
pub fn in_window(now_min: u16, from_min: u16, to_min: u16) -> bool {
    if from_min == to_min {
        return false;
    }
    if from_min < to_min {
        now_min >= from_min && now_min < to_min
    } else {
        now_min >= from_min || now_min < to_min
    }
}

/// Minutes from `now_min` until the next occurrence of `target_min`, always in `1..=DAY_MINUTES`.
///
/// Strictly in the future: asked at exactly the target minute it answers a full day, which is what
/// "the NEXT 07:00" means to someone switching sleep on at 07:00.
pub fn minutes_until(now_min: u16, target_min: u16) -> u16 {
    let delta = (target_min + DAY_MINUTES - now_min) % DAY_MINUTES;
    if delta == 0 {
        DAY_MINUTES
    } else {
        delta
    }
}

/// Parse an operator-typed time of day into minutes since midnight.
///
/// Accepts `23:00`, `23.00`, `23 00`, `2300` and `23`, because all five are what people type into
/// a two-character-wide field. Anything out of range answers `None` and the caller keeps the old
/// value rather than storing a nonsense window.
pub fn parse_hhmm(text: &str) -> Option<u16> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let split: Vec<&str> = text
        .split([':', '.', ' ', '-'])
        .filter(|s| !s.is_empty())
        .collect();
    let (hh, mm) = match split.as_slice() {
        [one] => {
            let digits: String = one.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() != one.len() {
                return None;
            }
            match digits.len() {
                1 | 2 => (digits.parse::<u16>().ok()?, 0u16),
                3 => (digits[..1].parse().ok()?, digits[1..].parse().ok()?),
                4 => (digits[..2].parse().ok()?, digits[2..].parse().ok()?),
                _ => return None,
            }
        }
        [hh, mm] => (hh.parse().ok()?, mm.parse().ok()?),
        _ => return None,
    };
    (hh < 24 && mm < 60).then_some(hh * 60 + mm)
}

/// Render minutes since midnight as `HH:MM` for the settings fields.
pub fn fmt_hhmm(minute_of_day: u16) -> String {
    let m = minute_of_day % DAY_MINUTES;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Parse an operator-typed `AddToChart` bypass list such as `3, 5`.
///
/// Separator-agnostic (comma, space, semicolon), deduplicated and sorted so the stored list has
/// one canonical form. `0` is dropped: it is Moonbot's "no chart" value and would match every
/// ordinary detect.
pub fn parse_chart_list(text: &str) -> Vec<u32> {
    let mut out: Vec<u32> = text
        .split([',', ';', ' ', '\t'])
        .filter_map(|part| part.trim().parse::<u32>().ok())
        .filter(|n| *n != 0)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Render a bypass list back into the field text.
pub fn fmt_chart_list(list: &[u32]) -> String {
    list.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;
