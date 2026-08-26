//! Reports a CPU spike of THIS process into the ordinary application log.
//!
//! Every other diagnostic here is a channel someone turns on. This one cannot be: the failure it
//! exists for is rare, arrives unannounced, and a restart cures it — so by the time anyone thinks
//! to arm a switch the evidence is already gone. It was written after a report of "70% CPU for
//! about a minute after the PC woke from sleep, normal again after a restart", which left nothing
//! behind at all: the figure came from the status bar, and the status bar keeps no history. It is
//! therefore unconditional output, like the reconnect and report-throughput lines beside it, rather
//! than a switch in `cfg/diagnostics.toml` — there is no state to arm it from.
//!
//! Two decisions follow from that.
//!
//! - **It rides the sampler that already exists.** [`super::Metrics`] polls this process
//!   once a second for the status bar, off a background timer rather than off render, so it keeps
//!   sampling with every window minimised. This module is handed those samples and adds no polling
//!   of its own: a second `refresh_processes` would be a whole extra walk of the system process
//!   table per second, and the figures could then disagree with the ones on screen.
//! - **An episode, not a stream.** One line when the load rises and one when it falls. A rise that
//!   never falls back is not a spike but a new normal — the terminal is doing more than it was — so
//!   after five minutes it is adopted as the baseline and the episode is closed. Without that the
//!   file would carry a heartbeat forever AND the detector would be stuck inside one episode,
//!   unable to report the next real spike. Not a running CPU record — a note when it grew.
//!
//! Thresholds are relative to ONE LOGICAL CORE, not to a fixed percentage. Everything here is a
//! share of the whole machine (the status bar's convention), and in that unit one pegged thread is
//! 4% on a 24-thread box and 25% on a 4-thread one; a fixed floor would either drown a small
//! machine in lines or hide a spinning thread on a large one.
//!
//! What it cannot see, stated plainly: on a very large machine one busy thread approaches the idle
//! jitter of the whole process, so a single-thread spin can hide under the noise. Catching that
//! needs per-thread accounting, which is a different instrument. This one is aimed at the shape the
//! incident had — many threads at once.

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

use super::MetricsSnapshot;

/// Samples the rolling baseline is taken over, at one sample a second.
const WINDOW_SAMPLES: usize = 60;

/// Samples that must already be in the window before any judgement is made.
///
/// The baseline is read BEFORE the current sample joins the window, so judgement begins on the
/// `MIN_SAMPLES + 1`-th observation. A baseline over two or three samples is not a baseline; it is
/// whatever the process happened to be doing when the window opened.
const MIN_SAMPLES: usize = 8;

/// Share of one logical core that counts as a rise over the baseline.
///
/// Three fifths of a core: enough that ordinary jitter cannot reach it, little enough that one
/// thread going into a spin does.
const RISE_CORES: f32 = 0.6;

/// Share of the baseline that counts as a rise when the baseline is already large.
///
/// Read together with [`RISE_CORES`] through a `max`, and expressed as a GAIN over the baseline
/// rather than a multiple of it: at 1.0 the threshold is a DOUBLING of the baseline. A multiple
/// would put the threshold past 100% on any install whose baseline is a third of the machine, and
/// silence the detector exactly where the load is worst.
const RISE_GAIN: f32 = 1.0;

/// Floor under which nothing is reported however small the machine's cores are, in percent.
const FLOOR_MIN_PCT: f32 = 2.5;

/// Ceiling the proportional part of the rise threshold is clamped to, in percent of the machine.
///
/// Not the final word on the threshold — a baseline above it still gets its own band bolted on
/// afterwards — but the point past which "twice the baseline" stops being a reachable ask.
const CEILING_PCT: f32 = 95.0;

/// Where the calm threshold sits between the baseline and the rise threshold.
///
/// Derived from the rise threshold rather than stated on its own, which is what keeps the two from
/// crossing or from leaving a band that is too low to report and too high to close.
const CALM_FRACTION: f32 = 0.5;

/// Consecutive samples a rise must hold before it is reported.
///
/// Five seconds. The threshold sits near one core precisely so a spinning thread is caught, and at
/// that sensitivity a shorter hold would report ordinary bursts — a chart repaint, a report query.
const RISE_SAMPLES: u8 = 5;

/// Consecutive samples the calm must hold before the episode is closed.
const CALM_SAMPLES: u8 = 3;

