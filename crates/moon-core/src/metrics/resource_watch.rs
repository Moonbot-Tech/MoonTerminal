//! Reports growth of THIS process's resource counts into the ordinary application log — memory,
//! kernel handles and, on Windows, USER and GDI objects.
//!
//! Written beside [`super::cpu_watch`] for the same reason it was: a crash "after one to three
//! hours" leaves the crash handler's line if the handler ran, and nothing at all if it did not.
//! A leak is the one cause of such a crash that announces itself in advance — a count climbs for
//! an hour before the call that finally fails — and the only record of that climb is whatever was
//! written while the process was still healthy. USER and GDI objects have a hard ceiling of ten
//! thousand per process; past it `CreateWindow` and `LoadCursor` return null, and an application
//! that never expected that reads through the null a few frames later.
//!
//! It is not a heartbeat. A line is written when a count has MOVED by a step since that count
//! was last reported (either way — a release after a rise says "not a leak"), plus one baseline
//! a minute after start and a warning when USER or GDI objects approach their ceiling. Each
//! count keeps its own reference: memory churning through its step every few minutes does not
//! reset the measure of a handle count creeping up beside it. A healthy process writes
//! the baseline and nothing more for the rest of the session; a leaking one writes a line per step,
//! and the slope between those lines is the diagnosis. Every line carries every count, so any one
//! of them is a complete snapshot — the last line before a silent exit is what the report reads.
//!
//! Like `cpu_watch`, it rides the sampler that already polls the process once a second and adds
//! no polling of its own; the two Windows calls it needs are microseconds. The detector returns
//! events instead of logging so the whole decision is testable without a clock or a machine.

#[cfg(test)]
mod tests;

/// Milliseconds after the first sample before the baseline line is written.
///
/// Startup allocates and opens a great deal — windows, fonts, the databases, the feed — and a
/// baseline taken inside that would make the settled process look like a leak. A minute is past
/// all of it on every machine this has been measured on.
const BASELINE_AFTER_MS: i64 = 60_000;

/// Resident memory movement that earns a line, in MiB.
///
/// A chart's caches and a report query move tens of megabytes and settle; a leak worth a line
/// keeps going. At 128 a session that never leaks stays silent, and a leak of a megabyte a minute
/// — the kind that ends a session in hours — still writes its first line within the first two.
const MEM_STEP_MB: f32 = 128.0;

/// Kernel-handle movement that earns a line.
///
/// The terminal holds several hundred at rest — files, sockets, events, the GPU. Opening a window
/// or a report adds a handful; only a loop that never closes something adds hundreds.
const HANDLE_STEP: u32 = 250;

/// USER- or GDI-object movement that earns a line.
///
/// Both counts sit in the tens or low hundreds for this process. A step of the same size as the
/// handle step is a leak of a cursor, a brush or a window per frame for a few seconds — small
/// enough to be caught long before the ceiling, large enough that a dialog opening is not a line.
const GUI_STEP: u32 = 250;

/// The per-process ceiling on USER and GDI objects that Windows enforces.
const GUI_LIMIT: u32 = 10_000;

/// USER or GDI objects from which the count is reported as near its ceiling.
///
/// Two thousand under the limit: at a leak fast enough to matter that is minutes of warning, and
/// a healthy process is an order of magnitude below it.
const GUI_NEAR_LIMIT: u32 = 8_000;

/// How far below [`GUI_NEAR_LIMIT`] the count must fall before the warning may repeat.
const GUI_NEAR_LIMIT_RELEASE: u32 = 7_500;

/// Shortest gap between two movement lines, in milliseconds.
///
/// A leak fast enough to cross a step every second would otherwise write a line every second;
/// one a minute is enough to see the slope, and the ceiling warning is exempt.
const MIN_GAP_MS: i64 = 60_000;

/// One reading of the counts. `None` where the platform offers no number or the sampler missed
/// this tick, never zero: a zero would read as a release of everything and reset the reference.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ResourceSample {
    /// Resident memory in MiB; `None` when sysinfo could not see the process this tick.
    pub mem_mb: Option<f32>,
    /// Kernel handles held by the process.
    pub handles: Option<u32>,
    /// USER objects (windows, menus, cursors, hooks) — Windows only.
    pub user_objects: Option<u32>,
    /// GDI objects (bitmaps, brushes, fonts, device contexts) — Windows only.
    pub gdi_objects: Option<u32>,
}

/// One of the counts, in the order a line prints them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Count {
    Memory,
    Handles,
    UserObjects,
    GdiObjects,
}

