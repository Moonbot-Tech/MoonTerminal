//! Frame-to-frame reuse of the strategy-tree adapter.
//!
//! `tree::moon::build` walks every core's strategies, counts each folder and allocates one
//! `NodeData` per visible node. That is the right cost once per data change — and the wrong cost
//! per FRAME, which is what the window actually paid: GPUI re-renders an uncached root view on
//! every repaint, and a mouse-over moving between rows is a repaint. Measured on a real account
//! (230 visible nodes): 1.0-1.3 ms per frame, and moving the mouse takes the window from 4 to
//! ~85 frames per second — 130 ms of rebuild work per second for a hover highlight.
//!
//! So the build is keyed on a signature of everything it reads. Same signature, same tree: the
//! previous result is handed out again and the walk is skipped. Data changes 4 times a second at
//! most (the backend notify throttle), so a mouse sweep hits the cache on ~95% of its frames.
//!
//! The signature is the whole correctness argument. Anything `build` reads and the signature omits
//! becomes a tree that renders stale — a strategy that stays checked after the core said otherwise,
//! a folder count frozen mid-session. [`super::tests`] holds the contract that keeps the two in
//! step; the guarantee it rests on is that every field is listed HERE, in one function, rather than
//! sampled across the call site.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::SharedString;
use moon_core::session::{CoreId, CoreStore};
use moon_core::venue::CoreVenue;

use super::super::StrategiesView;
use super::moon::NodeData;
use crate::controls::venue_section_label;

/// The one thing a cache hit has to hand back: the row data the renderer and decorators read.
///
/// Deliberately NOT the `MoonTreeItem` forest, the expanded-id list, the visible flat order or the
/// searching flag. The first two exist to be pushed into `MoonTreeState`, and a hit means the shape
/// did not change, so there is nothing to push. The flat order is written by the same frame that
/// builds — nothing else writes `flat_order` — so on a hit it already holds the matching value.
/// Retaining any of them would cost memory to store what would never be read.
pub(crate) struct TreeCache {
    /// Signature of the inputs this result was built from.
    sig: TreeSig,
    /// Per-node row data, shared with the row renderer and the drag decorators.
    node_data: Rc<std::collections::HashMap<SharedString, NodeData>>,
}

/// The signature, split by where its inputs come from.
///
/// Two halves rather than one number because a miss has two very different meanings: the cores sent
/// something (`store`) or the user did something (`view`). Only the first can churn without anyone
/// touching the window, and only the split can say which is happening — the counters
/// `strat_sig_store` and `strat_sig_view` report exactly this.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TreeSig {
    /// Cores, their strategy snapshots and the open-order counts the captions show.
    pub(crate) store: u64,
    /// Filter, expansion, selection, staging and the rest of the window's own state.
    pub(crate) view: u64,
}

impl TreeCache {
    /// Stores a fresh build result under the signature of the inputs that produced it.
    pub(crate) fn store(
        sig: TreeSig,
        node_data: Rc<std::collections::HashMap<SharedString, NodeData>>,
    ) -> Self {
        Self { sig, node_data }
    }

    /// Returns the retained row data when `sig` matches what it was built from.
    pub(crate) fn get(
        &self,
        sig: TreeSig,
    ) -> Option<Rc<std::collections::HashMap<SharedString, NodeData>>> {
        (self.sig == sig).then(|| self.node_data.clone())
    }

    /// Reports which half of the signature moved, for the miss-attribution counters.
    pub(crate) fn moved(&self, sig: TreeSig) -> (bool, bool) {
        (self.sig.store != sig.store, self.sig.view != sig.view)
    }
}