/// Gap between two samples that means this process was not running in between.
///
/// At a one-second cadence anything this long is a suspended process — the machine slept — or a
/// wall clock that was stepped.
const GAP_MS: i64 = 20_000;

/// Backwards step tolerated without calling it a discontinuity, in milliseconds.
///
/// An ordinary time correction moves the clock by milliseconds, and every duration here is
/// saturating, so treating that as a suspend would report a wake that never happened.
const BACK_STEP_MS: i64 = 1_000;

/// How long a rise may hold before it is treated as the new normal, in milliseconds.
///
/// Long enough that the incident this module was written for — about a minute — ends as a spike,
/// short enough that a changed workload does not sit in an open episode for a session.
const HOLD_REPORT_MS: i64 = 5 * 60 * 1000;

/// An episode cut short by a suspend or a clock step, reported with the resume rather than lost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Interrupted {
    /// Highest process CPU seen before the discontinuity.
    pub peak: f32,
    /// Seconds from the episode opening to the last sample before the discontinuity.
    pub held_secs: u64,
}

/// One thing worth saying about this process's CPU.
///
/// The detector returns these instead of logging itself so the whole decision is testable without a
/// clock, a machine load, or a log sink.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CpuEvent {
    /// Sampling stopped and restarted: the process was suspended, or the clock was stepped.
    ///
    /// The baseline deliberately SURVIVES this. The quiet state of a process does not change
    /// because its machine slept, and the spike worth catching is the one that is already running
    /// when sampling resumes — rebuilding the baseline from those samples would make the spike its
    /// own reference and report nothing at all.
    Resumed {
        /// Seconds between the last sample and this one; negative when the clock was set back.
        delta_secs: i64,
        /// The FIRST sample after the gap, percent of the machine, or `None` when that sample
        /// was not a number.
        ///
        /// Deliberately not called an average across the gap, and deliberately not used to decide
        /// whether the process slept or its sampling tick was starved. That distinction needs the
        /// process's cumulative CPU time measured against the wall clock; what sysinfo reports on
        /// Windows divides by the `GetSystemTimes` delta, which does not advance while the machine
        /// is suspended — so this figure describes the moment sampling resumed, not the silence
        /// before it. Telling the two kinds of gap apart needs an instrument this module lacks.
        ///
        /// An `Option` rather than a substituted zero: every other path DISCARDS a non-finite
        /// sample, and printing a figure nobody measured is worse than printing none.
        cpu_after_gap: Option<f32>,
        /// The episode the discontinuity cut short, if one was open.
        interrupted: Option<Interrupted>,
    },
    /// Process CPU rose clear of its own baseline and held there.
    Rose {
        /// Current process CPU, percent of the whole machine.
        cur: f32,
        /// Baseline the rise is measured against, frozen for the episode.
        baseline: f32,
        /// Whole-machine CPU at the same moment, for "is it only us".
        system: f32,
        /// Process resident memory at the same moment, MiB.
        mem_mb: f32,
    },
    /// The rise held for [`HOLD_REPORT_MS`] without falling back, so it is adopted as the new
    /// normal and the episode is closed.
    ///
    /// A load that lasts this long is a changed workload, not a spike: charts opened, a core added.
    /// Reporting it forever would bury the file, and holding the episode open would blind the
    /// detector to every later rise, because a detector inside an episode only looks for its end.
    Settled {
        /// Highest process CPU seen this episode.
        peak: f32,
        /// Level adopted as the new baseline.
        cur: f32,
        /// Wall-clock seconds from the rise to here.
        held_secs: u64,
        /// Resident memory change over those seconds, MiB.
        mem_delta_mb: f32,
    },
    /// The episode is over.
    Fell {
        /// Highest process CPU seen this episode.
        peak: f32,
        /// Baseline the episode was measured against.
        baseline: f32,
        /// Wall-clock seconds the episode lasted.
        held_secs: u64,
        /// Resident memory change over the episode, MiB.
        mem_delta_mb: f32,
    },
}

/// The two thresholds of one baseline, always with the calm one strictly under the rise one.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Thresholds {
    /// CPU a sample must reach to count as a rise.
    rise: f32,
    /// CPU an episode must fall back under to be over.
    calm: f32,
}

