//! Bounded UI-thread timing capture with off-thread log formatting and writes.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::time::{Duration, Instant, SystemTime};

const RECORD_CAPACITY: usize = 512;
const RECENT_CAPACITY: usize = 1024;
const MIN_CONTRIBUTOR: Duration = Duration::from_millis(1);

/// What the UI thread was doing during a timing span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallKind {
    Open,
    Close,
    Activate,
    Observe,
    Render,
    Input,
    Resize,
    Frame,
    Tick,
    ChartFrame,
}

impl StallKind {
    /// Return the ASCII token written to the stall log.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Close => "close",
            Self::Activate => "activate",
            Self::Observe => "observe",
            Self::Render => "render",
            Self::Input => "input",
            Self::Resize => "resize",
            Self::Frame => "frame",
            Self::Tick => "tick",
            Self::ChartFrame => "chartframe",
        }
    }

    /// Identify work that can explain a slow draw even below the stall threshold.
    fn contributes_to_frame(self) -> bool {
        matches!(
            self,
            Self::Render | Self::ChartFrame | Self::Observe | Self::Input
        )
    }
}

/// The timed part of a completed UI operation.
#[derive(Clone, Copy)]
struct TimedSpan {
    start: Instant,
    end: Instant,
    kind: StallKind,
    name: &'static str,
    window_id: Option<u64>,
}

/// A frame contributor and its elapsed time inside that draw.
#[derive(Clone, Copy)]
struct Contributor {
    kind: StallKind,
    name: &'static str,
    duration: Duration,
}

/// One captured operation or one slow draw with its longest overlapping operations.
enum Record {
    Span {
        timing: TimedSpan,
        depth: usize,
    },
    Frame {
        sample: FrameSample,
        contributors: [Option<Contributor>; 4],
    },
}

/// The UI-thread queues for captured records and short-lived frame attribution.
#[derive(Default)]
struct Capture {
    output: Drained,
    recent: VecDeque<TimedSpan>,
}

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static CAPTURE: RefCell<Capture> = const { RefCell::new(Capture {
        output: Drained { records: VecDeque::new(), dropped: 0 },
        recent: VecDeque::new(),
    }) };
}

/// Add an entry without letting the ring grow beyond its limit.
fn push_bounded<T>(ring: &mut VecDeque<T>, capacity: usize, item: T) -> usize {
    let evicted = usize::from(ring.len() == capacity);
    if evicted != 0 {
        ring.pop_front();
    }
    ring.push_back(item);
    evicted
}

/// Whether the stall channel is currently active.
#[inline]
pub fn enabled() -> bool {
    moon_core::diagnostics::ui_stall()
}

/// A timed UI-thread scope. Nesting belongs to the creating thread.
pub struct StallSpan {
    active: Option<ActiveSpan>,
    thread_bound: PhantomData<Rc<()>>,
}

/// Start time and nesting position of a scope captured while the channel is active.
struct ActiveSpan {
    kind: StallKind,
    name: &'static str,
    start: Instant,
    depth: usize,
    window_id: Option<u64>,
}

/// Start timing a UI-thread operation when the channel is active.
#[inline]
pub fn span(kind: StallKind, name: &'static str) -> StallSpan {
    start_span(kind, name, enabled(), Instant::now)
}

impl StallSpan {
    /// Bind a window-scoped operation to its draw even if another draw's interval overlaps.
    pub fn with_window(mut self, id: impl FnOnce() -> u64) -> Self {
        if let Some(active) = &mut self.active {
            active.window_id = Some(id());
        }
        self
    }
}

/// Keep the clock and thread locals entirely behind the channel gate.
#[inline]
fn start_span(
    kind: StallKind,
    name: &'static str,
    enabled: bool,
    now: impl FnOnce() -> Instant,
) -> StallSpan {
    let active = enabled.then(|| ActiveSpan {
        kind,
        name,
        start: now(),
        depth: DEPTH.with(|depth| depth.replace(depth.get() + 1)),
        window_id: None,
    });
    StallSpan {
        active,
        thread_bound: PhantomData,
    }
}

