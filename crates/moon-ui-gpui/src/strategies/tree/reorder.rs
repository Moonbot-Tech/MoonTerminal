//! Moving strategies up and down inside their folder, and holding the result on screen until the
//! core confirms it.
//!
//! A core's strategy list is an ORDER, not a set: the operator arranges it in MoonBot and moonproto
//! synchronizes that arrangement as the row sequence of a Full snapshot. The permutation itself is
//! [`ops::reorder_step`]; this module is the part that has a window — which rows a press acts on,
//! the command that carries the result to the core, and the overlay below.
//!
//! ## Why the overlay
//!
//! `sync_local_strategies` does NOT reorder moonproto's retained list. The library keeps the
//! core-confirmed order and rewrites it only from the core's own Full echo (`apply_server_order`),
//! so between the press and that echo `CoreData::strategies` still holds the OLD sequence. Drawn
//! straight, the tree would sit still for the whole round trip and — far worse — a second press
//! would be computed from the arrangement the first one already replaced, so holding the button
//! would produce one move instead of five. [`PendingOrder`] is that gap, and nothing more: it
//! overlays the sequence already sent, and it is dropped the moment the core answers.
//!
//! ## What it does not do
//!
//! Nothing tells the window that a reorder was REFUSED. The command's outcome lives on the feed
//! thread — a core whose state is not ready, a send that fails — and reaches only the log. Such an
//! overlay is retired by [`CONFIRMATION_WINDOW`] instead of by an answer, so the arrangement on
//! screen reverts silently rather than saying why. Closing that would take a result path from the
//! feed back to the view, which no strategy command has today.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use moon_core::feed::StrategyRow;
use moon_core::session::{CoreId, CoreStore};
use moon_core::venue::CoreVenue;

use super::super::StrategiesView;
use super::super::filter::PreparedFilter;
use super::super::logic::selected_keys;
use super::ops::{self, MoveStep};

/// How long an unconfirmed order keeps overriding what the core reports.
///
/// OUR bound, not a protocol promise. moonproto's `Edit*` lifecycle — including its 45-second
/// confirmation timeout — tracks strategy ROWS and explicitly not reorder-only actions, so a
/// reorder has no deadline of its own on the wire. The number is borrowed from that window for
/// consistency; what matters is only that one exists, because the alternative is an arrangement
/// that no core holds staying on screen for the rest of the session.
const CONFIRMATION_WINDOW: Duration = Duration::from_secs(45);

/// One core's strategy order that has been sent and not yet echoed back.
pub(in crate::strategies) struct PendingOrder {
    /// The sequence that was sent, which the tree cache hashes and a confirmation is compared to.
    ids: Vec<u64>,
    /// Position of each of those ids, so the tree can place a row without scanning the sequence.
    ranks: HashMap<u64, usize>,
    /// When it went to the core, for [`CONFIRMATION_WINDOW`].
    sent: Instant,
}

impl PendingOrder {
    /// Record a freshly sent sequence.
    fn new(ids: Vec<u64>) -> Self {
        let ranks = ids
            .iter()
            .enumerate()
            .map(|(rank, id)| (*id, rank))
            .collect();
        Self {
            ids,
            ranks,
            sent: Instant::now(),
        }
    }

    /// The sent sequence, for the tree cache's signature — which must separate two orders of the
    /// same ids, the case a second press before the core answers produces.
    pub(in crate::strategies) fn ids(&self) -> &[u64] {
        &self.ids
    }

    /// Position of one strategy in the sent sequence, or `None` for an id it never named.
    pub(in crate::strategies) fn rank(&self, id: u64) -> Option<usize> {
        self.ranks.get(&id).copied()
    }

    /// Whether `live` — the core's own current sequence — already agrees with what was sent.
    ///
    /// Read as: the ids the two have in COMMON appear in the live list in ascending sent position.
    /// Only the shared ones, because a strategy created, deleted or restored in the meantime is not
    /// a disagreement about order — without that, one unrelated create would pin the overlay open
    /// for the whole confirmation window.
    ///
    /// Args:
    ///     live: The core's current strategy ids, in the order it reports them.
    ///
    /// Returns:
    ///     Whether the two sequences order their shared ids the same way.
    fn confirmed_by(&self, live: impl Iterator<Item = u64>) -> bool {
        let mut previous: Option<usize> = None;
        for id in live {
            let Some(rank) = self.rank(id) else { continue };
            if previous.is_some_and(|prior| rank < prior) {
                return false;
            }
            previous = Some(rank);
        }
        true
    }
}

