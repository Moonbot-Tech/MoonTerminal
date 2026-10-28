//! Right-click context menu for a Core Status update scope: "update to release" -- the same action
//! the row button fires -- and, one level deeper, a free-text prompt for a named beta/test build.
//! Both dispatch through the shared `controls::core_update` (`update_scope`, `open_named_dialog`),
//! so the menu can never do anything the row button could not already do.
//!
//! A SCOPE, not a row: the same menu serves one core, a multi-row selection, every core of a
//! server, and every core under a Flat exchange heading. Which cores a click stands for is decided
//! before the menu opens -- `controls::core_update::resolve_menu_scope` for a row, the group or
//! section membership for a heading -- so this module only ever draws what it was handed.
//!
//! The named-build prompt itself lives in `controls::core_update`, so its transient state does
//! not leak into `CoreStatusView`. Right-click wiring leaves the current selection unchanged:
//! opening a menu must not move the selection the menu is about to act on. Resolving a
//! `MoonDataTable` line index back to a row, and the row-level `on_right_click_row` hook, follow
//! `crates/moon-ui-gpui/src/panels/report/render.rs:89-111` -- a line index addresses a LINE, not a
//! core, because the grouped view draws each heading as its own synthetic row.

use std::rc::Rc;

use gpui::*;
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::feed::UpdateTarget;
use moon_core::session::CoreId;

use super::model::CoreStatusRow;
use crate::Backend;
use crate::controls::core_update::{
    OfferCounts, OfferState, offer_state, open_named_dialog, replaces_test_note, skip_reasons,
    update_scope,
};
use crate::display_text::plural_key;

/// Fitted-menu bounds for this two-item (plus separator) menu -- narrower than the shared coin
/// menu's range, since every label here is short and fixed.
const MENU_MIN_WIDTH: f32 = 200.0;
const MENU_MAX_WIDTH: f32 = 380.0;

/// What one right-click, server row or heading stands for: the cores to command, their verbatim
/// names, and how the update queue currently classifies them.
pub(super) struct UpdateScope {
    /// Cores this menu will actually enqueue: the OFFERABLE subset, in the order drawn.
    ///
    /// The subset and not the whole click, because the menu NAMES this count and lists these
    /// names -- what it says and what it sends have to be one set. Holding every clicked core
    /// here instead let a core counted as skipped reconnect between the menu opening and the
    /// entry being pressed, and `enqueue_core_updates` would then accept it: more cores
    /// updated than the label promised.
    pub(super) targets: Rc<[CoreId]>,
    /// Display names parallel to [`Self::targets`], exactly as the operator named them.
    pub(super) names: Rc<[String]>,
    /// The reported build letter of each target that runs a lettered build -- what a release
    /// press would replace, and what the leading row warns about.
    letters: Vec<String>,
    /// Everything the click stood for, classified -- including what is NOT a target, which is
    /// what the skip note explains.
    pub(super) counts: OfferCounts,
    /// How many rows the click stood for at all, offerable or not.
    total: usize,
}

impl UpdateScope {
    /// Build a scope from the rows it stands for.
    ///
    /// Classifies with [`offer_state`] -- the same rule the row's own update button draws from --
    /// rather than a second, weaker copy that checked only `status` and `update`. MoonProto's
    /// lifecycle events do not arrive in a fixed order, so a core can be `Ready` before it has
    /// reported a `server_version` or an endpoint; the weaker copy enabled the menu in exactly that
    /// window, `enqueue_core_update` silently rejected the click, and the operator got a
    /// `log::warn!` they never saw.
    ///
    /// Args:
    ///     rows: The rows this click stands for, in render order.
    ///
    /// Returns:
    ///     The scope, with its offer tally already folded.
    pub(super) fn from_rows<'a>(rows: impl Iterator<Item = &'a CoreStatusRow>) -> Self {
        let mut targets = Vec::new();
        let mut names = Vec::new();
        let mut letters = Vec::new();
        let mut counts = OfferCounts::default();
        let mut total = 0usize;
        for row in rows {
            total += 1;
            let state = offer_state(
                &row.status,
                row.server_version,
                row.endpoint.is_some(),
                row.update.as_ref(),
            );
            counts.add(state);
            if state == OfferState::Offerable {
                targets.push(row.id);
                names.push(row.name.clone());
                letters.extend(row.server_version_suffix.clone());
            }
        }
        Self {
            targets: Rc::from(targets),
            names: Rc::from(names),
            letters,
            counts,
            total,
        }
    }

    /// Build a scope from already-resolved core ids, looked up in the panel's rows.
    ///
    /// The shape every call site actually reaches for: scope resolution hands back IDS (the
    /// selection, a server's group, a section's members), and the rows are what carry the name and
    /// the state a menu needs. A core with no row is dropped rather than guessed at -- an id the
    /// panel is not drawing has no business in a scope that enqueues.
    ///
    /// Args:
    ///     cores: Resolved core ids, in the order they are drawn.
    ///     rows: The panel's current rows.
    ///
    /// Returns:
    ///     The scope, in `cores` order.
    pub(super) fn from_cores(cores: &[CoreId], rows: &[CoreStatusRow]) -> Self {
        Self::from_rows(
            cores
                .iter()
                .filter_map(|core| rows.iter().find(|row| row.id == *core)),
        )
    }

    /// Whether the queue would accept anything in this scope right now.
    pub(super) fn offerable(&self) -> bool {
        !self.targets.is_empty()
    }

    /// Whether this scope stands for no core at all -- nothing for a menu to open about.
    pub(super) fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Open the update menu for `scope` at `pos`.
