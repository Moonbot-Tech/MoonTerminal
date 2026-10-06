//! Strategy edit watches and shell toast outcomes.

use crate::Backend;
use moon_core::feed::StrategyEditOutcome;
use moon_core::feed::StrategyEditResult;
use moon_core::feed::StrategyFieldChange;
use moon_core::session::CoreId;
use std::collections::HashMap;

impl Backend {
    /// Register interest in one strategy edit so the shell can report a non-clean outcome.
    ///
    /// The coin menu is fire-and-forget with no surface of its own; this is the only place an
    /// edit it dispatched can become visible. Self-draining: an entry is dropped as soon as the
    /// edit resolves or its core disappears.
    pub fn watch_strategy_edit(&mut self, core: CoreId, id: u64, coin: String) {
        self.strategy_edit_watches.push(PendingStrategyEditWatch {
            core,
            id,
            coin,
            sent: false,
            timed_out_shown: false,
        });
    }

    /// Watch one strategy edit's OUTCOME without announcing that it was sent.
    ///
    /// For a control that already shows what was chosen the moment it is clicked: the send needs no
    /// toast, but an Adjusted, Superseded or TimedOut answer still has nowhere else to appear.
    pub(crate) fn watch_strategy_edit_quiet(&mut self, core: CoreId, id: u64, label: String) {
        self.strategy_edit_watches.push(PendingStrategyEditWatch {
            core,
            id,
            coin: label,
            sent: true,
            timed_out_shown: false,
        });
    }

    /// Drain queued coin-menu strategy-edit outcomes into toast events, advancing each watched
    /// core's note cursor.
    ///
    /// The split ownership across this feature is deliberate, so nobody adds a second pump for
    /// the same signal: the params panel reads `CoreData` directly and renders in place (no
    /// toast); the Analytics window owns its own banner and toast because `push_notification`
    /// targets the ACTIVE window and Analytics is a separate one; this queue is the only source
    /// for the shell's main-window toast, drained by `Shell::drain_strategy_edit_toasts`.
    pub(crate) fn take_strategy_edit_toasts(&mut self) -> Vec<StrategyEditToast> {
        let mut events = Vec::new();

        for watch in &mut self.strategy_edit_watches {
            if !watch.sent {
                watch.sent = true;
                events.push(StrategyEditToast::Sent {
                    coin: watch.coin.clone(),
                });
            }
        }

        let watches = std::mem::take(&mut self.strategy_edit_watches);

        // Group the surviving watches by core so every watch on a core sees the SAME batch of
        // unseen notes -- one watch's scan must never advance another watch's core cursor past a
        // note that watch has not had the chance to consume yet.
        let mut by_core: HashMap<CoreId, Vec<PendingStrategyEditWatch>> = HashMap::new();
        for watch in watches {
            if self.session.store().core(watch.core).is_none() {
                continue; // core disappeared; drop silently
            }
            by_core.entry(watch.core).or_default().push(watch);
        }

        let mut kept = Vec::new();
        for (core, core_watches) in by_core {
            let cursor = *self.strategy_edit_note_cursor.get(&core).unwrap_or(&0);
            let mut batch_max = cursor;
            // `store()` is re-fetched per core rather than hoisted: different cores need
            // different `CoreData`, and the immutable borrow must end before the cursor map (a
            // different field) is written below.
            let mut notes = Vec::new();
            if let Some(core_data) = self.session.store().core(core) {
                for note in core_data.strategy_edit_notes_since(cursor) {
                    batch_max = batch_max.max(note.seq);
                    notes.push(note.clone());
                }
            }
            self.strategy_edit_note_cursor.insert(core, batch_max);

            for mut watch in core_watches {
                let outcome = self
                    .session
                    .store()
                    .core(watch.core)
                    .map(|cd| cd.resolve_strategy_edit(watch.id, cursor))
                    .unwrap_or(StrategyEditOutcome::Pending);
                match outcome {
                    StrategyEditOutcome::Resolved(StrategyEditResult::Confirmed) => continue,
                    StrategyEditOutcome::Resolved(StrategyEditResult::Adjusted) => {
                        let changes = notes
                            .iter()
                            .find(|note| note.id == watch.id)
                            .map(|note| note.changes.clone())
                            .unwrap_or_default();
                        events.push(StrategyEditToast::Adjusted {
                            coin: watch.coin.clone(),
                            changes,
                        });
                        continue; // resolved; drop the watch
                    }
                    StrategyEditOutcome::Resolved(StrategyEditResult::Superseded) => {
                        events.push(StrategyEditToast::Superseded {
                            coin: watch.coin.clone(),
                        });
                        continue; // resolved; drop the watch
                    }
                    StrategyEditOutcome::TimedOut => {
                        if !watch.timed_out_shown {
                            watch.timed_out_shown = true;
                            events.push(StrategyEditToast::TimedOut {
                                coin: watch.coin.clone(),
                            });
                        }
                    }
                    StrategyEditOutcome::Pending => {}
                }
                kept.push(watch);
            }
        }
        self.strategy_edit_watches = kept;

        events
    }
}

/// One coin-menu strategy edit awaiting resolution, so [`Backend::take_strategy_edit_toasts`]
/// can still report it once resolved. Self-draining: dropped as soon as a resolved note for its
/// id arrives, or its core disappears from the store.
pub(crate) struct PendingStrategyEditWatch {
    core: CoreId,
    id: u64,
    coin: String,
    /// Whether the "sent" toast has already been queued for this watch.
    sent: bool,
    /// Whether the one-shot `TimedOut` warning has already been queued. `TimedOut` is not
    /// terminal upstream -- a late core echo can still confirm, adjust or supersede it -- so the
    /// watch stays open after this fires instead of being dropped.
    timed_out_shown: bool,
}

/// One coin-menu strategy-edit outcome ready to become a toast. `Backend` never constructs
/// `MoonNotification` or touches a window -- that stays in `Shell::drain_strategy_edit_toasts`,
/// the only place with window access.
pub(crate) enum StrategyEditToast {
    Sent {
        coin: String,
    },
    Adjusted {
        coin: String,
        changes: Vec<StrategyFieldChange>,
    },
    Superseded {
        coin: String,
    },
    TimedOut {
        coin: String,
    },
}