/// What one core contributes to a move: its rows as the tree draws them, and the ids selected in it.
struct MovableCore<'a> {
    /// Core these rows belong to.
    core: CoreId,
    /// Its strategies in display order — the core's own, or an unconfirmed order overlaying it.
    rows: Vec<&'a StrategyRow>,
    /// Strategy ids of the current selection that live in this core.
    selected: HashSet<u64>,
}

/// Everything one press acts on, resolved once for both directions.
struct MoveScope<'a> {
    /// Row predicate deciding which strategies are drawn, shared by every core below.
    filter: PreparedFilter,
    /// The cores the tree actually shows, in the order the selection first named them.
    cores: Vec<MovableCore<'a>>,
}

impl StrategiesView {
    /// The sequence one core's rows are currently DRAWN in: its own, unless an unconfirmed reorder
    /// is overlaying it.
    ///
    /// This is what a reorder must be computed from, or a second press repeats the first.
    ///
    /// Args:
    ///     store: Live per-core strategy snapshots.
    ///     core: Core to read.
    ///
    /// Returns:
    ///     Borrowed rows in display order; empty when the core is not in the store.
    fn displayed_rows<'a>(&self, store: &'a CoreStore, core: CoreId) -> Vec<&'a StrategyRow> {
        let Some(data) = store.core(core) else {
            return Vec::new();
        };
        let mut rows: Vec<&StrategyRow> = data.strategies.iter().collect();
        if let Some(pending) = self.pending_order.get(&core) {
            // The same rule the feed and the tree apply, so all three agree on where a strategy the
            // sent sequence never named belongs.
            moon_core::feed::strategy_order::resequence(&mut rows, |row| pending.rank(row.id));
        }
        rows
    }

    /// Group the selection by core, keeping only the cores the tree actually shows.
    ///
    /// A core is skipped when the exchange filter has taken it off the screen or when its row is
    /// collapsed: in both cases the operator cannot see the rows, so a press that rearranged them
    /// would be a change nobody watched being made.
    ///
    /// One pass, and shared by both directions, because the two move buttons ask this same question
    /// on every pane-cache miss — which is every strategy revision on any core.
    ///
    /// Args:
    ///     store: Live per-core strategy snapshots.
    ///     venues: Session venue identities, for the exchange filter.
    ///
    /// Returns:
    ///     The prepared row filter, and per core its drawn row order with the ids selected in it.
    fn movable_selection<'a>(
        &self,
        store: &'a CoreStore,
        venues: &HashMap<CoreId, CoreVenue>,
    ) -> MoveScope<'a> {
        let filter = self.filter.prepare();
        // Search forces every core and folder open, exactly as the tree build reads it.
        let searching = filter.searching();
        let mut cores: Vec<MovableCore<'a>> = Vec::new();
        for (core, id) in selected_keys(self) {
            if let Some(entry) = cores.iter_mut().find(|entry| entry.core == core) {
                entry.selected.insert(id);
                continue;
            }
            if !self.filter.core_matches(venues.get(&core)) {
                continue;
            }
            let open = searching
                || super::super::state::core_is_open(
                    &self.expanded_cores,
                    self.rail_expanded_core,
                    core,
                );
            if !open {
                continue;
            }
            cores.push(MovableCore {
                core,
                rows: self.displayed_rows(store, core),
                selected: HashSet::from([id]),
            });
        }
        MoveScope { filter, cores }
    }

    /// Plan one reorder step for every core the selection reaches.
    ///
    /// Args:
    ///     store: Live per-core strategy snapshots.
    ///     venues: Session venue identities, for the exchange filter.
    ///     step: Direction the operator asked for.
    ///
    /// Returns:
    ///     `(core, complete new id sequence)` for each core that can actually move.
    fn reorder_plan(
        &self,
        store: &CoreStore,
        venues: &HashMap<CoreId, CoreVenue>,
        step: MoveStep,
    ) -> Vec<(CoreId, Vec<u64>)> {
        let scope = self.movable_selection(store, venues);
        scope
            .cores
            .into_iter()
            .filter_map(|entry| {
                ops::reorder_step(
                    &entry.rows,
                    &entry.selected,
                    |row| scope.filter.matches(row),
                    step,
                )
                .map(|order| (entry.core, order))
            })
            .collect()
    }

    /// Whether each move button has anything to do, as `(up, down)`.
    ///
    /// Derived from the same rows the click acts on, so a button is enabled exactly when pressing
    /// it would change something — a selection at the top of its folder disables Up and nothing
    /// else. Both directions come out of ONE grouping pass; asking twice walked every strategy of
    /// every selected core a second time for an answer built from identical inputs.
    ///
    /// Called through the pane cache rather than per frame: see [`super::pane_cache`].
    ///
    /// Args:
    ///     store: Live per-core strategy snapshots.
    ///     venues: Session venue identities, for the exchange filter.
    ///
    /// Returns:
    ///     Whether a move up, and a move down, would rearrange at least one core.
    pub(in crate::strategies) fn move_availability(
        &self,
        store: &CoreStore,
        venues: &HashMap<CoreId, CoreVenue>,
    ) -> (bool, bool) {
        let scope = self.movable_selection(store, venues);
        let mut up = false;
        let mut down = false;
        for entry in &scope.cores {
            let visible = |row: &StrategyRow| scope.filter.matches(row);
            let ask = |step| ops::reorder_step(&entry.rows, &entry.selected, visible, step);
            up = up || ask(MoveStep::Up).is_some();
            down = down || ask(MoveStep::Down).is_some();
        }
        (up, down)
    }

    /// Move the selection one place inside its folder and send the new order to each core.
    ///
    /// Args:
    ///     step: Direction the operator asked for.
    ///     cx: View context used to reach the session and repaint.
    ///
    /// Returns:
    ///     Nothing; a selection that cannot move, and a core that refuses the command, both leave
    ///     the view untouched.
    pub(in crate::strategies) fn move_selection(
        &mut self,
        step: MoveStep,
        cx: &mut gpui::Context<Self>,
    ) {
        let plan = {
            let backend = self.backend.read(cx);
            self.reorder_plan(backend.session.store(), backend.session.core_venues(), step)
        };
        let mut sent = false;
        for (core, order) in plan {
            // Read per core rather than hoisted: `self.pending_order` is written inside this loop,
            // and the borrow checker is right that the two cannot overlap.
            let result = self
                .backend
                .read(cx)
                .session
                .reorder_strategies(core, order.clone());
            match result {
                Ok(()) => {
                    self.pending_order.insert(core, PendingOrder::new(order));
                    sent = true;
                }
                // Left OUT of `pending_order` on purpose: an overlay for a command that never
                // reached the queue would show an arrangement no core will ever confirm. A
                // multi-core selection applies core by core, so the ones that were queued stay
                // queued — there is no arrangement spanning two cores to roll back to.
                Err(error) => log::warn!("reorder strategies failed: {error}"),
            }
        }
        if sent {
            cx.notify();
        }
    }

    /// Drop overlays the core has answered, or waited long enough for.
    ///
    /// Called from two places, and it needs both: from the backend observer, where the core's echo
    /// arrives, and from render, because the deadline has to elapse even for a core that has gone
    /// quiet — which is the very failure the deadline exists for, and the one case that produces no
    /// backend notify at all.
    ///
    /// Args:
    ///     store: Live per-core strategy snapshots.
    ///
    /// Returns:
    ///     Whether anything was dropped, so the caller can repaint on the frame the tree stops
    ///     showing the overlay.
    pub(in crate::strategies) fn reconcile_pending_order(&mut self, store: &CoreStore) -> bool {
        if self.pending_order.is_empty() {
            return false;
        }
        let before = self.pending_order.len();
        let now = Instant::now();
        self.pending_order.retain(|core, pending| {
            // A core REMOVED FROM CONFIGURATION, which is the only thing that takes its data out of
            // the store — a disconnect leaves the snapshot in place, and that overlay is retired by
            // the deadline below or by the full list the core resends when it comes back.
            let Some(data) = store.core(*core) else {
                return false;
            };
            if pending.confirmed_by(data.strategies.iter().map(|row| row.id)) {
                return false;
            }
            now.duration_since(pending.sent) < CONFIRMATION_WINDOW
        });
        before != self.pending_order.len()
    }
}

#[cfg(test)]
mod tests;