///
/// Both entries are always present and DISABLED (never hidden) when nothing in the scope is
/// offerable, plus a leading disabled row naming the reason -- the row must not silently offer
/// nothing, and the menu must not change shape between the enqueueable and non-enqueueable case.
/// A scope that is only PARTLY offerable says so on the same leading row, reusing the wording the
/// server button's tooltip already uses for the same two skips.
///
/// Args:
///     backend: Shared terminal state, forwarded to the shared enqueue entry points unchanged.
///     scope: The cores this click stands for.
///     pos: Window-coordinate open point -- the click position, or `window.mouse_position()` from
///         a `MoonDataTable` row handler that receives only a line index.
///     window: Host window used to open the fitted menu.
///     cx: Application context used to open the fitted menu.
///
/// Returns:
///     Nothing; opens the menu as a side effect.
pub(super) fn open_update_row_menu(
    backend: &Entity<Backend>,
    scope: UpdateScope,
    pos: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let offerable = scope.offerable();
    let many = scope.targets.len() > 1;
    let n = scope.targets.len();
    let scope = Rc::new(scope);

    let mut items: Vec<MoonMenuItem> = Vec::new();
    if let Some(note) = scope_note(&scope) {
        items.push(MoonMenuItem::with_key("core-update-scope-note", note).disabled(true));
    }

    let release_label = if many {
        t!(plural_key("core_update.menu.release_n", n as u64), n = n).to_string()
    } else {
        t!("core_update.menu.release").to_string()
    };
    let mut release_item =
        MoonMenuItem::with_key("core-update-release", release_label).disabled(!offerable);
    if offerable {
        let backend_r = backend.clone();
        let cores = scope.targets.clone();
        release_item = release_item.on_click(move |_, window, app| {
            window.close_context_menu(app);
            update_scope(&backend_r, &cores, UpdateTarget::Release, window, app);
        });
    }
    items.push(release_item);

    items.push(MoonMenuItem::separator());

    let named_label = if many {
        t!(plural_key("core_update.menu.named_n", n as u64), n = n).to_string()
    } else {
        t!("core_update.menu.named").to_string()
    };
    let mut named_item =
        MoonMenuItem::with_key("core-update-named", named_label).disabled(!offerable);
    if offerable {
        let backend_n = backend.clone();
        let scope_n = scope.clone();
        named_item = named_item.on_click(move |_, window, app| {
            window.close_context_menu(app);
            open_named_dialog(
                backend_n.clone(),
                scope_n.targets.clone(),
                scope_n.names.clone(),
                None,
                window,
                app,
            );
        });
    }
    items.push(named_item);

    window.open_fitted_moon_context_menu(
        cx,
        "core-update-row-menu",
        pos,
        items,
        MENU_MIN_WIDTH,
        MENU_MAX_WIDTH,
    );
}

/// The leading disabled row, when this scope has something to say before it is acted on.
///
/// Returns:
///     Why nothing is offerable; otherwise what a press would SKIP and which test builds the
///     release entry would replace, joined with the separator `controls::core_update::view`'s
///     server tooltip uses, so the menu and that tooltip can never word one situation two ways.
///     `None` when every core in the scope would be accepted and none runs a lettered build.
fn scope_note(scope: &UpdateScope) -> Option<String> {
    if !scope.offerable() {
        return Some(t!("core_update.menu.unavailable").to_string());
    }
    let mut parts = skip_reasons(scope.counts.offline, scope.counts.tracked);
    parts.extend(replaces_test_note(scope.letters.iter().map(String::as_str)));
    (!parts.is_empty()).then(|| parts.join(" \u{2014} "))
}
