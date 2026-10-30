//! A small persistent read pool; completions still run on the station owner's thread.

use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::Thread;

use moon_tg::{Finish, Job};

/// Read jobs and their completion mailbox, with the same owner wake as the former per-read threads.
pub(super) struct ReadWorker {
    jobs: mpsc::Sender<Job>,
    finished: mpsc::Sender<Finish>,
    owner: Thread,
}

impl ReadWorker {
    /// Start two readers so one lengthy chart read does not serialize every other request.
    /// The host retains ownership of admission and each completion's generation checks.
    pub(super) fn start() -> (Self, mpsc::Receiver<Finish>) {
        let (jobs, incoming) = mpsc::channel::<Job>();
        let incoming = Arc::new(Mutex::new(incoming));
        let (finished, completed) = mpsc::channel::<Finish>();
        let owner = std::thread::current();
        for index in 0..2 {
            let incoming = incoming.clone();
            let finished = finished.clone();
            let owner = owner.clone();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("tg-read-{index}"))
                .spawn(move || {
                    loop {
                        // Release the receiver lock before running a job, so both readers can work.
                        let job = incoming
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .recv();
                        let Ok(job) = job else { return };
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) {
                            Ok(finish) => {
                                if finished.send(finish).is_err() {
                                    return;
                                }
                                owner.unpark();
                            }
                            Err(_) => log::error!("telegram: a background read panicked"),
                        }
                    }
                })
            {
                log::warn!("telegram: cannot start reader {index}: {error}");
            }
        }
        (
            Self {
                jobs,
                finished,
                owner,
            },
            completed,
        )
    }

    /// Enqueue a read, falling back in place if all workers have gone, so no Busy latch is lost.
    pub(super) fn spawn(&self, job: Job) {
        if let Err(mpsc::SendError(job)) = self.jobs.send(job) {
            log::warn!("telegram: no read worker; running the read in place");
            let _ = self.finished.send(job());
            self.owner.unpark();
        }
    }
}

#[cfg(test)]
mod tests;
