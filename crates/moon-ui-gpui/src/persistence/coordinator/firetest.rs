//! Exercise the live autosave dispatcher while its serial writer is blocked and fails once.

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc::{self, Receiver, Sender},
};

use moon_core::{
    figures::{FigureKind, FigureStore},
    session::CoreId,
};

use super::*;
use crate::{Backend, diag};

/// Progress published by the worker; the application thread never waits for it.
struct SlowFigures {
    state: Arc<AtomicU8>,
    release: Receiver<()>,
}

impl PersistenceSink for SlowFigures {
    /// Keep the ordinary writer for every class outside the injected drawing failure.
    fn save_layout(&mut self, value: &WindowLayout) -> bool {
        FilePersistenceSink.save_layout(value)
    }
    /// Preserve the production paired-file transaction.
    fn save_classic(&mut self, docks: &DockMap, detached: &[DetachedSpec]) -> bool {
        FilePersistenceSink.save_classic(docks, detached)
    }
    /// Preserve the production topology writer.
    fn save_auto(&mut self, value: &DockTopologyByName) -> bool {
        FilePersistenceSink.save_auto(value)
    }
    /// Preserve the production detection writer.
    fn save_detects(&mut self, value: &DetectViewFile) -> bool {
        FilePersistenceSink.save_detects(value)
    }
    /// Preserve the production chart writer.
    fn save_charts(&mut self, value: &[ChartTabSpec]) -> bool {
        FilePersistenceSink.save_charts(value)
    }
    /// Fail a slow first write, then commit the next immutable snapshot to the real fixture file.
    fn save_figures(&mut self, value: &FigureSnapshot) -> bool {
        if self.state.load(Ordering::Acquire) == 0 {
            let started = std::time::Instant::now();
            self.state.store(1, Ordering::Release);
            std::thread::sleep(Duration::from_millis(600));
            // Software rendering can take longer than 600 ms. Keep the writer blocked until
            // the UI has witnessed progress, but leave time for retry inside the existing storm.
            let remaining = Duration::from_secs(4).saturating_sub(started.elapsed());
            if self.release.recv_timeout(remaining).is_err() {
                self.state.store(4, Ordering::Release);
                return false;
            }
            self.state.store(2, Ordering::Release);
            return false;
        }
        let saved = FilePersistenceSink.save_figures(value);
        if saved {
            self.state.store(3, Ordering::Release);
        }
        saved
    }
    /// Preserve the production badge writer.
    fn save_badges(&mut self, value: &TabBadgeSettings) -> bool {
        FilePersistenceSink.save_badges(value)
    }
    /// Preserve the production update-history writer.
    fn save_updates(&mut self, value: &CoreUpdateHistory) -> bool {
        FilePersistenceSink.save_updates(value)
    }
    /// Preserve config validation and encryption.
    fn save_config(&mut self, value: &ConfigSnapshot) -> bool {
        FilePersistenceSink.save_config(value)
    }
}

/// Evidence from a real fixture figure, the actual UI loop, and the final persisted file.
pub(crate) struct AutosaveProbe {
    state: Arc<AtomicU8>,
    release: Sender<()>,
    released: bool,
    core: CoreId,
    figure: u64,
    blocked_ticks: usize,
    previous_counters: [u64; 2],
    progress: [bool; 2],
    persisted: bool,
}

impl AutosaveProbe {
    /// Install one injected writer in the existing coordinator, only for the isolated dense fixture.
    pub(crate) fn start(backend: &mut Backend, core: CoreId) -> Result<Self, String> {
        let market = moon_core::fixture::dense::MARKETS[1];
        let figure = backend
            .figures
            .borrow()
            .figures(core, market)
            .iter()
            .find(|figure| matches!(figure.kind, FigureKind::HLine(_)))
            .map(|figure| figure.id)
            .ok_or("dense fixture has no horizontal drawing")?;
        let state = Arc::new(AtomicU8::new(0));
        let (release, wait_for_progress) = mpsc::channel();
        let coordinator = backend.persistence.clone();
        if coordinator.borrow().is_in_flight() {
            return Err("autosave probe cannot replace an active writer".into());
        }
        *coordinator.borrow_mut() = PersistenceCoordinator::with_sinks(
            Box::new(SlowFigures {
                state: state.clone(),
                release: wait_for_progress,
            }),
            Box::new(FilePersistenceSink),
        );
        let probe = Self {
            state,
            release,
            released: false,
            core,
            figure,
            blocked_ticks: 0,
            previous_counters: Self::counters(),
            progress: [false; 2],
            persisted: false,
        };
        probe.edit(backend, 104.125);
        crate::startup::dispatch_live_persistence(backend, &mut coordinator.borrow_mut());
        Ok(probe)
    }

    /// Read actual drawing and fast-input work, allowing the normal one-second diagnostic reset.
    fn counters() -> [u64; 2] {
        [
            diag::CHART_PRESENT.load(Ordering::Relaxed),
            diag::CHART_MOUSE_MOVE.load(Ordering::Relaxed),
        ]
    }

    /// Change the live figure through its ordinary dirty/revision owner.
    fn edit(&self, backend: &mut Backend, price: f64) {
        backend.figures.borrow_mut().edit(
            self.core,
            moon_core::fixture::dense::MARKETS[1],
            self.figure,
            |figure| {
                let FigureKind::HLine(line) = &mut figure.kind else {
                    return false;
                };
                line.price = price;
                true
            },
        );
    }

    /// Pump the same dispatcher used by startup and observe progress while its worker sleeps.
    pub(crate) fn tick(&mut self, backend: &mut Backend) {
        if self.persisted {
            return;
        }
        if self.state.load(Ordering::Acquire) == 1 {
            if self.blocked_ticks == 0 {
                self.edit(backend, 104.5);
            }
            let counters = Self::counters();
            // The first observation starts the interval; earlier work cannot prove progress
            // while this writer is blocked.
            if self.blocked_ticks > 0 {
                for (ix, current) in counters.into_iter().enumerate() {
                    self.progress[ix] |= current > self.previous_counters[ix];
                }
            }
            self.blocked_ticks += 1;
            self.previous_counters = counters;
            if self.blocked_ticks >= 3 && self.progress == [true; 2] && !self.released {
                self.released = self.release.send(()).is_ok();
            }
        }
        let coordinator = backend.persistence.clone();
        crate::startup::dispatch_live_persistence(backend, &mut coordinator.borrow_mut());
        if self.state.load(Ordering::Acquire) == 3 && !coordinator.borrow().is_in_flight() {
            self.persisted = FigureStore::load()
                .get(
                    self.core,
                    moon_core::fixture::dense::MARKETS[1],
                    self.figure,
                )
                .is_some_and(
                    |figure| matches!(&figure.kind, FigureKind::HLine(line) if line.price == 104.5),
                );
        }
    }

    /// Require frames, input, a mutation during the failed write, and its latest durable value.
    pub(crate) fn verify(&self) -> Result<usize, String> {
        if !self.released || self.progress != [true; 2] || !self.persisted {
            return Err(format!(
                "autosave blocked_ticks={} frames/input={:?} released={} latest_persisted={}",
                self.blocked_ticks, self.progress, self.released, self.persisted
            ));
        }
        Ok(self.blocked_ticks)
    }
}