impl Count {
    pub(super) const ALL: [Count; 4] = [
        Count::Memory,
        Count::Handles,
        Count::UserObjects,
        Count::GdiObjects,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Count::Memory => "RSS",
            Count::Handles => "handles",
            Count::UserObjects => "USER objects",
            Count::GdiObjects => "GDI objects",
        }
    }

    /// Whether this count moved by its step between two samples. A count missing on either side
    /// has not moved: there is nothing to measure.
    fn stepped(self, from: &ResourceSample, to: &ResourceSample) -> bool {
        let ints = |a: Option<u32>, b: Option<u32>, step: u32| match (a, b) {
            (Some(a), Some(b)) => a.abs_diff(b) >= step,
            _ => false,
        };
        match self {
            Count::Memory => match (from.mem_mb, to.mem_mb) {
                (Some(a), Some(b)) => (a - b).abs() >= MEM_STEP_MB,
                _ => false,
            },
            Count::Handles => ints(from.handles, to.handles, HANDLE_STEP),
            Count::UserObjects => ints(from.user_objects, to.user_objects, GUI_STEP),
            Count::GdiObjects => ints(from.gdi_objects, to.gdi_objects, GUI_STEP),
        }
    }

    /// Whether the sample carries no number for this count.
    fn is_missing(self, s: &ResourceSample) -> bool {
        match self {
            Count::Memory => s.mem_mb.is_none(),
            Count::Handles => s.handles.is_none(),
            Count::UserObjects => s.user_objects.is_none(),
            Count::GdiObjects => s.gdi_objects.is_none(),
        }
    }

    /// Copies this count's value from `from` into `to`.
    fn take(self, from: &ResourceSample, to: &mut ResourceSample) {
        match self {
            Count::Memory => to.mem_mb = from.mem_mb,
            Count::Handles => to.handles = from.handles,
            Count::UserObjects => to.user_objects = from.user_objects,
            Count::GdiObjects => to.gdi_objects = from.gdi_objects,
        }
    }
}

/// The counts that crossed their step in one line, in print order; `Copy` so the event stays a
/// plain value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Crossed([bool; 4]);

impl Crossed {
    fn any(self) -> bool {
        self.0.iter().any(|c| *c)
    }

    pub(super) fn contains(self, what: Count) -> bool {
        self.0[what as usize]
    }

    pub(super) fn iter(self) -> impl Iterator<Item = Count> {
        Count::ALL.into_iter().filter(move |c| self.contains(*c))
    }
}

/// One thing worth saying about this process's resources.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ResourceEvent {
    /// The settled process, a minute in: the reference every later line is read against.
    Baseline {
        after_secs: i64,
        sample: ResourceSample,
    },
    /// One or more counts moved by their step since each was last reported.
    Moved {
        crossed: Crossed,
        /// Per count (indexed like [`Crossed`]): seconds since THAT count was last reported —
        /// the baseline, or the movement line that named it. A ceiling warning in between is not
        /// a reference point: the slope is read between the two values on the line.
        since_secs: [i64; 4],
        /// Each count as it was when it was last reported — the reference its movement is
        /// measured from. Counts that did not cross keep their older reference.
        prev: ResourceSample,
        cur: ResourceSample,
    },
    /// USER or GDI objects reached [`GUI_NEAR_LIMIT`]. Carries the whole sample: this is the line
    /// most likely to be the last one before a silent exit.
    NearLimit {
        what: Count,
        value: u32,
        sample: ResourceSample,
    },
}

/// The detector: remembers what each count was when it was last reported and decides whether
/// this sample earns another line.
#[derive(Debug, Default)]
pub(super) struct ResourceWatch {
    /// Time of the first sample; the baseline waits [`BASELINE_AFTER_MS`] from it.
    first_ms: Option<i64>,
    /// Per-count reference: the value each count had when it was last reported, and the time of
    /// the last movement line (what the minute gap is measured from). `None` until the baseline.
    reference: Option<(i64, ResourceSample)>,
    /// When each count was last reported, indexed like [`Crossed`]; what its slope is read over.
    reported_ms: [i64; 4],
    /// Whether the near-limit warning is currently raised for USER / GDI objects.
    user_warned: bool,
    gdi_warned: bool,
}

impl ResourceWatch {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Judges one sample. At most one event per call; the ceiling check comes first because it is
    /// the one that predicts a crash, and a movement line the same second would only repeat its
    /// numbers.
    pub(super) fn observe(&mut self, now_ms: i64, sample: ResourceSample) -> Option<ResourceEvent> {
        let first = *self.first_ms.get_or_insert(now_ms);

        if let Some(event) = self.near_limit(sample) {
            return Some(event);
        }

        let Some((last_ms, reference)) = self.reference else {
            if now_ms.saturating_sub(first) < BASELINE_AFTER_MS {
                return None;
            }
            self.reference = Some((now_ms, sample));
            self.reported_ms = [now_ms; 4];
            return Some(ResourceEvent::Baseline {
                after_secs: now_ms.saturating_sub(first) / 1000,
                sample,
            });
        };

        // A count the baseline (or a later reference) has no number for takes the first number
        // that arrives, silently: a sampler miss on that one tick must not leave the count
        // unwatched for the rest of the session. Its slope is read from this moment.
        let mut reference = reference;
        for what in Count::ALL {
            if what.is_missing(&reference) && !what.is_missing(&sample) {
                what.take(&sample, &mut reference);
                self.reported_ms[what as usize] = now_ms;
                self.reference = Some((last_ms, reference));
            }
        }

        if now_ms.saturating_sub(last_ms) < MIN_GAP_MS {
            return None;
        }
        let mut crossed = Crossed::default();
        for what in Count::ALL {
            crossed.0[what as usize] = what.stepped(&reference, &sample);
        }
        if !crossed.any() {
            return None;
        }
        // Only the counts named on the line move their reference: a slow leak in another count
        // keeps accumulating against the value it was last reported at.
        let mut next = reference;
        let mut since_secs = [0; 4];
        for what in crossed.iter() {
            what.take(&sample, &mut next);
            since_secs[what as usize] =
                now_ms.saturating_sub(self.reported_ms[what as usize]) / 1000;
            self.reported_ms[what as usize] = now_ms;
        }
        self.reference = Some((now_ms, next));
        Some(ResourceEvent::Moved {
            crossed,
            since_secs,
            prev: reference,
            cur: sample,
        })
    }

