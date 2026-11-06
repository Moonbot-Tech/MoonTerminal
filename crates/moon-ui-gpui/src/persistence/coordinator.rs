//! Serial background persistence for immutable snapshots of live workspace state.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use moon_core::config::{
    ConfigSnapshot, CoreUpdateHistory, DetectViewFile, TabBadgeSettings, WindowLayout,
};
use moon_core::figures::FigureSnapshot;
use moon_ui::DockTopologyByName;

use crate::persistence::chart_persist::ChartTabSpec;
use crate::persistence::dock_persist::DockMap;
use crate::window::detached::DetachedSpec;

#[cfg(test)]
mod tests;

pub(crate) mod firetest;

/// Immutable values written by one serial persistence request.
#[derive(Clone)]
pub(crate) struct PersistenceSnapshot {
    /// Complete `layout.toml` authority when layout state was dirty.
    layout: Option<WindowLayout>,
    /// Complete paired Classic dock and detached-window authority when either side was dirty.
    classic: Option<(DockMap, Vec<DetachedSpec>)>,
    /// Complete shared Auto topology when its persistence authority was dirty.
    auto: Option<DockTopologyByName>,
    /// Complete detection display authority when a popup control changed.
    detects: Option<DetectViewFile>,
    /// Chart specifications captured before any later tab edits.
    charts: Option<Vec<ChartTabSpec>>,
    /// Local drawings, including unknown types retained from newer builds.
    figures: Option<FigureSnapshot>,
    /// Tab display preferences and read watermarks.
    badges: Option<TabBadgeSettings>,
    /// A snapshot of the session-owned core update history.
    updates: Option<CoreUpdateHistory>,
    /// Validated config, normalized on the application thread before capture.
    config: Option<ConfigSnapshot>,
}

impl PersistenceSnapshot {
    /// Build an empty request that performs no file writes.
    ///
    /// Returns:
    ///     A snapshot with no persistence class selected.
    pub(crate) fn empty() -> Self {
        Self {
            layout: None,
            classic: None,
            auto: None,
            detects: None,
            charts: None,
            figures: None,
            badges: None,
            updates: None,
            config: None,
        }
    }

    /// Attach the latest complete layout authority.
    ///
    /// Args:
    ///     layout: Immutable layout value captured on the application thread.
    ///
    /// Returns:
    ///     This snapshot with layout persistence selected.
    pub(crate) fn with_layout(mut self, layout: WindowLayout) -> Self {
        self.layout = Some(layout);
        self
    }

    /// Attach the latest complete paired Classic authority.
    ///
    /// Args:
    ///     docks: Complete Classic dock topology and panel payload map.
    ///     detached: Complete Classic detached-panel ownership list.
    ///
    /// Returns:
    ///     This snapshot with joint Classic persistence selected.
    pub(crate) fn with_classic(mut self, docks: DockMap, detached: Vec<DetachedSpec>) -> Self {
        self.classic = Some((docks, detached));
        self
    }

    /// Attach the latest complete shared Auto dock topology.
    ///
    /// Args:
    ///     topology: Normalized topology-only authority accepted by Backend.
    ///
    /// Returns:
    ///     This snapshot with Auto topology persistence selected.
    pub(crate) fn with_auto(mut self, topology: DockTopologyByName) -> Self {
        self.auto = Some(topology);
        self
    }

    /// Attach the complete detection display authority for the serial settings writer.
    pub(crate) fn with_detects(mut self, detects: DetectViewFile) -> Self {
        self.detects = Some(detects);
        self
    }

    /// Capture chart specifications without JSON serialization on the UI thread.
    pub(crate) fn with_charts(mut self, charts: Vec<ChartTabSpec>) -> Self {
        self.charts = Some(charts);
        self
    }

    /// Attach the current local drawing snapshot.
    pub(crate) fn with_figures(mut self, figures: FigureSnapshot) -> Self {
        self.figures = Some(figures);
        self
    }

    /// Attach current tab preferences and read watermarks.
    pub(crate) fn with_badges(mut self, badges: TabBadgeSettings) -> Self {
        self.badges = Some(badges);
        self
    }

    /// Attach the bounded update history without adding a second mutable authority.
    pub(crate) fn with_updates(mut self, updates: CoreUpdateHistory) -> Self {
        self.updates = Some(updates);
        self
    }