/// Hashes every input `tree::moon::build` reads, so an unchanged signature means an unchanged tree.
///
/// The two halves are the store and the window's own state:
///
///   * per core, in the order the window lists them: its id, its display name, venue presence and
///     identity/caption fields, `strategies_rev` (the strategy snapshot), and the rendered
///     open-order digest. A core appearing, disappearing or being renamed moves the list itself.
///   * per window field: the filter, the three expansion sets, the UI-only folders, the selection,
///     the staged checkboxes, the selected folder, and the deleted-strategy revision.
///
/// `deleted` is represented by `deleted_rev` rather than hashed: it is a background load, so it
/// arrives on a frame nothing else moved on, and only an explicit revision can see that.
///
/// Sets and maps are folded COMMUTATIVELY — a `HashSet` has no stable iteration order, so hashing
/// it as a sequence would produce a different signature for identical contents and defeat the
/// cache in a way that looks like correct-but-slow.
///
/// Args:
///     view: The window whose filter, expansion and selection state feeds the tree.
///     store: Per-core data behind the strategy rows.
///     cores: Connected cores in the canonical order the tree lists them.
///     venues: Canonical session venue identities and captions used to group those cores.
///
/// Returns:
///     A signature that changes whenever the rendered tree would.
pub(crate) fn data_sig(
    view: &StrategiesView,
    store: &CoreStore,
    cores: &crate::core_order::OrderedCores,
    venues: &HashMap<CoreId, CoreVenue>,
) -> TreeSig {
    let mut h = DefaultHasher::new();

    venues_digest(cores.iter().map(|(core, _)| (*core, venues.get(core)))).hash(&mut h);
    for (core_id, core_name) in cores.iter() {
        core_id.hash(&mut h);
        core_name.hash(&mut h);
        let Some(cd) = store.core(*core_id) else {
            0u64.hash(&mut h);
            continue;
        };
        cd.strategies_rev.hash(&mut h);
        open_orders_digest(cd).hash(&mut h);
    }
    let store_sig = h.finish();

    let mut h = DefaultHasher::new();
    view.filter.search.hash(&mut h);
    view.filter.kind.hash(&mut h);
    view.filter.dir.hash(&mut h);

    unordered(view.expanded_cores.iter()).hash(&mut h);
    unordered(view.expanded_folders.iter()).hash(&mut h);
    unordered(view.expanded_deleted.iter()).hash(&mut h);
    unordered(view.ui_folders.iter()).hash(&mut h);
    unordered(view.sel.iter()).hash(&mut h);
    unordered(view.staged.iter()).hash(&mut h);
    view.selected.hash(&mut h);
    view.selected_folder.hash(&mut h);
    view.deleted_rev.hash(&mut h);

    // The Deleted folder's caption comes from the dictionary, and a language switch reaches windows
    // through `cx.refresh_windows()` — a repaint, which moves nothing else this signature reads. So
    // the active locale is an input like any other, or that one row keeps the retired language.
    rust_i18n::locale().hash(&mut h);

    TreeSig {
        store: store_sig,
        view: h.finish(),
    }
}

/// Digest venue inputs in the same canonical core order used by the tree builder.
///
/// Presence is explicit so a core whose venue has not arrived cannot collide structurally with an
/// identified venue. Identity controls grouping. The displayed section caption is hashed through
/// [`venue_section_label`] so a heading change invalidates the tree without this module reading
/// the raw reported name.
///
/// Args:
///     rows: Ordered `(core id, optional venue)` pairs from the visible core list.
///
/// Returns:
///     Stable digest that moves whenever exchange grouping or its displayed caption can change.
fn venues_digest<'a>(rows: impl IntoIterator<Item = (CoreId, Option<&'a CoreVenue>)>) -> u64 {
    let mut h = DefaultHasher::new();
    for (core, venue) in rows {
        core.hash(&mut h);
        match venue {
            None => 0u8.hash(&mut h),
            Some(venue) => {
                1u8.hash(&mut h);
                venue.id.hash(&mut h);
                venue.dex.hash(&mut h);
                venue_section_label(Some(venue)).hash(&mut h);
            }
        }
    }
    h.finish()
}

/// Digests exactly what the tree shows of a core's orders: how many are still open, and which
/// strategies they belong to.
///
/// Deliberately NOT `orders_table_rev`. That counter advances for every combined order batch — a
/// price moving on an untouched order advances it — while the tree only ever renders two counts,
/// `(open)` beside the core and `kind(open)` beside the strategy. Keying the cache on the revision
/// made a busy account miss on roughly half its frames and rebuild an identical tree; keying it on
/// the counts themselves means an order changing price is free and an order opening or closing is
/// not.
///
/// One pass, no allocation, and a cheap multiply per row instead of a full hasher: this runs on
/// every frame, so it has to cost far less than the build it is there to skip.
fn open_orders_digest(cd: &moon_core::session::store::CoreData) -> u64 {
    /// Odd 64-bit constant (the golden-ratio mix used by rustc's FxHash) that spreads small
    /// sequential strategy ids across the whole word.
    const MIX: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut count = 0u64;
    let mut acc = 0u64;
    for o in cd.orders.iter().filter(|o| !o.job_is_done) {
        count += 1;
        // Commutative on purpose: the row ORDER inside the batch is not something the tree renders.
        acc = acc.wrapping_add(o.strat_id.wrapping_mul(MIX));
    }
    acc.wrapping_mul(31).wrapping_add(count)
}

#[cfg(test)]
mod tests;

/// Folds a collection whose iteration order is not stable into an order-independent digest.
///
/// The length is mixed in as well, so a set that loses one element and gains another whose hashes
/// happen to sum the same still moves — the sum alone would not.
fn unordered<T: Hash>(items: impl Iterator<Item = T>) -> u64 {
    let mut sum = 0u64;
    let mut count = 0u64;
    for item in items {
        let mut h = DefaultHasher::new();
        item.hash(&mut h);
        sum = sum.wrapping_add(h.finish());
        count += 1;
    }
    sum.wrapping_mul(31).wrapping_add(count)
}
