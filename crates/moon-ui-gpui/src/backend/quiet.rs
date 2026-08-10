//! Quiet mode ("sleep") on the Backend: the persisted configuration, the cached asleep/awake
//! answer, and the once-per-tick schedule advance.
//!
//! The cached [`Backend::quiet_sleeping`] flag exists because the detect-sound path runs inside the
//! feed drain, which wakes hundreds of times per second, while answering "are we asleep" needs a
//! time-zone conversion. The conversion therefore happens once per coordination tick (10 Hz) and at
//! every user action; the hot path reads a bool.
//!
//! The rules themselves live in `moon_core::config::quiet` as pure functions over a minute-of-day,
//! so they are testable without a clock. This module only supplies "now" and publishes changes.

use moon_core::config::quiet::{self, QuietCfg};

use crate::Backend;

impl Backend {
    /// Current minute of day in the zone the header clock shows.
    ///
    /// The same zone as the visible clock on purpose: a schedule that fires at a different "23:00"
    /// than the one on screen is unexplainable to the operator.
    pub(crate) fn quiet_now_min(&self) -> u16 {
        let zone = crate::chrome::clock::resolved_header_clock_zone(self.header_clock_zone());
        let now = chrono::DateTime::from_timestamp_millis(moon_core::util::now_unix_ms_i64())
            .unwrap_or(chrono::DateTime::UNIX_EPOCH)
            .with_timezone(&zone);
        use chrono::Timelike as _;
        (now.hour() * 60 + now.minute()) as u16
    }

    /// The persisted quiet-mode configuration.
    pub(crate) fn quiet_cfg(&self) -> QuietCfg {
        self.layout.quiet.clone()
    }

    /// Whether sounds are currently silenced. Cached; see the module note.
    pub(crate) fn quiet_sleeping(&self) -> bool {
        self.quiet_sleeping
    }

    /// Replace the quiet-mode configuration from the settings popup.
    ///
    /// Args:
    ///     cfg: Edited configuration.
    ///     cx: Backend context used to publish the repaint.
    ///
    /// Returns:
    ///     Nothing; an unchanged configuration writes and notifies nothing.
    pub(crate) fn set_quiet_cfg(&mut self, cfg: QuietCfg, cx: &mut gpui::Context<Self>) {
        if self.layout.quiet == cfg {
            return;
        }
        self.layout.quiet = cfg;
        self.layout_dirty = true;
        self.refresh_quiet_state();
        self.mark_backend_dirty(cx);
    }

    /// Flip quiet mode from the header toggle.
    ///
    /// Args:
    ///     cx: Backend context used to publish the repaint.
    ///
    /// Returns:
    ///     Nothing; the new state is persisted with the layout.
    pub(crate) fn toggle_quiet(&mut self, cx: &mut gpui::Context<Self>) {
        let now_ms = moon_core::util::now_unix_ms_i64();
        let now_min = self.quiet_now_min();
        // The end of a hand-switched sleep, resolved HERE into an absolute instant: the rule is
        // "until the configured off-time", and only an instant survives the terminal being closed
        // across it. Derived from the local minute, so a DST shift inside the window moves the end
        // by that hour — an hour of sleep once every six months, against a schedule that would
        // otherwise be stuck for a whole day.
        // Floored to the current minute before adding the remaining ones, so the deadline lands ON
        // 07:00:00 rather than at 07:00:45. The tick that acts on it only runs when the minute
        // changes, so an unfloored deadline would sleep a full minute past the configured end.
        let until_ms = now_ms - now_ms.rem_euclid(60_000)
            + i64::from(quiet::minutes_until(
                now_min,
                self.layout.quiet.to_min_norm(),
            )) * 60_000;
        self.layout.quiet.toggle_at(now_min, Some(until_ms));
        self.layout_dirty = true;
        self.refresh_quiet_state();
        self.mark_backend_dirty(cx);
    }

    /// Advance the schedule one coordination tick.
    ///
    /// Runs at 10 Hz from the coordination loop and does nothing at all until the wall-clock minute
    /// changes — the transitions it owns (a manual sleep reaching its off-time, a spent wake
    /// override) happen at most twice a day, and repainting the header on a tick that changed
    /// nothing is exactly the notify-everything habit this codebase keeps paying for.
    ///
    /// Args:
    ///     cx: Backend context used to publish the repaint on an actual transition.
    ///
    /// Returns:
    ///     Nothing; state, persistence and observers are updated in place.
    pub(crate) fn tick_quiet(&mut self, cx: &mut gpui::Context<Self>) {
        let now_min = self.quiet_now_min();
        if now_min == self.quiet_last_min {
            return;
        }
        let was = self.quiet_sleeping;
        self.apply_quiet_at(now_min);
        // The window's own start and end move this flag without any persisted field changing, so
        // the repaint is gated on the FLAG, not on whether a field was written.
        if was != self.quiet_sleeping {
            self.mark_backend_dirty(cx);
        }
    }

    /// Recompute the cached asleep/awake answer from the configuration and the current minute.
    ///
    /// Also advances the expiries, which is what makes a manual sleep that ran out while the
    /// terminal was CLOSED end at startup instead of silencing the next working day.
    pub(crate) fn refresh_quiet_state(&mut self) {
        let now_min = self.quiet_now_min();
        self.apply_quiet_at(now_min);
    }

    /// Advance expiries and the cached flag to an already-resolved minute of day.
    ///
    /// One body for the tick and the seed so they cannot answer differently, and it takes the
    /// minute rather than reading the clock again — the zone conversion is the expensive part.
    fn apply_quiet_at(&mut self, now_min: u16) {
        self.quiet_last_min = now_min;
        if self
            .layout
            .quiet
            .tick(moon_core::util::now_unix_ms_i64(), now_min)
        {
            self.layout_dirty = true;
        }
        self.quiet_sleeping = self.layout.quiet.sleeping_at(now_min);
    }

    /// Whether one arriving detect may still play its sound.
    ///
    /// Args:
    ///     is_alert: Whether the event is a drawn-figure alert trigger.
    ///     add_to_chart: The detect's `AddToChart` tab number (`0` = none).
    ///
    /// Returns:
    ///     Whether the sound survives quiet mode; always true while awake.
    pub(crate) fn quiet_allows_detect(&self, is_alert: bool, add_to_chart: u32) -> bool {
        !self.quiet_sleeping
            || self
                .layout
                .quiet
                .allows_detect_sound(is_alert, add_to_chart)
    }

    /// Whether one core-warning axis may still play its alert sound.
    ///
    /// Args:
    ///     axis: Axis whose episode just opened.
    ///
    /// Returns:
    ///     Whether the sound survives quiet mode; always true while awake.
    pub(crate) fn quiet_allows_warn(&self, axis: crate::backend::core_warn::WarnAxis) -> bool {
        use crate::backend::core_warn::WarnAxis;
        if !self.quiet_sleeping {
            return true;
        }
        let b = &self.layout.quiet.warn_bypass;
        match axis {
            WarnAxis::SysCpu => b.cpu,
            WarnAxis::MemGrowth => b.mem,
            WarnAxis::Unreachable => b.conn,
            WarnAxis::Ping => b.ping,
            WarnAxis::ExchPing => b.exch,
            WarnAxis::ApiExpiry => b.api,
        }
    }
}