    /// Attach config only after UID normalization reached the live authority.
    pub(crate) fn with_config(mut self, config: ConfigSnapshot) -> Self {
        self.config = Some(config);
        self
    }

    /// Return whether this request contains no selected persistence class.
    ///
    /// Returns:
    ///     `true` when dispatching the snapshot would perform no work.
    pub(crate) fn is_empty(&self) -> bool {
        self.layout.is_none()
            && self.classic.is_none()
            && self.auto.is_none()
            && self.detects.is_none()
            && self.charts.is_none()
            && self.figures.is_none()
            && self.badges.is_none()
            && self.updates.is_none()
            && self.config.is_none()
    }

    /// Describe the persistence classes selected by this snapshot.
    ///
    /// Returns:
    ///     Class mask copied into the eventual acknowledgement.
    fn classes(&self) -> PersistenceClasses {
        PersistenceClasses {
            layout: self.layout.is_some(),
            classic: self.classic.is_some(),
            auto: self.auto.is_some(),
            detects: self.detects.is_some(),
            charts: self.charts.is_some(),
            figures: self.figures.is_some(),
            badges: self.badges.is_some(),
            updates: self.updates.is_some(),
            config: self.config.is_some(),
        }
    }

    /// Retain only classes selected by a failed acknowledgement.
    ///
    /// Args:
    ///     classes: Failed class mask returned by the worker.
    ///
    /// Returns:
    ///     Snapshot containing exactly the authorities that still need a quit-only retry.
    fn retain_classes(mut self, classes: PersistenceClasses) -> Self {
        if !classes.layout {
            self.layout = None;
        }
        if !classes.classic {
            self.classic = None;
        }
        if !classes.auto {
            self.auto = None;
        }
        if !classes.detects {
            self.detects = None;
        }
        if !classes.charts {
            self.charts = None;
        }
        if !classes.figures {
            self.figures = None;
        }
        if !classes.badges {
            self.badges = None;
        }
        if !classes.updates {
            self.updates = None;
        }
        if !classes.config {
            self.config = None;
        }
        self
    }
}

/// Persistence classes carried by a request or acknowledgement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PersistenceClasses {
    /// Whether `layout.toml` belongs to the request.
    pub(crate) layout: bool,
    /// Whether the paired Classic state belongs to the request.
    pub(crate) classic: bool,
    /// Whether the shared Auto topology belongs to the request.
    pub(crate) auto: bool,
    /// Whether detection display preferences belong to the request.
    pub(crate) detects: bool,
    /// Whether chart specifications belong to the request.
    pub(crate) charts: bool,
    /// Whether local drawings belong to the request.
    pub(crate) figures: bool,
    /// Whether tab preferences and watermarks belong to the request.
    pub(crate) badges: bool,
    /// Whether core update history belongs to the request.
    pub(crate) updates: bool,
    /// Whether automatic configuration persistence belongs to the request.
    pub(crate) config: bool,
}

/// Result of one serial persistence request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PersistenceAck {
    /// Classes selected by the immutable request.
    pub(crate) classes: PersistenceClasses,
    /// Whether every selected class finished successfully.
    pub(crate) succeeded: PersistenceClasses,
}

impl PersistenceAck {
    /// Return the failed subset of the request for dirty-state restoration.
    ///
    /// Returns:
    ///     Selected classes whose background writer reported failure.
    pub(crate) fn failed(self) -> PersistenceClasses {
        PersistenceClasses {
            layout: self.classes.layout && !self.succeeded.layout,
            classic: self.classes.classic && !self.succeeded.classic,
            auto: self.classes.auto && !self.succeeded.auto,
            detects: self.classes.detects && !self.succeeded.detects,
            charts: self.classes.charts && !self.succeeded.charts,
            figures: self.classes.figures && !self.succeeded.figures,
            badges: self.classes.badges && !self.succeeded.badges,
            updates: self.classes.updates && !self.succeeded.updates,
            config: self.classes.config && !self.succeeded.config,
        }
    }
}

