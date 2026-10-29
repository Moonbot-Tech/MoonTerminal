//! Background release discovery; chat status reads the last completed result without waiting.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use moon_core::update::{BuildIdentity, ReleaseDiscovery, UpdateEligibility, station_asset_name};
use moon_tg::{ReleaseCheck, ReleaseFailure};

/// One conditional-discovery session and its independently published result.
#[derive(Default)]
pub struct ReleaseWatch {
    discovery: Arc<Mutex<Option<ReleaseDiscovery>>>,
    latest: Arc<Mutex<Option<ReleaseCheck>>>,
    worker: Option<JoinHandle<()>>,
    next_refresh: Option<SystemTime>,
}

impl ReleaseWatch {
    /// Start immediately, then at the station's existing UTC release-check slots.
    /// A stale status request calls this same method and starts a refresh without awaiting it.
    pub fn refresh_if_due(&mut self, now: SystemTime) {
        let discovery = self.discovery.clone();
        self.refresh_with(now, move || check(&discovery));
    }

    /// Return the last completed discovery, or the existing timeout presentation before the first.
    pub fn latest(&self) -> ReleaseCheck {
        self.latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or(ReleaseCheck::Failed(ReleaseFailure::Timeout))
    }

    /// Admit one background lookup, retaining the prior result throughout slow upstream I/O.
    fn refresh_with(
        &mut self,
        now: SystemTime,
        lookup: impl FnOnce() -> ReleaseCheck + Send + 'static,
    ) {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = self.worker.take().expect("finished worker").join();
        }
        if self.worker.is_some() || self.next_refresh.is_some_and(|at| now < at) {
            return;
        }
        let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        self.next_refresh = Some(
            UNIX_EPOCH
                + std::time::Duration::from_secs(seconds)
                + crate::auto_update::until_next_slot(seconds),
        );
        let latest = self.latest.clone();
        match std::thread::Builder::new()
            .name("release-check".into())
            .spawn(move || {
                let result = lookup();
                *latest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
            }) {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => {
                *self
                    .latest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(ReleaseCheck::Failed(ReleaseFailure::Unavailable(format!(
                        "no thread for the check: {error}"
                    ))));
            }
        }
    }
}

impl Drop for ReleaseWatch {
    /// Join the one bounded discovery job when its station owner shuts down.
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Perform the unchanged GitHub discovery on its worker, never holding the published-result lock.
fn check(discovery: &Mutex<Option<ReleaseDiscovery>>) -> ReleaseCheck {
    let Some(asset) = station_asset_name() else {
        return ReleaseCheck::Failed(ReleaseFailure::UnsupportedArchitecture(
            std::env::consts::ARCH.into(),
        ));
    };
    let identity = BuildIdentity::from_release_base(super::release_base());
    if identity.baseline().is_none() {
        return ReleaseCheck::Unversioned;
    }
    let Ok(mut discovery) = discovery.lock() else {
        return ReleaseCheck::Failed(ReleaseFailure::Unavailable(
            "the release check failed earlier".into(),
        ));
    };
    let discovery = discovery.get_or_insert_with(|| ReleaseDiscovery::for_asset(identity, asset));
    match discovery.scan() {
        Ok(result) => match result.eligibility {
            UpdateEligibility::Available(release) => {
                ReleaseCheck::Newer(release.version().to_string())
            }
            UpdateEligibility::Current => ReleaseCheck::Current,
            UpdateEligibility::Unsupported => ReleaseCheck::Unversioned,
        },
        Err(error) => ReleaseCheck::Failed(ReleaseFailure::Unavailable(error.to_string())),
    }
}

#[cfg(test)]
mod tests;