/// Thresholds for one baseline on a machine whose logical core is `core_pct` of the whole.
///
/// Args:
///     baseline: Quiet-state process CPU, percent of the machine.
///     core_pct: One logical core as a percentage of the machine.
///
/// Above [`CEILING_PCT`] the pair still brackets the baseline, but the band narrows towards a single
/// point of headroom, so the detector turns twitchy rather than quiet. That is accepted, not
/// designed: a terminal whose NORMAL is 95% of the machine is past what this instrument is for, and
/// the figure in the status bar is already saying so.
///
/// Returns:
///     The rise and calm thresholds, with `baseline < calm < rise <= 100` guaranteed for any
///     baseline below [`CEILING_PCT`].
fn thresholds(baseline: f32, core_pct: f32) -> Thresholds {
    // Only the absolute half of the floor can bind: the sum below already carries a whole
    // `RISE_CORES * core_pct` for any baseline at or above zero.
    // `clamp` is safe here on both counts the lint warns about: the two bounds are finite literals
    // and `FLOOR_MIN_PCT < CEILING_PCT` by inspection.
    let rise = (baseline + (RISE_CORES * core_pct).max(baseline * RISE_GAIN))
        .clamp(FLOOR_MIN_PCT, CEILING_PCT)
        // A band to report into, so a capped threshold on a pathological baseline does not sit
        // under the very level it is measuring — and last the ceiling of what can ever be
        // MEASURED, because a threshold above 100% of the machine is one no sample can reach.
        .max(baseline + 1.0)
        .min(100.0);
    Thresholds {
        rise,
        calm: baseline + (rise - baseline) * CALM_FRACTION,
    }
}

/// State of an open episode.
#[derive(Clone, Copy, Debug)]
struct Episode {
    /// Baseline frozen when the episode opened.
    ///
    /// Frozen so the rise and the fall are judged against the SAME number. The window is also held
    /// still for the duration, and the two work together: a baseline recomputed from a window the
    /// spike had fed would climb to meet the load, and the episode could never be declared over.
    baseline: f32,
    /// Highest process CPU seen so far, including the samples that opened the episode.
    peak: f32,
    /// Wall-clock millisecond the episode opened at.
    started_ms: i64,
    /// Resident memory when the episode opened, MiB.
    started_mem_mb: f32,
    /// Level this episode must fall back under to be over.
    ///
    /// Computed once here rather than per sample: it derives from `baseline`, which is frozen for
    /// the episode, so recomputing it on every sample would re-derive a constant — and discard the
    /// rise half of the pair while doing so.
    calm: f32,
}

/// Turns a stream of samples into episodes.
#[derive(Debug)]
pub(super) struct SpikeDetector {
    /// One logical core as a percentage of the whole machine.
    core_pct: f32,
    /// Recent process-CPU samples, oldest first, at most [`WINDOW_SAMPLES`].
    window: VecDeque<f32>,
    /// Wall-clock millisecond of the previous sample, for discontinuity detection.
    last_ms: Option<i64>,
    /// Consecutive samples above the rise threshold.
    rising: u8,
    /// Highest sample of the current run of rising ones, so an episode opens with the peak it
    /// actually reached rather than with the sample that happened to confirm it.
    rise_peak: f32,
    /// Consecutive samples below the calm threshold.
    calming: u8,
    /// The open episode, if any.
    episode: Option<Episode>,
}

impl SpikeDetector {
    /// Creates a detector for a machine with `logical_cpus` logical processors.
    pub(super) fn new(logical_cpus: usize) -> Self {
        Self {
            core_pct: 100.0 / logical_cpus.max(1) as f32,
            window: VecDeque::with_capacity(WINDOW_SAMPLES),
            last_ms: None,
            rising: 0,
            rise_peak: 0.0,
            calming: 0,
            episode: None,
        }
    }

    /// Folds one sample in and returns the one thing worth saying about it, if any.
    ///
    /// Args:
    ///     now_ms: Wall-clock milliseconds since the epoch for this sample.
    ///     snap: The metrics sample the status bar was handed at the same moment.
    ///
    /// Returns:
    ///     The event to report, or `None` while nothing has changed.
    pub(super) fn observe(&mut self, now_ms: i64, snap: &MetricsSnapshot) -> Option<CpuEvent> {
        if let Some(delta_secs) = self.discontinuity(now_ms) {
            return Some(self.resume(now_ms, delta_secs, snap));
        }
        self.last_ms = Some(now_ms);
        // A sample that is not a number can neither open nor close an episode, and letting one into
        // the comparisons below would open an episode that no later sample can ever close.
        if !snap.cpu_process.is_finite() {
            return None;
        }
        match self.episode {
            Some(episode) => self.observe_open(now_ms, snap, episode),
            None => {
                // Read BEFORE this sample joins the window: a spike must not raise the bar it is
                // being measured against. Both are skipped while an episode is open — the window is
                // held still for its duration, and sorting it for a baseline nothing reads would be
                // work done precisely while the process is already under the load being diagnosed.
                let baseline = self.baseline();
                self.window_push(snap.cpu_process);
                self.observe_quiet(now_ms, snap, baseline)
            }
        }
    }

