//! The shared core RUN control: is this core up, is it trading, and the two buttons that change it.
//!
//! One implementation for every surface that lists cores. The Profit Monitor is the first consumer;
//! the Core Status panel and the core-settings popup are the next, and none of them re-derives the
//! rules — the state comes from `moon_core::session::CoreRunState`, the decisions from
//! `moon_core::session::RunSummary`, the waiting from [`pending`], and the pixels from [`view`].
//!
//! Scope-shaped by construction: the same call renders one core's cell and a whole group's caption
//! cell. A group acts on every core it names, which is the one thing a per-core widget would have
//! forced each caller to reinvent.
//!
//! What the protocol allows, and therefore what this can offer (see MoonProto `docs/features.md`):
//! - `restart_now` — starts the market runtime, leaves passive mode, starts checked strategies.
//!   There is NO stop counterpart, so a running core simply shows a status dot;
//! - `strategies start/stop` — the global strategy engine, reported back as `strategies_running`.
//!   This is Moonbot's own Start/Stop, NOT "run the ticked rows" — the checkbox set is never
//!   produced here, though the protocol packet carries whatever delta MoonProto still owes.

mod actions;
mod pending;
mod view;

pub(crate) use actions::restart;
pub(crate) use pending::RunPending;
pub(crate) use view::{reserved_cell, run_cell};

use std::rc::Rc;

use gpui::{App, ElementId, Entity};
use moon_core::session::CoreId;

use crate::Backend;
use crate::design;

/// Design-reference edge of one slot in the run column.
///
/// The height a `MoonButtonSize::Micro` control takes at zero scale, so a slot is square and a
/// column of them lines up with the row heights around it.
const SLOT_W: f32 = 18.0;

/// Gap between the two slots.
const SLOT_GAP: f32 = 3.0;

/// Which slots a surface reserves on every line of its table.
///
/// Reserving is a property of the TABLE, not of the line: every line must claim the same width or
/// the column beside it stops lining up with its own heading. What a given line may FILL is a
/// property of its scope — see [`RunScope::offers_trading`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunSlots {
    /// The leading slot: a runtime status dot, or the restart button when the runtime is stopped.
    pub(crate) status: bool,
    /// The trailing slot: start/stop the global strategy engine.
    pub(crate) trading: bool,
}

impl RunSlots {
    /// Whether any slot is present.
    pub(crate) fn any(self) -> bool {
        self.status || self.trading
    }

    /// Return the column's design-reference width, gaps included.
    ///
    /// Returns:
    ///     Width in design units; zero when the column is switched off entirely.
    pub(crate) fn width(self) -> f32 {
        let slots = usize::from(self.status) + usize::from(self.trading);
        slots as f32 * SLOT_W + slots.saturating_sub(1) as f32 * SLOT_GAP
    }

    /// Return the same width already scaled for rendering.
    ///
    /// UI-scaled, like every other fixed column in the tables that host this: the width a caller
    /// takes out of its own layout budget is stated in design units, so the column must grow on
    /// exactly the scale that budget was spent on. The buttons inside are given the same
    /// `ui_value` — deliberately not the `font_w` the other micro buttons take, which would let
    /// the control outgrow the column the Font slider never widened.
    ///
    /// Args:
    ///     cx: Application context supplying the UI scale.
    ///
    /// Returns:
    ///     Rendered column width.
    pub(crate) fn width_px(self, cx: &App) -> gpui::Pixels {
        design::ui_px(cx, self.width())
    }
}

/// Return the repaint token a cached surface folds into its own gate.
///
/// Covers both halves a run cell draws: the cores' own state (connection, runtime, strategy engine)
/// and the process-wide pending register. A surface that gated on the store alone would keep
/// showing a pressed button's old face until some unrelated revision moved.
///
/// The per-core fold itself belongs to the session — one definition of "did anything about these
/// cores change" — and this only seeds it with the register the UI owns.
///
/// Args:
///     backend: Shared terminal state.
///     cores: Every core the surface draws a run cell for.
///     cx: Application context used to read the session.
///
/// Returns:
///     A value that changes whenever any of those run cells would draw differently.
pub(crate) fn run_scope_rev(
    backend: &Entity<Backend>,
    cores: impl IntoIterator<Item = CoreId>,
    cx: &App,
) -> u64 {
    let backend = backend.read(cx);
    backend
        .session
        .run_scope_rev(cores, backend.run_pending.rev())
}

/// One rendered run cell's scope.
#[derive(Clone)]
pub(crate) struct RunScope {
    /// Stable identity key of the line this cell belongs to.
    ///
    /// A core uid for a row, a caption's own position for a group heading — NEVER the line's index
    /// in the table. GPUI keys interactive state on element identity, so an index-derived id
    /// migrates hover and press state onto a different core the moment a row appears above.
    pub(crate) key: RunKey,
    /// Cores this cell stands for; one for a core row, many for a group caption or exchange row.
    ///
    /// Its LENGTH is also what decides how much the cell offers: exactly one core can be
    /// restarted, while a caption standing for six shows their folded status and acts on trading
    /// only. That is a property of the scope, not a flag a caller could set inconsistently.
    pub(crate) cores: Rc<[CoreId]>,
    /// Slots every line of the hosting table reserves, so its columns line up.
    pub(crate) reserve: RunSlots,
    /// Whether THIS line fills the reserved trading slot.
    ///
    /// The one bit that varies per line: rows and captions carry separate preferences, and a
    /// reserved slot a line does not fill is drawn empty at its reserved width rather than skipped.
    pub(crate) offers_trading: bool,
}

/// Which identity space a run cell's key belongs to.
///
/// Two spaces share one frame — cores and captions — and their numbers overlap: core uid 0 and
/// section 0 both exist. The variant is what keeps their element ids apart without allocating.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunKey {
    /// A row standing for one core, keyed by its uid.
    Core(CoreId),
    /// A REPEATED drawing of a core already drawn above — a core saved into several groups — keyed
    /// by its line position.
    ///
    /// The same rule the hosting row's own identity follows: two live lines under one identity
    /// would trade hover and press state every frame, and only the repeats pay the moving key.
    Repeat(usize),
    /// A group caption, keyed by its stable position among the drawn sections.
    Section(usize),
}

/// One addressable slot of a run cell.
///
/// An enum rather than a string: the set is closed, and a stringly-typed lookup needs a fallback
/// arm that silently answers for a name nobody defined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunSlot {
    /// The leading status/restart slot.
    Status,
    /// The trailing trading slot.
    Trading,
}

impl RunScope {
    /// Return one slot's element identity.
    ///
    /// A `(&'static str, u64)` pair, which costs no allocation in a virtual-list item builder.
    ///
    /// Args:
    ///     slot: Which slot of the cell.
    ///
    /// Returns:
    ///     The slot's identity.
    pub(crate) fn slot_element_id(&self, slot: RunSlot) -> ElementId {
        match (self.key, slot) {
            (RunKey::Core(core), RunSlot::Status) => ("core-run-status", core).into(),
            (RunKey::Core(core), RunSlot::Trading) => ("core-run-trading", core).into(),
            (RunKey::Repeat(line), RunSlot::Status) => {
                ("core-run-repeat-status", line as u64).into()
            }
            (RunKey::Repeat(line), RunSlot::Trading) => {
                ("core-run-repeat-trading", line as u64).into()
            }
            (RunKey::Section(section), RunSlot::Status) => {
                ("core-run-section-status", section as u64).into()
            }
            (RunKey::Section(section), RunSlot::Trading) => {
                ("core-run-section-trading", section as u64).into()
            }
        }
    }
}
