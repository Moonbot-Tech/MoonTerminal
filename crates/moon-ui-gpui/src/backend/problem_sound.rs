//! Moonbot's "sound on network problems" switch, played here.
//!
//! The setting is the CORE's own — `signals.play_signal_sound` and the sound number beside it,
//! `signals.signal_sound`, both on the expert window's Interface page — but the core is a headless
//! Moonbot on another machine, so a switch left to it alone would beep where nobody is sitting.
//! The same reasoning put the price-approach alerts in [`super::alert_sound`], and this pass is
//! built on the same frame: a per-core revision gate so the feed drain does the work only when a
//! list actually arrived, a silent seed on a core's first visit, and at most one alarm per drain.
//! That last rule no longer protects the player — the scheduler never cuts a clip off — but a
//! network outage confirmed on many cores in one drain is one piece of news, not one alarm per
//! core. It bounds that one drain only; across drains the core's own five-second throttle and the
//! notice lane's fold of a repeated sound apply. A detect or an approach beep in the same drain
//! does not hold the alarm back (#1447).
//!
//! What counts as a "network problem" is the core's own word for it: a confirmed finding in the
//! `Network` category of its diagnostics feed. Moonbot's switch reacts to its exchange
//! connection dropping or lagging; those are exactly the findings that feed confirms and sends,
//! and it is the only wire the terminal has for them. The core throttles its own beep to once
//! every five seconds, and the same interval is kept here so a burst of confirmations reads as
//! one alarm rather than a stutter.

use std::collections::{HashMap, HashSet};

use moon_core::feed::CoreProblemCategory;
use moon_core::session::CoreId;

use crate::Backend;

/// Moonbot's own throttle on this alarm: one beep per core per five seconds.
const THROTTLE_MS: i64 = 5_000;

/// Per-core memory of which network findings have already sounded, and when the last one did.
#[derive(Default)]
pub(crate) struct ProblemSoundState {
    /// Kinds of the `Network` findings each core currently lists. A kind still in the set has
    /// announced itself; one that leaves and returns is a new finding and sounds again.
    seen: HashMap<CoreId, HashSet<u8>>,
    /// Last observed `problems_rev` per core: the pass costs nothing between list changes.
    last_rev: HashMap<CoreId, u64>,
    /// When each core last sounded, unix ms, for the throttle.
    last_sound_ms: HashMap<CoreId, i64>,
}

/// Why a fresh network finding was not handed to the scheduler.
///
/// Named so the detect log can say which one it was: "it did not beep" has each of these causes,
/// and they are indistinguishable from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Silenced {
    /// Quiet mode is sleeping; the finding is still marked seen.
    Quiet,
    /// Another core's alarm already went to the scheduler in this drain.
    AlarmThisDrain,
    /// This core alarmed less than five seconds ago.
    Throttled,
}

impl Silenced {
    /// The suffix the `[problem-sound]` diagnostic line carries for this reason.
    fn diag(self) -> &'static str {
        match self {
            Self::Quiet => ", silent: quiet mode",
            Self::AlarmThisDrain => ", silent: another core's alarm sounded in this drain",
            Self::Throttled => ", silent: within the 5 s throttle",
        }
    }
}

/// What one core's list meant on this pass.
#[derive(Debug, PartialEq, Eq)]
enum Observation {
    /// The list revision has not moved; nothing was read.
    Unchanged,
    /// First list from this core: remembered, never announced.
    Seeded,
    /// The list changed but every network finding in it had already been seen.
    NothingNew,
    /// A network finding this core did not list before.
    Fresh {
        /// Within five seconds of this core's last alarm.
        throttled: bool,
    },
}

impl ProblemSoundState {
    /// Collects a changed list only, reporting whether it holds a new network finding.
    fn observe(
        &mut self,
        core: CoreId,
        rev: u64,
        network_kinds: impl FnOnce() -> HashSet<u8>,
        now_ms: i64,
    ) -> Observation {
        if self.last_rev.get(&core) == Some(&rev) {
            return Observation::Unchanged;
        }
        let network_kinds = network_kinds();
        self.last_rev.insert(core, rev);
        // A first list seeds silently: findings that were already there when the terminal
        // connected are old news, not an alarm. The whole set is stored either way, so a finding
        // that clears and comes back is announced as the new event it is.
        let Some(was) = self.seen.insert(core, network_kinds.clone()) else {
            return Observation::Seeded;
        };
        if !network_kinds.iter().any(|kind| !was.contains(kind)) {
            return Observation::NothingNew;
        }
        let throttled = self
            .last_sound_ms
            .get(&core)
            .is_some_and(|last| now_ms - last < THROTTLE_MS);
        Observation::Fresh { throttled }
    }

