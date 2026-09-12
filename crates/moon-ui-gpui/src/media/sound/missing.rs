//! Notices about sounds that were asked for and could not be found, so a fallback never plays in
//! silence about WHY it was the fallback.
//!
//! A strategy names `MYSOUND`, a core's setting says sound number 23, a trade sound's file was
//! deleted: each plays the default, and the operator hears the wrong noise with no way to tell
//! which setting is at fault. The notice names it, once per name per session — the same detect can
//! fire fifty times a night, and the second toast teaches nothing the first did not.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};

/// What was asked for and not found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MissingSound {
    /// A stem from a strategy, a trade-sound setting, a warn setting or the alerts panel.
    Name(String),
    /// A Moonbot ordinal from a core's settings, past the terminal's sound table.
    Ordinal(i32),
}

impl MissingSound {
    /// The dedupe key: one notice per distinct SOUND for the session. A name is keyed by its stem
    /// — trimmed, lowercase, `.wav` stripped — the way the catalog looks it up, so a strategy's
    /// `MYSOUND` and a trade-sound setting's `mysound.wav` are one missing sound, not two toasts.
    fn key(&self) -> String {
        match self {
            Self::Name(name) => {
                let low = name.trim().to_ascii_lowercase();
                format!("name:{}", low.strip_suffix(".wav").unwrap_or(&low))
            }
            Self::Ordinal(n) => format!("ordinal:{n}"),
        }
    }
}

#[derive(Default)]
struct State {
    /// Notices not yet shown, in the order they arose.
    queue: VecDeque<MissingSound>,
    /// Every key already queued this session.
    seen: HashSet<String>,
    /// Requests that failed BEFORE the folder scan landed. The embedded set alone cannot say a
    /// name is missing — the folder may hold it — so these wait, and the ones the scanned catalog
    /// still cannot answer become notices then.
    pending: Vec<MissingSound>,
}

thread_local! {
    /// Same thread as the player: every caller is on the GPUI thread.
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Records a miss. Before the first scan it is held back rather than reported.
pub(super) fn note(missing: MissingSound, scanned: bool) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !scanned {
            state.pending.push(missing);
            return;
        }
        push(&mut state, missing);
    });
}

/// Re-checks everything held back before the scan; `still_missing` answers against the catalog
/// that just landed.
pub(super) fn settle_pending(still_missing: impl Fn(&MissingSound) -> bool) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let pending = std::mem::take(&mut state.pending);
        for missing in pending {
            if still_missing(&missing) {
                push(&mut state, missing);
            }
        }
    });
}

fn push(state: &mut State, missing: MissingSound) {
    if state.seen.insert(missing.key()) {
        state.queue.push_back(missing);
    }
}

/// Drains the notices for display. The shell calls this on its tick and turns each into a toast.
pub(crate) fn take() -> Vec<MissingSound> {
    STATE.with(|state| state.borrow_mut().queue.drain(..).collect())
}

/// Forgets which names were already reported, so a rescan — the user just added the file — can
/// report a name again if it is STILL missing afterwards.
pub(super) fn reset_seen() {
    STATE.with(|state| state.borrow_mut().seen.clear());
}
