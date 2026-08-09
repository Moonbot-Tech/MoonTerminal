//! Serial background persistence for layout and Classic window ownership snapshots.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use moon_core::config::WindowLayout;
use moon_ui::DockTopologyByName;

use crate::persistence::dock_persist::DockMap;
use crate::window::detached::DetachedSpec;

#[cfg(test)]
mod tests;

/// Immutable values written by one serial persistence request.
#[derive(Clone)]
pub(crate) struct PersistenceSnapshot {
    /// Complete `layout.toml` authority when layout state was dirty.
    layout: Option<WindowLayout>,
    /// Complete paired Classic dock and detached-window authority when either side was dirty.
    classic: Option<(DockMap, Vec<DetachedSpec>)>,
    /// Complete shared Auto topology when its persistence authority was dirty.
    auto: Option<DockTopologyByName>,
}

impl PersistenceSnapshot {
    /// Build an empty request that performs no file writes.
    ///
    /// Returns:
    ///     A snapshot with neither persistence class selected.
    pub(crate) fn empty() -> Self {
        Self {
            layout: None,
            classic: None,
            auto: None,
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

    /// Return whether this request contains no selected persistence class.
    ///
    /// Returns:
    ///     `true` when dispatching the snapshot would perform no work.
    pub(crate) fn is_empty(&self) -> bool {
        self.layout.is_none() && self.classic.is_none() && self.auto.is_none()
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
}

/// Commands consumed in order by the single persistence worker.
enum WorkerCommand {
    /// Ordinary debounced work whose acknowledgement is polled by the live loop.
    Persist(PersistenceSnapshot),
    /// Final full snapshot followed by worker termination and a synchronous acknowledgement.
    Shutdown(PersistenceSnapshot, Sender<PersistenceAck>),
}

/// One serial standard-thread worker with at most one debounced request in flight.
pub(crate) struct PersistenceCoordinator {
    commands: Option<Sender<WorkerCommand>>,
    acknowledgements: Receiver<PersistenceAck>,
    worker: Option<JoinHandle<()>>,
    in_flight: Option<PersistenceClasses>,
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

    /// Queue the latest full snapshot behind prior work, then join at the final durability boundary.
    ///
    /// Args:
    ///     snapshot: Latest complete layout and Classic authorities captured during quit.
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
    PersistenceAck {
        classes,
        succeeded: PersistenceClasses {
            layout,
            classic,
            auto,
        },
    }
}
