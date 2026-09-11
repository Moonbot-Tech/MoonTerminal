//! The set of parameters a multi-core OK will write, kept apart from the page that shows them.
//!
//! A settings surface edits ONE page — the core it draws — but may send to EVERY core the user
//! selected, and may switch which core it draws while the edits stand. So "what changed" cannot be
//! the page: a page belongs to one core, and a change re-derived by comparing the page against
//! whichever core is now drawn would lose a parameter the moment a core is picked that already
//! holds the new value. The set is therefore held explicitly — which fields, and the values they
//! were staged to — and the page is only ever the current core's snapshot with this set laid over.
//!
//! Membership follows the edits, and LATCHES: a field enters the set when a control moves it away
//! from the base it was seeded from, and stays there until the set is cleared. It is not dropped
//! when a later edit brings it back to the base, because the base is one core's and the set is
//! written to many: a value moved on core A and then moved on core B's page to what B already holds
//! is still a change for A, and dropping it would lose the edit the user made first. The cost is
//! that a value typed and typed back on one core still counts as a parameter and is still written
//! — as the value on the page, which is what the count promises. A control that moves nothing (a
//! dead row's slider, a stepper pressed at its floor) never enters the set at all.
//!
//! The set keeps a SHADOW of the page — the page as it stood after the last edit — so an edit is
//! measured against it without the surface cloning the page before every keystroke or slider tick.
//! The shadow is also where the staged values live once the surface drops or replaces its page,
//! and for a staged field it is authoritative over the page: a page whose mirror is on shows a
//! staged short gesture as its long twin's copy, and the shadow keeps the staged value through
//! that. A surface lays the set back over its page after every edit ([`Self::overlay`]), so the
//! moment such a copy stops being one — the mirror is turned off — the page shows the staged value
//! again, and what it shows is what OK sends.
//!
//! Lives beside [`CORE_FIELDS`] rather than in the UI crate because it is pure projection logic,
//! and because only this crate can build the projection a test needs.

use super::CoreConfig;
use super::fields::{CORE_FIELDS, changed_fields, mask_for_fields};
use crate::feed::FieldMask;

/// Fields a control has changed, with the values they were changed to.
#[derive(Default)]
pub struct CoreChangeSet {
    /// Indices into [`CORE_FIELDS`], ascending, no duplicates.
    fields: Vec<usize>,
    /// The page as it stood after the last edit (or as it was seeded, before any): the one copy
    /// of the staged values once the surface drops or replaces its page, and the "before" every
    /// edit is measured against. Meaningful only through [`Self::fields`] once the page is gone;
    /// every other field of it is whichever core's snapshot the edit happened to land on.
    ///
    /// Boxed because it is two orders of magnitude larger than the index list beside it.
    shadow: Option<Box<CoreConfig>>,
}

impl CoreChangeSet {
    /// Whether any field is staged.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// How many parameters an OK would write.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the table field at `index` is staged.
    pub fn contains(&self, index: usize) -> bool {
        self.fields.binary_search(&index).is_ok()
    }

    /// The staged fields, ascending.
    pub fn fields(&self) -> &[usize] {
        &self.fields
    }

    /// Drop every staged change and the shadow with it.
    pub fn clear(&mut self) {
        self.fields.clear();
        self.shadow = None;
    }

    /// Forget the staged fields and take `page` as the shadow — what a surface does once the set
    /// has been sent and the page is the new base. Keeps the shadow's allocation, unlike
    /// [`Self::clear`] followed by [`Self::seed`].
    pub fn restart(&mut self, page: &CoreConfig) {
        self.fields.clear();
        match self.shadow.as_deref_mut() {
            Some(shadow) => shadow.clone_from(page),
            None => self.shadow = Some(Box::new(page.clone())),
        }
    }

    /// Take a freshly seeded page as the shadow — one clone per seed, so that no edit has to.
    ///
    /// Call it with the page AFTER [`Self::overlay`] has been laid over it: the shadow must equal
    /// what the surface draws, or the next edit would be measured against the wrong "before". The
    /// staged fields themselves are left as they are: the shadow is where their values live, and
    /// a page whose mirror is on shows a staged short gesture as its long twin's copy — taking
    /// that copy would lose the value staged for the core the mirror is off on.
    pub fn seed(&mut self, page: &CoreConfig) {
        match self.shadow.as_deref_mut() {
            Some(shadow) => {
                for (index, field) in CORE_FIELDS.iter().enumerate() {
                    if self.fields.binary_search(&index).is_err() {
                        field.copy_into(page, shadow);
                    }
                }
            }
            None => self.shadow = Some(Box::new(page.clone())),
        }
    }