/// Blocking persistence implementation owned exclusively by the worker thread.
trait PersistenceSink: Send + 'static {
    /// Persist one complete layout snapshot.
    ///
    /// Args:
    ///     layout: Immutable layout authority captured by the application thread.
    ///
    /// Returns:
    ///     `true` only when the durable write succeeds.
    fn save_layout(&mut self, layout: &WindowLayout) -> bool;

    /// Persist one complete paired Classic snapshot.
    ///
    /// Args:
    ///     docks: Complete Classic dock topology and panel payload map.
    ///     detached: Complete Classic detached-panel ownership list.
    ///
    /// Returns:
    ///     `true` only when the journaled two-file commit succeeds.
    fn save_classic(&mut self, docks: &DockMap, detached: &[DetachedSpec]) -> bool;

    /// Persist one complete shared Auto topology.
    ///
    /// Args:
    ///     topology: Normalized topology-only authority captured by the application thread.
    ///
    /// Returns:
    ///     `true` only when the atomic write succeeds.
    fn save_auto(&mut self, topology: &DockTopologyByName) -> bool;

    /// Persist complete detection display preferences; false retains the authority for retry.
    fn save_detects(&mut self, detects: &DetectViewFile) -> bool;

    /// Persist the complete captured chart specification list.
    fn save_charts(&mut self, charts: &[ChartTabSpec]) -> bool;

    /// Persist local drawings without borrowing the live UI store.
    fn save_figures(&mut self, figures: &FigureSnapshot) -> bool;

    /// Persist tab preferences and read watermarks.
    fn save_badges(&mut self, badges: &TabBadgeSettings) -> bool;

    /// Persist a captured session update-history ring.
    fn save_updates(&mut self, updates: &CoreUpdateHistory) -> bool;

    /// Persist the complete normalized config using its existing multi-file contract.
    fn save_config(&mut self, config: &ConfigSnapshot) -> bool;
}

/// Production sink that performs the existing atomic file writes.
struct FilePersistenceSink;

impl PersistenceSink for FilePersistenceSink {
    /// Persist `layout.toml` through the canonical layout writer.
    ///
    /// Args:
    ///     layout: Immutable layout authority captured by the application thread.
    ///
    /// Returns:
    ///     `true` only when the atomic layout write succeeds.
    fn save_layout(&mut self, layout: &WindowLayout) -> bool {
        layout.save()
    }

    /// Persist Classic dock and detached ownership through their joint journal.
    ///
    /// Args:
    ///     docks: Complete Classic dock topology and panel payload map.
    ///     detached: Complete Classic detached-panel ownership list.
    ///
    /// Returns:
    ///     `true` only when the joint commit succeeds.
    fn save_classic(&mut self, docks: &DockMap, detached: &[DetachedSpec]) -> bool {
        super::window_state_persist::save_all(docks, detached)
    }

    /// Persist `auto_dock.json` through its canonical atomic writer.
    fn save_auto(&mut self, topology: &DockTopologyByName) -> bool {
        super::auto_dock_persist::save(topology)
    }

    /// Use the canonical atomic writer on the existing serial persistence thread.
    fn save_detects(&mut self, detects: &DetectViewFile) -> bool {
        detects.save()
    }

    /// Serialize and atomically write chart specifications on this worker.
    fn save_charts(&mut self, charts: &[ChartTabSpec]) -> bool {
        super::chart_persist::save_all(charts)
    }

    /// Serialize and atomically write captured local drawings on this worker.
    fn save_figures(&mut self, figures: &FigureSnapshot) -> bool {
        figures.save()
    }

    /// Keep badge write failures visible to the live retry loop.
    fn save_badges(&mut self, badges: &TabBadgeSettings) -> bool {
        badges.save()
    }

    /// Keep update-history failures pending without blocking later classes.
    fn save_updates(&mut self, updates: &CoreUpdateHistory) -> bool {
        if let Err(error) = updates.save() {
            log::warn!("core update history save failed: {error}");
            false
        } else {
            true
        }
    }

    /// Preserve config failure reporting so the latest live authority is retried.
    fn save_config(&mut self, config: &ConfigSnapshot) -> bool {
        if let Err(error) = config.save() {
            log::warn!("config autosave failed: {error}");
            false
        } else {
            true
        }
    }
}

/// Commands consumed in order by the single persistence worker.
enum WorkerCommand {
    /// Ordinary debounced work whose acknowledgement is polled by the live loop.
    Persist(PersistenceSnapshot),
    /// Final full snapshot followed by worker termination and a synchronous acknowledgement.
    Shutdown(PersistenceSnapshot, Sender<PersistenceAck>),
}

/// Minimum spacing between two live snapshot dispatches: a drag or resize dirties the layout every
/// tick, and each dispatch clones every authority.
pub(crate) const DOCK_DISPATCH_INTERVAL: Duration = Duration::from_millis(500);