    /// Returns the signed seconds of a discontinuity before this sample, if there was one.
    fn discontinuity(&self, now_ms: i64) -> Option<i64> {
        let last = self.last_ms?;
        let delta = now_ms.saturating_sub(last);
        (delta >= GAP_MS || delta <= -BACK_STEP_MS).then_some(delta / 1000)
    }

    /// Closes out a discontinuity, keeping the baseline and surrendering any open episode.
    fn resume(&mut self, now_ms: i64, delta_secs: i64, snap: &MetricsSnapshot) -> CpuEvent {
        let last_ms = self.last_ms.unwrap_or(now_ms);
        let interrupted = self.episode.take().map(|episode| Interrupted {
            peak: episode.peak,
            held_secs: secs_between(episode.started_ms, last_ms),
        });
        self.reset_rise();
        self.calming = 0;
        self.last_ms = Some(now_ms);
        CpuEvent::Resumed {
            delta_secs,
            cpu_after_gap: snap.cpu_process.is_finite().then_some(snap.cpu_process),
            interrupted,
        }
    }

    /// Ends the current run of rising samples.
    ///
    /// One place, so a further fact about a run cannot be forgotten at one of the sites that end it.
    fn reset_rise(&mut self) {
        self.rising = 0;
        self.rise_peak = 0.0;
    }

    /// Appends one sample, evicting the oldest past [`WINDOW_SAMPLES`].
    fn window_push(&mut self, cpu: f32) {
        if self.window.len() == WINDOW_SAMPLES {
            self.window.pop_front();
        }
        self.window.push_back(cpu);
    }

    /// Median of the window, or `None` before [`MIN_SAMPLES`] have arrived.
    ///
    /// A median rather than a mean: one 60% sample in a window of 2% samples moves a mean enough to
    /// hide the next rise behind it, and the whole point of the baseline is that it describes the
    /// quiet state. The LOWER median on an even window, so the figure every threshold is built on
    /// is not biased upwards.
    fn baseline(&self) -> Option<f32> {
        (self.window.len() >= MIN_SAMPLES).then(|| {
            // On the stack and partially ordered: this runs every quiet second for the life of the
            // process, and one element of it is read. The window is bounded by `WINDOW_SAMPLES`, so
            // the array covers it and the length below bounds what is actually used.
            let mut scratch = [0.0_f32; WINDOW_SAMPLES];
            let len = self.window.len();
            for (slot, value) in scratch.iter_mut().zip(self.window.iter()) {
                *slot = *value;
            }
            let median = (len - 1) / 2;
            *scratch[..len]
                .select_nth_unstable_by(median, f32::total_cmp)
                .1
        })
    }

    /// Handles a sample while no episode is open.
    fn observe_quiet(
        &mut self,
        now_ms: i64,
        snap: &MetricsSnapshot,
        baseline: Option<f32>,
    ) -> Option<CpuEvent> {
        let baseline = baseline?;
        let thresholds = thresholds(baseline, self.core_pct);
        if snap.cpu_process < thresholds.rise {
            self.reset_rise();
            return None;
        }
        self.rising = self.rising.saturating_add(1);
        self.rise_peak = self.rise_peak.max(snap.cpu_process);
        if self.rising < RISE_SAMPLES {
            return None;
        }
        let peak = self.rise_peak;
        self.reset_rise();
        self.episode = Some(Episode {
            baseline,
            peak,
            started_ms: now_ms,
            started_mem_mb: snap.mem_mb,
            calm: thresholds.calm,
        });
        Some(CpuEvent::Rose {
            cur: snap.cpu_process,
            baseline,
            system: snap.cpu_system,
            mem_mb: snap.mem_mb,
        })
    }