    /// Record what one edit did to the page.
    ///
    /// A field the edit moved away from `base` — the snapshot the page was seeded from — enters
    /// the set; one already in it stays, whatever value it now holds (see the module doc). Fields
    /// the edit did not touch keep their membership — an edit on one page must not un-stage a
    /// change made on another.
    ///
    /// A field that is derived from another on this page (see [`super::CoreField::is_derived`])
    /// is a copy, not a value the user set: it is never staged from here, and the shadow keeps
    /// whatever value it holds for it rather than taking the copy — a staged short gesture from a
    /// core whose mirror is off must survive the page of one whose mirror is on. One that just
    /// STOPPED being derived is measured against the base like a touched field, because the page
    /// has been showing it as a value of its own all along.
    ///
    /// Args:
    ///     after: The page as it stands now.
    ///     base: The core's snapshot the page was seeded from.
    pub fn note_edit(&mut self, after: &CoreConfig, base: &CoreConfig) {
        let Some(shadow) = self.shadow.as_deref_mut() else {
            // No page was ever seeded, so there is nothing to measure against.
            return;
        };
        let touched = changed_fields(shadow, after);
        // Membership BEFORE this edit stages anything: a field staged by this very edit because
        // it stopped being a copy holds the user's value on the page, and must not be mistaken
        // for one staged earlier whose value the shadow keeps (the `was_copy` rule below).
        let staged_before = self.fields.clone();
        let mut candidates = touched.clone();
        for (index, field) in CORE_FIELDS.iter().enumerate() {
            if field.is_derived(shadow) && !field.is_derived(after) {
                candidates.push(index);
            }
        }
        for index in candidates {
            let field = &CORE_FIELDS[index];
            if field.is_derived(after) {
                continue;
            }
            if self.fields.binary_search(&index).is_err() && field.differs(after, base) {
                insert_sorted(&mut self.fields, index);
            }
        }
        // Only what moved is copied — the shadow already agrees with the page everywhere else.
        // Two kinds of move are not taken: a derived copy, for the reason the doc gives, and a
        // staged field that only moved because it STOPPED being a copy — the page holds the twin's
        // value, not the user's, and the shadow's is the one the surface lays back over the page.
        //
        // Decided over the shadow as it stood BEFORE any copy: the mirror flag precedes the shorts
        // in table order, and copying it first would make every short read as never having been a
        // copy. And over the set as it stood before this edit, for the reason `staged_before`
        // gives.
        let to_copy: Vec<usize> = touched
            .into_iter()
            .filter(|&index| {
                let field = &CORE_FIELDS[index];
                let was_copy =
                    staged_before.binary_search(&index).is_ok() && field.is_derived(shadow);
                !field.is_derived(after) && !was_copy
            })
            .collect();
        for index in to_copy {
            CORE_FIELDS[index].copy_into(after, shadow);
        }
    }

    /// Stage one field at the value the page holds, whether or not that value moved.
    ///
    /// For a control drawn MIXED — the selection disagrees on its field — a click that lands on
    /// the value the drawn core already holds is still a decision: "this value, everywhere". An
    /// edit measured by movement alone would miss it, and the control would stay mixed however
    /// often it was clicked. A field that is derived on this page is not a decision and is left
    /// alone.
    ///
    /// Args:
    ///     index: Index into [`CORE_FIELDS`].
    ///     page: The page as it stands, whose value for the field is the one staged.
    pub fn stage(&mut self, index: usize, page: &CoreConfig) {
        let field = &CORE_FIELDS[index];
        if field.is_derived(page) {
            return;
        }
        let Some(shadow) = self.shadow.as_deref_mut() else {
            return;
        };
        insert_sorted(&mut self.fields, index);
        field.copy_into(page, shadow);
    }

    /// Lay the staged values over a snapshot, leaving every unstaged field of it alone.
    ///
    /// Both halves of a multi-core surface's job in one function: the page for a newly picked core
    /// is its snapshot with this laid over, and what OK sends to each selected core is that core's
    /// LATEST snapshot with this laid over — so a bulk OK writes the parameters the user changed
    /// and nothing the cores held differently from each other.
    pub fn overlay(&self, into: &mut CoreConfig) {
        let Some(values) = self.shadow.as_deref() else {
            return;
        };
        for &index in &self.fields {
            CORE_FIELDS[index].copy_into(values, into);
        }
        // A field-by-field copy cannot keep the projection's one cross-field rule: with
        // `same_hotkeys_for_move` on, every short gesture mirrors its long one. The target's own
        // rule is restored from ITS longs — whether only a long was staged, or the flag itself
        // arrived — and nothing moves while the flag is off.
        into.gestures.normalize();
    }

    /// The mask a write of the staged fields must carry.
    pub fn mask(&self) -> FieldMask {
        mask_for_fields(&self.fields)
    }
}

/// Put a field into a sorted, unique index list.
///
/// A free function over the list rather than a method: callers hold the shadow borrowed at the
/// same time, and the two are disjoint fields only when reached directly.
fn insert_sorted(fields: &mut Vec<usize>, index: usize) {
    if let Err(slot) = fields.binary_search(&index) {
        fields.insert(slot, index);
    }
}

#[cfg(test)]
mod tests;