impl ActiveSpan {
    /// Unwind nesting and keep the completed timing only where it is needed.
    fn finish(self, end: Instant, threshold: Duration) {
        DEPTH.with(|depth| depth.set(depth.get() - 1));
        let elapsed = end.duration_since(self.start);
        let contributes = self.kind.contributes_to_frame() && elapsed >= MIN_CONTRIBUTOR;
        let slow = elapsed >= threshold;
        if !slow && !contributes {
            return;
        }
        let timing = TimedSpan {
            start: self.start,
            end,
            kind: self.kind,
            name: self.name,
            window_id: self.window_id,
        };
        CAPTURE.with_borrow_mut(|capture| {
            if contributes {
                push_bounded(&mut capture.recent, RECENT_CAPACITY, timing);
            }
            if slow {
                capture.output.push(Record::Span {
                    timing,
                    depth: self.depth,
                });
            }
        });
    }
}

impl Drop for StallSpan {
    /// Close the active scope without touching the clock when capture was disabled.
    fn drop(&mut self) {
        if let Some(active) = self.active.take() {
            active.finish(Instant::now(), moon_core::diagnostics::ui_stall_threshold());
        }
    }
}

/// GPUI's drawing interval and window identity, excluding native presentation.
#[derive(Clone, Copy)]
pub struct FrameSample {
    pub window_id: u64,
    pub draw_start: Instant,
    pub draw_end: Instant,
}

/// Report slow draws using the labels captured on this foreground thread.
pub fn note_frames(frames: impl IntoIterator<Item = FrameSample>) {
    record_frames(frames, moon_core::diagnostics::ui_stall_threshold());
}

/// Fold frame samples with an explicit threshold so interval edges are testable.
fn record_frames(frames: impl IntoIterator<Item = FrameSample>, threshold: Duration) {
    CAPTURE.with_borrow_mut(|capture| {
        let mut oldest_draw_start: Option<Instant> = None;
        for sample in frames {
            oldest_draw_start = Some(
                oldest_draw_start.map_or(sample.draw_start, |oldest| oldest.min(sample.draw_start)),
            );
            if sample.draw_end.duration_since(sample.draw_start) >= threshold {
                let contributors = frame_contributors(sample, &capture.recent);
                capture.output.push(Record::Frame {
                    sample,
                    contributors,
                });
            }
        }
        if let Some(oldest) = oldest_draw_start {
            capture.recent.retain(|span| span.end >= oldest);
        }
    });
}

/// Rank the four longest overlaps with this window's draw on the foreground thread.
fn frame_contributors(
    sample: FrameSample,
    recent: &VecDeque<TimedSpan>,
) -> [Option<Contributor>; 4] {
    let mut top: [Option<Contributor>; 4] = [None; 4];
    for span in recent {
        if span.window_id.is_some_and(|id| id != sample.window_id) {
            continue;
        }
        let start = span.start.max(sample.draw_start);
        let end = span.end.min(sample.draw_end);
        if end <= start {
            continue;
        }
        let duration = end.duration_since(start);
        if let Some(i) = top
            .iter()
            .position(|slot| slot.is_none_or(|c| duration > c.duration))
        {
            top[i..].rotate_right(1);
            top[i] = Some(Contributor {
                kind: span.kind,
                name: span.name,
                duration,
            });
        }
    }
    top
}

/// Raw records handed from the foreground capture loop to the log writer.
#[derive(Default)]
pub struct Drained {
    records: VecDeque<Record>,
    dropped: usize,
}

impl Drained {
    /// Append a record and account for any record evicted by the cap.
    fn push(&mut self, record: Record) {
        self.dropped += push_bounded(&mut self.records, RECORD_CAPACITY, record);
    }

    /// Fold a later drain into the pending batch without growing behind slow storage.
    fn merge(&mut self, later: Self) {
        if self.records.is_empty() {
            self.records = later.records;
        } else {
            for record in later.records {
                self.push(record);
            }
        }
        self.dropped += later.dropped;
    }