    /// The ceiling warning, raised once per crossing and re-armed only after the count has fallen
    /// back below [`GUI_NEAR_LIMIT_RELEASE`], so a count hovering at the threshold does not write
    /// a line per second.
    fn near_limit(&mut self, sample: ResourceSample) -> Option<ResourceEvent> {
        let checks = [
            (
                Count::UserObjects,
                sample.user_objects,
                &mut self.user_warned,
            ),
            (Count::GdiObjects, sample.gdi_objects, &mut self.gdi_warned),
        ];
        for (what, value, warned) in checks {
            let Some(value) = value else { continue };
            if value >= GUI_NEAR_LIMIT {
                if !*warned {
                    *warned = true;
                    return Some(ResourceEvent::NearLimit {
                        what,
                        value,
                        sample,
                    });
                }
            } else if value < GUI_NEAR_LIMIT_RELEASE {
                *warned = false;
            }
        }
        None
    }
}

/// Every count of a sample on one line; `-` where there is no number.
fn counts(s: &ResourceSample) -> String {
    Count::ALL
        .into_iter()
        .map(|c| format!("{} {}", c.name(), value_of(s, c)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The value of one count as text, for the `prev→cur` part of a movement line.
fn value_of(s: &ResourceSample, what: Count) -> String {
    let int = |v: Option<u32>| v.map_or_else(|| "-".to_owned(), |v| v.to_string());
    match what {
        Count::Memory => s
            .mem_mb
            .map_or_else(|| "- MiB".to_owned(), |v| format!("{v:.0} MiB")),
        Count::Handles => int(s.handles),
        Count::UserObjects => int(s.user_objects),
        Count::GdiObjects => int(s.gdi_objects),
    }
}

/// Writes one event to the application log.
///
/// Movement is `info`: it is a fact about the session, not yet a fault. The ceiling warning is
/// `warn`: the process is minutes from a call that returns null.
pub(super) fn report(event: ResourceEvent) {
    match event {
        ResourceEvent::Baseline { after_secs, sample } => {
            log::info!(
                "resources: baseline {after_secs}s after start — {}",
                counts(&sample)
            );
        }
        ResourceEvent::Moved {
            crossed,
            since_secs,
            prev,
            cur,
        } => {
            let moves = crossed
                .iter()
                .map(|c| {
                    let secs = since_secs[c as usize];
                    format!(
                        "{} {}→{} over {}m{:02}s",
                        c.name(),
                        value_of(&prev, c),
                        value_of(&cur, c),
                        secs / 60,
                        secs % 60
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            log::info!("resources: {moves} — {}", counts(&cur));
        }
        ResourceEvent::NearLimit {
            what,
            value,
            sample,
        } => {
            log::warn!(
                "resources: {} at {value} of the {GUI_LIMIT} per-process limit — at the limit \
                 CreateWindow and LoadCursor return null — {}",
                what.name(),
                counts(&sample)
            );
        }
    }
}

/// Kernel handle and GUI-object counts of this process, or `None` off Windows.
#[cfg(windows)]
pub(super) fn platform_counts() -> (Option<u32>, Option<u32>, Option<u32>) {
    use windows_sys::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources, GetProcessHandleCount,
    };
    // A count of zero from `GetGuiResources` is its failure value as well as a real zero; this
    // process always has windows, so zero is read as "not available".
    let (handles, user, gdi) = unsafe {
        let process = GetCurrentProcess();
        let mut handles = 0u32;
        let handles_ok = GetProcessHandleCount(process, &mut handles) != 0;
        (
            handles_ok.then_some(handles),
            GetGuiResources(process, GR_USEROBJECTS),
            GetGuiResources(process, GR_GDIOBJECTS),
        )
    };
    (
        handles,
        (user > 0).then_some(user),
        (gdi > 0).then_some(gdi),
    )
}

#[cfg(not(windows))]
pub(super) fn platform_counts() -> (Option<u32>, Option<u32>, Option<u32>) {
    (None, None, None)
}