    /// Handles a sample while an episode is open.
    ///
    /// The episode arrives by value and is written back rather than borrowed in place: closing it
    /// means clearing the very field a borrow would hold.
    fn observe_open(
        &mut self,
        now_ms: i64,
        snap: &MetricsSnapshot,
        mut episode: Episode,
    ) -> Option<CpuEvent> {
        episode.peak = episode.peak.max(snap.cpu_process);

        if snap.cpu_process <= episode.calm {
            self.calming = self.calming.saturating_add(1);
            if self.calming < CALM_SAMPLES {
                self.episode = Some(episode);
                return None;
            }
            self.calming = 0;
            self.episode = None;
            return Some(CpuEvent::Fell {
                peak: episode.peak,
                baseline: episode.baseline,
                held_secs: secs_between(episode.started_ms, now_ms),
                mem_delta_mb: snap.mem_mb - episode.started_mem_mb,
            });
        }

        self.calming = 0;
        if now_ms.saturating_sub(episode.started_ms) < HOLD_REPORT_MS {
            self.episode = Some(episode);
            return None;
        }
        // Held long enough to be a workload rather than a spike. Close the episode and re-seed the
        // window from this level: leaving it open would emit this line forever and, worse, keep the
        // detector inside an episode where it looks only for an end and can never report the next
        // rise.
        self.episode = None;
        self.window.clear();
        self.window_push(snap.cpu_process);
        Some(CpuEvent::Settled {
            peak: episode.peak,
            cur: snap.cpu_process,
            held_secs: secs_between(episode.started_ms, now_ms),
            mem_delta_mb: snap.mem_mb - episode.started_mem_mb,
        })
    }
}

/// Whole seconds between two wall-clock milliseconds, never negative.
fn secs_between(from_ms: i64, to_ms: i64) -> u64 {
    to_ms.saturating_sub(from_ms).max(0) as u64 / 1000
}

/// A memory delta rendered for the log: signed, whole MiB, and never a signed zero.
///
/// Through [`crate::util::fmt::signed_fixed`], which owns that rule — a delta of a few hundred
/// kilobytes rounds to `-0.0` and `format!` prints the minus, putting a minus sign on a line that
/// says nothing changed. A reader who cannot trust the sign cannot trust the figure.
fn mib_for_log(delta_mb: f32) -> String {
    crate::util::fmt::signed_fixed(delta_mb as f64, 0)
        .map_or_else(|| "?".to_owned(), |(text, _)| text)
}

/// Writes one event to the application log.
///
/// A rise is a warning; a return to normal is not, so `Fell` and `Resumed` are `info` — a recovered
/// process must not leave a warning standing in the Log panel. All four carry the same `cpu:` tag,
/// so one grep still collects a whole episode. Memory is MiB throughout, absolute on the way in and
/// as a change afterwards, so the lines of one episode can be read against each other.
pub(super) fn report(event: CpuEvent) {
    match event {
        CpuEvent::Resumed {
            delta_secs,
            cpu_after_gap,
            interrupted,
        } => {
            let what = if delta_secs < 0 {
                format!("clock stepped back {}s", -delta_secs)
            } else {
                format!("no samples for {delta_secs}s")
            };
            let ended = match interrupted {
                Some(Interrupted { peak, held_secs }) => {
                    format!("; the open episode ends here after {held_secs}s, peak {peak:.0}%")
                }
                None => String::new(),
            };
            // States the silence and the first figure after it, and stops there. Whether the
            // process slept or its sampling tick was starved is not a question this figure can
            // answer; the timestamps of the lines around this one can.
            let after = match cpu_after_gap {
                Some(cpu) => format!("first sample after it reads {cpu:.0}% of the machine"),
                None => "the first sample after it was not a number".to_owned(),
            };
            log::info!("cpu: {what} — {after}; the baseline is kept{ended}");
        }
        CpuEvent::Rose {
            cur,
            baseline,
            system,
            mem_mb,
        } => log::warn!(
            "cpu: process rose to {cur:.0}% of the machine (baseline {baseline:.0}%), \
             system {system:.0}%, RSS {mem_mb:.0} MiB"
        ),
        CpuEvent::Settled {
            peak,
            cur,
            held_secs,
            mem_delta_mb,
        } => log::warn!(
            "cpu: still at {cur:.0}% after {held_secs}s — adopting it as the new baseline, \
             peak was {peak:.0}%, RSS {} MiB over those seconds",
            mib_for_log(mem_delta_mb)
        ),
        CpuEvent::Fell {
            peak,
            baseline,
            held_secs,
            mem_delta_mb,
        } => log::info!(
            "cpu: back to baseline {baseline:.0}% after {held_secs}s — peak was {peak:.0}%, \
             RSS {} MiB over the episode",
            mib_for_log(mem_delta_mb)
        ),
    }
}