    /// Whether there is any record or loss to write.
    fn is_empty(&self) -> bool {
        self.records.is_empty() && self.dropped == 0
    }
}

/// Take the raw output and clear the attribution ring after folding the current draw samples.
pub fn drain() -> Drained {
    CAPTURE.with_borrow_mut(|capture| {
        capture.recent.clear();
        std::mem::take(&mut capture.output)
    })
}

/// Hold one pending batch while a detached background task writes the previous one.
#[derive(Default)]
pub struct Writer {
    pending: Drained,
    done: Option<Receiver<usize>>,
}

impl Writer {
    /// Submit a bounded batch without waiting for the channel writer's filesystem lock.
    pub fn submit(&mut self, executor: &gpui::BackgroundExecutor, drained: Drained) {
        self.pending.merge(drained);
        if let Some(done) = &self.done {
            match done.try_recv() {
                Ok(dropped) => self.pending.dropped += dropped,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {}
            }
            self.done = None;
        }
        if self.pending.is_empty() {
            return;
        }

        let batch = std::mem::take(&mut self.pending);
        let (completed, done) = sync_channel(1);
        self.done = Some(done);
        executor
            .spawn(async move {
                let lines = format(&batch, Instant::now(), SystemTime::now());
                let dropped = if lines.is_empty()
                    || moon_core::diagnostics::channel_lines("ui_stall.log", &lines)
                {
                    0
                } else {
                    lost_on_failure(&batch, &lines)
                };
                let _ = completed.try_send(dropped);
            })
            .detach();
    }
}

/// Count lost records after a failed write without counting the loss summary as a record.
fn lost_on_failure(batch: &Drained, lines: &[String]) -> usize {
    batch.dropped + lines.len() - usize::from(batch.dropped != 0)
}

/// Convert raw timings into ASCII lines using one wall/monotonic clock pair.
pub fn format(drained: &Drained, now: Instant, wall_now: SystemTime) -> Vec<String> {
    let mut lines = Vec::with_capacity(drained.records.len() + usize::from(drained.dropped != 0));
    for record in &drained.records {
        let start = match record {
            Record::Span { timing, .. } => timing.start,
            Record::Frame { sample, .. } => sample.draw_start,
        };
        let wall_start = if start <= now {
            wall_now.checked_sub(now.duration_since(start))
        } else {
            wall_now.checked_add(start.duration_since(now))
        };
        let ms = wall_start
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |since_epoch| since_epoch.as_millis());
        let mut line = format!("ts={ms} ");
        match record {
            Record::Span { timing, depth } => {
                let _ = write!(line, "kind={} name=", timing.kind.as_str());
                push_label(&mut line, timing.name);
                let elapsed_ms = millis(timing.end.duration_since(timing.start));
                let _ = write!(line, " ms={elapsed_ms:.1} depth={depth}");
            }
            Record::Frame {
                sample,
                contributors,
            } => {
                let kind = StallKind::Frame.as_str();
                let elapsed_ms = millis(sample.draw_end.duration_since(sample.draw_start));
                let _ = write!(
                    line,
                    "kind={kind} name=window:{} ms={elapsed_ms:.1} in=[",
                    sample.window_id
                );
                if contributors[0].is_none() {
                    line.push_str("unattributed");
                }
                for (i, contributor) in contributors.iter().flatten().enumerate() {
                    if i > 0 {
                        line.push(',');
                    }
                    line.push_str(contributor.kind.as_str());
                    line.push(':');
                    push_label(&mut line, contributor.name);
                    let _ = write!(line, ":{:.1}", millis(contributor.duration));
                }
                line.push(']');
            }
        }
        lines.push(line);
    }
    if drained.dropped != 0 {
        lines.push(format!("dropped={}", drained.dropped));
    }
    lines
}

/// Escape characters that could break the ASCII line or its field delimiters.
fn push_label(line: &mut String, label: &str) {
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
            line.push(ch);
        } else {
            let _ = write!(line, "\\u{{{:x}}}", u32::from(ch));
        }
    }
}

/// Keep sub-millisecond precision until the log line rounds to one decimal place.
fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests;
