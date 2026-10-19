use super::*;

/// Test seams the search calls under `cfg(test)`: they live here so the logic file stays logic.
impl SearchHandle {
    /// Arm a [`Self::finish`] the moment the search first holds an answer, so a test can stop it
    /// between a scored point and the end of its first restart.
    pub(crate) fn finish_on_answer(&self) {
        self.0.finish_on_answer.store(true, Ordering::Relaxed);
    }

    /// Arm a hard cancel at the same scored-point boundary as the keep-stop test seam.
    pub(crate) fn cancel_on_answer(&self) {
        self.0.cancel_on_answer.store(true, Ordering::Relaxed);
        self.finish_on_answer();
    }

    /// Fire the armed [`Self::finish_on_answer`] once `holds` says the search has an answer;
    /// `holds` is not called while nothing is armed.
    pub(crate) fn answered(&self, holds: impl FnOnce() -> bool) {
        if self.0.finish_on_answer.load(Ordering::Relaxed) && holds() {
            self.0.finish_on_answer.store(false, Ordering::Relaxed);
            if self.0.cancel_on_answer.load(Ordering::Relaxed) {
                self.cancel();
            } else {
                self.finish();
            }
        }
    }
}

/// A Stop followed by a scope change must abort the trim again, not keep finishing for nobody.
#[test]
fn cancel_after_finish_drops_the_keep() {
    let handle = SearchHandle::new();
    handle.finish();
    assert!(handle.is_cancelled() && handle.keeps());
    assert!(!handle.aborts_trim());
    handle.cancel();
    assert!(handle.is_cancelled() && !handle.keeps());
    assert!(handle.aborts_trim());
}
