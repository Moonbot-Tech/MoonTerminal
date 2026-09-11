//! Which controls show a MIXED value — a parameter the selected cores disagree on — and how a
//! control finds out that it is one of them.
//!
//! The pages draw a hundred-odd controls and hand each one only its id and the function that
//! stages its value into the page (`fn(&mut CoreConfig, bool)` and the like). Nothing in that
//! shape names the field the control writes, and giving every call site a field key would touch
//! every row of every page. So the control asks here, with the setter it already has, and the
//! answer is found by PROBING the setter against two configurations that disagree on every field
//! (`moon_core::feed::ProbeBases`): the fields that moved are the ones it writes. One probe per
//! control id for the life of the window — the setter of a given id never changes — so a frame
//! costs a hash lookup per control.
//!
//! The scope is installed for the duration of one render by the view (see `render.rs`), because
//! the widgets are free functions drawn inside that view's own render, where reading the view
//! back would panic. A control drawn with no scope installed — the compact popup shares none of
//! these helpers, but the guard is cheap — reads as not mixed.
//!
//! "Mixed" means: the selection has two or more live pages and they disagree on a field this
//! control writes that still needs the trader's attention — the view hands the scope that list
//! (`CoreExpertView::attention`), which already leaves out the staged fields, since a staged field
//! shows the value the OK will write, and the copies of another field on the drawn page.

use std::cell::RefCell;
use std::collections::HashMap;

use moon_core::feed::{CoreConfig, ProbeBases};

/// The probe bases and everything learned from them, kept on the view between renders.
#[derive(Default)]
pub(super) struct MixedScope {
    /// What setters are probed against; `None` until a page has been seeded, and any page will
    /// do — a setter writes the same field whatever the values around it.
    bases: Option<ProbeBases>,
    /// Fields each probed control writes, by control id. Empty for a dead control.
    probes: HashMap<&'static str, Vec<usize>>,
    /// Fields that still need attention, ascending; see the module doc.
    differing: Vec<usize>,
    /// The palette's warning colour, for the widgets that draw a mark rather than set a tone.
    accent: u32,
}

impl MixedScope {
    /// Remember a page to probe against, the first time one is available.
    pub(super) fn seed(&mut self, page: &CoreConfig) {
        if self.bases.is_none() {
            self.bases = Some(ProbeBases::new(page));
        }
    }

    /// Set what this render's answers are computed from. The buffer is reused across frames.
    pub(super) fn set_context(&mut self, differing: impl Iterator<Item = usize>, accent: u32) {
        self.differing.clear();
        self.differing.extend(differing);
        self.accent = accent;
    }

    /// The fields that still need attention, ascending, as last set.
    pub(super) fn differing(&self) -> &[usize] {
        &self.differing
    }

    /// Whether the control `id`, whose setter `probe` runs, writes a mixed field.
    ///
    /// The probe runs at most once per id; while nothing differs it does not run at all, so a
    /// window over a single core never pays for it.
    pub(super) fn is_mixed(&mut self, id: &'static str, probe: &dyn Fn(&mut CoreConfig)) -> bool {
        if self.differing.is_empty() {
            return false;
        }
        let Some(bases) = self.bases.as_ref() else {
            return false;
        };
        let fields = self
            .probes
            .entry(id)
            .or_insert_with(|| bases.fields_written_by(probe));
        hits(fields, &self.differing)
    }

    /// The fields ONE change writes, probed now rather than read from the cache — what a
    /// decision on a mixed control stages.
    ///
    /// Probed per decision, not per control: the cache says what the control CAN write, this says
    /// what this change DID — a number box whose parser rejected the text writes nothing, and a
    /// decision on nothing must stage nothing. Two clones and two diffs, on a click into a mixed
    /// control only.
    pub(super) fn fields_written_by(&self, change: &dyn Fn(&mut CoreConfig)) -> Vec<usize> {
        self.bases
            .as_ref()
            .map_or_else(Vec::new, |bases| bases.fields_written_by(change))
    }

    /// Whether a control probed earlier — by [`Self::is_mixed`], or by the view for the controls
    /// whose setters live in the page specs — writes a mixed field.
    pub(super) fn cached(&self, id: &str) -> bool {
        self.probes
            .get(id)
            .is_some_and(|fields| hits(fields, &self.differing))
    }

    /// A scope with known probe results, for tests of the answer alone.
    #[cfg(test)]
    pub(super) fn with_probes(probes: &[(&'static str, &[usize])]) -> Self {
        Self {
            probes: probes
                .iter()
                .map(|(id, fields)| (*id, fields.to_vec()))
                .collect(),
            ..Self::default()
        }
    }
}

/// Whether any of a control's fields is among the ones needing attention.
fn hits(fields: &[usize], differing: &[usize]) -> bool {
    fields
        .iter()
        .any(|field| differing.binary_search(field).is_ok())
}

thread_local! {
    /// The scope of the render in progress, if a render of the expert window is in progress.
    static ACTIVE: RefCell<Option<MixedScope>> = const { RefCell::new(None) };
}

/// Install the scope for the render about to happen. Paired with [`take`].
pub(super) fn install(scope: MixedScope) {
    ACTIVE.with(|active| *active.borrow_mut() = Some(scope));
}

/// Take the scope back after the render, with whatever the render's probes taught it.
pub(super) fn take() -> MixedScope {
    ACTIVE.with(|active| active.borrow_mut().take().unwrap_or_default())
}

/// Whether the control `id` writes a mixed field, probing its setter once. See the module doc.
pub(super) fn is_mixed(id: &'static str, probe: impl Fn(&mut CoreConfig)) -> bool {
    ACTIVE.with(|active| {
        active
            .borrow_mut()
            .as_mut()
            .is_some_and(|scope| scope.is_mixed(id, &probe))
    })
}

/// Whether a control probed earlier writes a mixed field — for the widgets that carry no setter
/// (sliders and text boxes, whose setters the view probes from the page specs).
pub(super) fn cached(id: &str) -> bool {
    ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .is_some_and(|scope| scope.cached(id))
    })
}

/// The colour a widget marks a mixed control with, when its component has no tone of its own.
pub(super) fn accent() -> Option<u32> {
    ACTIVE.with(|active| active.borrow().as_ref().map(|scope| scope.accent))
}

#[cfg(test)]
mod tests;