/// Frequent chart edits retain the existing one-second autosave limit.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Whether a snapshot may be dispatched at `now`, given when the previous one was accepted.
pub(crate) fn dock_dispatch_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= DOCK_DISPATCH_INTERVAL)
}

/// One serial standard-thread worker with at most one debounced request in flight.
pub(crate) struct PersistenceCoordinator {
    commands: Option<Sender<WorkerCommand>>,
    acknowledgements: Receiver<PersistenceAck>,
    worker: Option<JoinHandle<()>>,
    in_flight: Option<PersistenceClasses>,
    /// When the last debounced snapshot was accepted, for [`DOCK_DISPATCH_INTERVAL`].
    last_dispatch: Option<Instant>,
    /// Last autosave attempt, independent of layout traffic; invalid snapshots are also rate-limited.
    last_autosave: Option<Instant>,
    /// Independent sink used only if the worker cannot perform the final quit snapshot.
    fallback: Box<dyn PersistenceSink>,
}

impl PersistenceCoordinator {
    /// Start the production persistence worker.
    ///
    /// Returns:
    ///     Coordinator used exclusively by the GPUI application thread.
    pub(crate) fn start() -> Self {
        Self::with_sinks(Box::new(FilePersistenceSink), Box::new(FilePersistenceSink))
    }

    /// Start one named worker around an injected blocking sink.
    ///
    /// Args:
    ///     sink: File or test implementation that performs serial writes.
    ///
    /// Returns:
    ///     Coordinator whose dispatch side never executes sink I/O.
    fn with_sinks(mut sink: Box<dyn PersistenceSink>, fallback: Box<dyn PersistenceSink>) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (ack_tx, ack_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("moon-persistence".to_string())
            .spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    match command {
                        WorkerCommand::Persist(snapshot) => {
                            let acknowledgement = persist_snapshot(&mut *sink, &snapshot);
                            if ack_tx.send(acknowledgement).is_err() {
                                break;
                            }
                        }
                        WorkerCommand::Shutdown(snapshot, final_ack_tx) => {
                            let acknowledgement = persist_snapshot(&mut *sink, &snapshot);
                            let _ = final_ack_tx.send(acknowledgement);
                            break;
                        }
                    }
                }
            });
        if let Err(error) = &worker {
            log::warn!("could not start persistence worker: {error}");
        }
        let worker = worker.ok();
        Self {
            commands: worker.as_ref().map(|_| command_tx),
            acknowledgements: ack_rx,
            worker,
            in_flight: None,
            last_dispatch: None,
            last_autosave: None,
            fallback,
        }
    }

    /// Queue one debounced snapshot without waiting for file I/O.
    ///
    /// A second request is rejected until the live loop consumes the first acknowledgement. The
    /// caller therefore keeps later mutations dirty and submits one newer coalesced snapshot.
    ///
    /// Args:
    ///     snapshot: Immutable selected authorities captured on the application thread.
    ///
    /// Returns:
    ///     `true` only when a non-empty request was accepted by an idle worker channel.
    pub(crate) fn dispatch(&mut self, snapshot: PersistenceSnapshot) -> bool {
        if snapshot.is_empty() || self.in_flight.is_some() {
            return false;
        }
        let classes = snapshot.classes();
        let Some(commands) = &self.commands else {
            return false;
        };
        if commands.send(WorkerCommand::Persist(snapshot)).is_err() {
            return false;
        }
        self.in_flight = Some(classes);
        self.last_dispatch = Some(Instant::now());
        true
    }

    /// Whether the dispatch window since the last accepted snapshot has elapsed at `now`.
    pub(crate) fn dispatch_due(&self, now: Instant) -> bool {
        dock_dispatch_due(self.last_dispatch, now)
    }

    /// Admit a coalesced autosave without repeatedly cloning dirty models while the gate is closed.
    pub(crate) fn admit_autosave(&mut self, now: Instant) -> bool {
        if self
            .last_autosave
            .is_some_and(|last| now.saturating_duration_since(last) < AUTOSAVE_INTERVAL)
        {
            return false;
        }
        self.last_autosave = Some(now);
        true
    }

    /// Poll the one outstanding debounced acknowledgement without blocking GPUI.
    ///
    /// Returns:
    ///     Completed acknowledgement, or `None` while the worker is busy or idle.
    pub(crate) fn poll(&mut self) -> Option<PersistenceAck> {
        let classes = self.in_flight?;
        match self.acknowledgements.try_recv() {
            Ok(acknowledgement) => {
                self.in_flight = None;
                Some(acknowledgement)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.in_flight = None;
                Some(PersistenceAck {
                    classes,
                    succeeded: PersistenceClasses::default(),
                })
            }
        }
    }

    /// Return whether a debounced snapshot is still awaiting acknowledgement.
    ///
    /// Returns:
    ///     `true` while the worker owns a previously dispatched live request.
    pub(crate) fn is_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Quit must include the latest config when an older config write has not been acknowledged.
    pub(crate) fn has_pending_config(&self) -> bool {
        self.in_flight.is_some_and(|classes| classes.config)
    }

    /// Finish earlier autosaves before a deliberate synchronous config/key change.
    /// The live tick uses `poll`; only explicit save operations may wait for disk here.
    pub(crate) fn finish_pending(&mut self) -> Option<PersistenceAck> {
        let classes = self.in_flight.take()?;
        Some(self.acknowledgements.recv().unwrap_or(PersistenceAck {
            classes,
            succeeded: PersistenceClasses::default(),
        }))
    }

    /// Queue the latest full snapshot behind prior work, then join at the final durability boundary.
    ///
    /// Args:
    ///     snapshot: Latest complete persistence authorities captured during quit.
    ///
    /// Returns:
    ///     Result of the final snapshot, independent of any superseded in-flight acknowledgement.
    pub(crate) fn shutdown(&mut self, snapshot: PersistenceSnapshot) -> PersistenceAck {
        let classes = snapshot.classes();
        let fallback_snapshot = snapshot.clone();
        let (final_ack_tx, final_ack_rx) = mpsc::channel();
        let sent = self.commands.take().is_some_and(|commands| {
            commands
                .send(WorkerCommand::Shutdown(snapshot, final_ack_tx))
                .is_ok()
        });
        let mut acknowledgement = if sent {
            final_ack_rx.recv().unwrap_or(PersistenceAck {
                classes,
                succeeded: PersistenceClasses::default(),
            })
        } else {
            PersistenceAck {
                classes,
                succeeded: PersistenceClasses::default(),
            }
        };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let failed = acknowledgement.failed();
        if failed != PersistenceClasses::default() {
            log::warn!("persistence worker missed final snapshot; retrying failed classes on quit");
            acknowledgement = persist_snapshot(
                &mut *self.fallback,
                &fallback_snapshot.retain_classes(failed),
            );
        }
        self.in_flight = None;
        acknowledgement
    }
}