    /// Records that this core's alarm just played, for the throttle.
    fn sounded(&mut self, core: CoreId, now_ms: i64) {
        self.last_sound_ms.insert(core, now_ms);
    }

    /// Decides whether a fresh finding is handed to the scheduler, and spends the core's throttle
    /// only when it is.
    ///
    /// The throttle lives here, beside the decision, so that no silenced branch can start the five
    /// seconds: an alarm that quiet mode or another core's alarm kept silent must not also make
    /// the core's next confirmation within the window silent, for a sound nobody heard.
    ///
    /// Args:
    ///     core: The core whose finding is fresh.
    ///     now_ms: The current unix time, for the throttle.
    ///     throttled: What [`Self::observe`] reported for this finding.
    ///     quiet: Whether quiet mode is sleeping.
    ///     alarm_this_drain: Whether another core's alarm was already handed over in this drain.
    ///
    /// Returns:
    ///     `Ok` when the caller must hand the sound to the scheduler, or why it stays silent.
    fn admit(
        &mut self,
        core: CoreId,
        now_ms: i64,
        throttled: bool,
        quiet: bool,
        alarm_this_drain: bool,
    ) -> Result<(), Silenced> {
        if quiet {
            return Err(Silenced::Quiet);
        }
        if alarm_this_drain {
            return Err(Silenced::AlarmThisDrain);
        }
        if throttled {
            return Err(Silenced::Throttled);
        }
        self.sounded(core, now_ms);
        Ok(())
    }

    /// Forgets a core whose alarm is not armed — no config yet, or the switch off — so that arming
    /// it later seeds silently from the list current THEN, instead of comparing against a set that
    /// stopped being maintained while the switch was off. The price alerts do the same.
    fn disarm(&mut self, core: CoreId) {
        self.seen.remove(&core);
        self.last_rev.remove(&core);
    }
}

impl Backend {
    /// Sounds a newly confirmed network problem on any core whose own switch asks for it, at most
    /// one alarm per drain.
    ///
    /// Takes no word from the detect scan or the price-approach pass of the same drain: the alarm
    /// goes to the notice lane, which plays ahead of a waiting detect and never interrupts the
    /// clip on the device, so an alarm held back for them was simply lost — its finding marked
    /// seen, never heard (#1447).
    pub(crate) fn play_problem_sounds(&mut self) {
        let mut played = false;
        let now_ms = moon_core::util::now_unix_ms_i64();
        for (core, data) in self.session.store().cores() {
            // The arming check comes BEFORE the list is folded in. No config yet means the core
            // has not sent its snapshot, and inventing a default would beep on a setting the user
            // never chose; the switch off means the same. Either way the core's memory is dropped,
            // so nothing is folded into `seen` while unarmed. Arming later then seeds silently from
            // the list current at that moment — findings already listed are old news, exactly as on
            // a core's first visit — and only what is confirmed AFTER arming sounds. Folding lists
            // in while unarmed would instead have marked a finding confirmed one second before the
            // switch flipped as seen, and a finding still listed at arming time would be no
            // different; the price alerts draw the same line.
            let armed = data
                .core_config
                .as_ref()
                .filter(|c| c.interface.play_signal_sound)
                .map(|c| c.interface.signal_sound);
            let Some(signal_sound) = armed else {
                self.problem_sound.disarm(core);
                continue;
            };
            let network_kinds = || {
                data.problems
                    .items
                    .iter()
                    .filter(|p| p.category == CoreProblemCategory::Network)
                    .map(|p| p.kind)
                    .collect()
            };
            let throttled =
                match self
                    .problem_sound
                    .observe(core, data.problems_rev, network_kinds, now_ms)
                {
                    Observation::Fresh { throttled } => throttled,
                    _ => continue,
                };
            // Quiet mode silences the sound but NOT the bookkeeping in `observe`: the set is
            // updated either way, so a night of findings does not empty itself into the morning.
            let admitted =
                self.problem_sound
                    .admit(core, now_ms, throttled, self.quiet_sleeping, played);
            moon_core::detect_diag::line(&format!(
                "[problem-sound] core={} sound={} ({}){}",
                moon_core::feed::core_label(core),
                signal_sound,
                crate::media::sound::mb_sound_name(signal_sound)
                    .as_deref()
                    .unwrap_or("NO SUCH SOUND, default plays"),
                admitted.err().map_or("", Silenced::diag)
            ));
            if admitted.is_ok() {
                crate::media::sound::play_ordinal(signal_sound);
                played = true;
            }
        }
    }
}

#[cfg(test)]
mod tests;
