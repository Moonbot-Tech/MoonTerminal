//! Plays sounds for incoming core detects and alerts. An alert firing arrives as `Event::Detect`
//! with `DETECT_KIND_ALERT`, so it obtains the source strategy sound through
//! `DetectRow.sound_name`, just like an ordinary detect. This is a `Backend` method because it needs
//! the core store. Feed draining invokes the scan; revision and sequence cursors avoid redundant
//! work and startup backlog. Each pass selects at most one eligible new detect per core, without an
//! audio queue. `sound::play` is asynchronous on Windows, where a later sound interrupts the one
//! already playing.

use crate::Backend;

impl Backend {
    /// Plays the most recent eligible new detect sound for each core. The `last_detect_seq` cursor
    /// prevents duplicates and startup bursts: the first visit seeds the cursor without playback.
    pub(crate) fn play_detect_sounds(&mut self) {
        for (core, data) in self.session.store().cores() {
            // Gate on revision because feed draining wakes this path hundreds of times per second
            // while detects change infrequently. Leave the list untouched when nothing changed.
            if self.last_detect_rev.get(&core) == Some(&data.detects_rev) {
                continue;
            }
            self.last_detect_rev.insert(core, data.detects_rev);
            crate::diag::bump(&crate::diag::DETECT_SCAN);
            // `seq` is a client-side monotonic counter and detects are appended in ascending order,
            // so the maximum is always at the back and requires no full traversal.
            let cur_max = data.detects.back().map(|d| d.seq).unwrap_or(0);
            let last = match self.last_detect_seq.get(&core) {
                Some(v) => *v,
                None => {
                    // Seed the cursor silently on the first visit instead of playing the backlog.
                    self.last_detect_seq.insert(core, cur_max);
                    continue;
                }
            };
            if cur_max <= last {
                continue;
            }
            // Find the newest eligible detect: use its strategy sound, or the default for an alert
            // firing without a strategy. Detects are ordered oldest to newest.
            let default_sound = &self.default_alert_sound;
            // Eligible events are detects with Moonbot's `SoundAlert=Yes` play-sound flag and all
            // alert firings. Play exactly the strategy-provided `sound_name`. For an ordinary detect,
            // None means SoundKind=NONE and remains silent, so find_map continues to the next event.
            // Only an alert firing without a strategy sound receives the default. Traverse newest to
            // oldest and select one sound per core per pass to limit bursts.
            let sound = data
                .detects
                .iter()
                .rev()
                .take_while(|d| d.seq > last)
                .find_map(|d| {
                    // Accept only alert firings or ordinary detects with SoundAlert enabled.
                    if !d.is_alert && !d.sound_alert {
                        return None;
                    }
                    match &d.sound_name {
                        Some(name) => Some(name.clone()),
                        // Use the default for an alert firing without a strategy snapshot. An
                        // ordinary detect with SoundKind=NONE stays silent and gets no default.
                        None if d.is_alert => Some(default_sound.clone()),
                        None => None,
                    }
                });
            self.last_detect_seq.insert(core, cur_max);
            if let Some(name) = sound {
                moon_core::detect_diag::line(&format!("[sound] core={core} play={name}"));
                crate::sound::play(&name);
            } else {
                moon_core::detect_diag::line(&format!(
                    "[sound] core={core} silent: {} new detects, среди них нет is_alert/SoundAlert",
                    cur_max.saturating_sub(last)
                ));
            }
        }
    }
}