impl Drop for PersistenceCoordinator {
    /// Stop and join the worker if application shutdown did not already do so.
    fn drop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        let _ = self.shutdown(PersistenceSnapshot::empty());
    }
}

/// Run one immutable request entirely on the persistence worker.
///
/// Args:
///     sink: Blocking writer owned by the current worker thread.
///     snapshot: Selected immutable authorities for this request.
///
/// Returns:
///     Per-class acknowledgement used to restore failed dirty flags.
fn persist_snapshot(
    sink: &mut dyn PersistenceSink,
    snapshot: &PersistenceSnapshot,
) -> PersistenceAck {
    let classes = snapshot.classes();
    let layout = snapshot
        .layout
        .as_ref()
        .is_some_and(|layout| sink.save_layout(layout));
    let classic = snapshot
        .classic
        .as_ref()
        .is_some_and(|(docks, detached)| sink.save_classic(docks, detached));
    let auto = snapshot
        .auto
        .as_ref()
        .is_some_and(|topology| sink.save_auto(topology));
    let detects = snapshot
        .detects
        .as_ref()
        .is_some_and(|detects| sink.save_detects(detects));
    let charts = snapshot
        .charts
        .as_ref()
        .is_some_and(|charts| sink.save_charts(charts));
    let figures = snapshot
        .figures
        .as_ref()
        .is_some_and(|figures| sink.save_figures(figures));
    let badges = snapshot
        .badges
        .as_ref()
        .is_some_and(|badges| sink.save_badges(badges));
    let updates = snapshot
        .updates
        .as_ref()
        .is_some_and(|updates| sink.save_updates(updates));
    let config = snapshot
        .config
        .as_ref()
        .is_some_and(|config| sink.save_config(config));
    PersistenceAck {
        classes,
        succeeded: PersistenceClasses {
            layout,
            classic,
            auto,
            detects,
            charts,
            figures,
            badges,
            updates,
            config,
        },
    }
}
