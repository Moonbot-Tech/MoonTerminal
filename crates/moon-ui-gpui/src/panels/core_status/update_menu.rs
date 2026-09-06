//! Right-click context menu for a Core Status update scope: "update to release" -- the same action
//! the row button fires -- and, one level deeper, a free-text prompt for a named beta/test build.
//! Both dispatch through the shared `controls::core_update::{update_core, update_scope}`, so the
//! menu can never do anything the row button could not already do.
//!
//! A SCOPE, not a row: the same menu serves one core, a multi-row selection, every core of a
//! server, and every core under a Flat exchange heading. Which cores a click stands for is decided
//! before the menu opens -- `controls::core_update::resolve_menu_scope` for a row, the group or
//! section membership for a heading -- so this module only ever draws what it was handed.
//!
//! The fitted menu keeps the named-build input in a self-contained `MoonDialog`, so its transient
//! state does not leak into `CoreStatusView`. Right-click wiring leaves the current selection
//! unchanged: opening a menu must not move the selection the menu is about to act on. Resolving a
//! `MoonDataTable` line index back to a row, and the row-level `on_right_click_row` hook, follow
//! `crates/moon-ui-gpui/src/panels/report/render.rs:89-111` -- a line index addresses a LINE, not a
//! core, because the grouped view draws each heading as its own synthetic row.

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonContextMenuWindowExt as _, MoonInput,
    MoonInputState, MoonMenuItem, MoonPalette, MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::feed::UpdateTarget;
use moon_core::session::CoreId;

use super::model::CoreStatusRow;
use crate::Backend;
use crate::controls::core_update::{
    OfferCounts, OfferState, offer_state, update_core, update_scope,
};
use crate::design::{self, moon};

/// Fitted-menu bounds for this two-item (plus separator) menu -- narrower than the shared coin
/// menu's range, since every label here is short and fixed.
const MENU_MIN_WIDTH: f32 = 200.0;
const MENU_MAX_WIDTH: f32 = 380.0;

/// Unique id for the named-build prompt dialog.
const NAMED_DIALOG_ID: &str = "core-update-named-dialog";

/// Largest named-build name kept, in characters -- this field has no sibling constant of its own
/// to reuse (unlike a saved core group's name), so it picks a sane length rather than staying
/// unbounded over the wire and in the persisted history.
const NAMED_BUILD_NAME_MAX: usize = 64;

/// How many core names the named-build dialog shows before the LIST starts scrolling.
///
/// The list scrolls; a NAME never shortens. A core name is what the operator typed into Moonbot and
/// is the only thing identifying which machine is about to be updated, so clipping one to fit is
/// the one economy this dialog may not make.
const NAMED_LIST_ROWS: f32 = 7.0;

/// Height of one name row in the dialog's scope list.
const NAMED_LIST_ROW_H: f32 = 18.0;

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
            }
        }
        Self {
            targets: Rc::from(targets),
            names: Rc::from(names),
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
        t!("core_update.menu.release_n", n = n).to_string()
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
            enqueue(&backend_r, &cores, UpdateTarget::Release, app);
        });
    }
    items.push(release_item);

    items.push(MoonMenuItem::separator());

    let named_label = if many {
        t!("core_update.menu.named_n", n = n).to_string()
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
            open_named_dialog(backend_n.clone(), scope_n.clone(), window, app);
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
///     Why nothing is offerable, or what a press would SKIP when only part of the scope is --
///     joined the way `controls::core_update::view`'s server tooltip joins the same two counts, so
///     the menu and that tooltip can never word one situation two ways. `None` when every core in
///     the scope would be accepted and there is nothing to warn about.
fn scope_note(scope: &UpdateScope) -> Option<String> {
    if !scope.offerable() {
        return Some(t!("core_update.menu.unavailable").to_string());
    }
    let mut parts = Vec::new();
    if scope.counts.offline > 0 {
        parts.push(t!("core_update.skipped_offline", n = scope.counts.offline).to_string());
    }
    if scope.counts.tracked > 0 {
        parts.push(t!("core_update.skipped_already", n = scope.counts.tracked).to_string());
    }
    (!parts.is_empty()).then(|| parts.join(" \u{2014} "))
}

/// Send one scope to the update queue.
///
/// Dispatched by scope SIZE, exactly as `controls::core_update::view`'s own button does: one core
/// goes through `update_core`, several fill the per-IP lane queue through `update_scope`. Both are
/// the shared entry points -- nothing here ever reaches `enqueue_core_update` directly, because the
/// queue is what serializes updates one-per-server.
///
/// Args:
///     backend: Shared terminal state.
///     cores: The scope to enqueue.
///     target: Release, or the build name the operator typed.
///     app: Application context used to reach the session.
fn enqueue(backend: &Entity<Backend>, cores: &Rc<[CoreId]>, target: UpdateTarget, app: &mut App) {
    if cores.len() == 1 {
        update_core(backend, cores[0], target, app);
    } else {
        update_scope(backend, cores, target, app);
    }
}

/// Normalize what the operator typed into the build name that goes over the wire.
///
/// A tester may type either the bare build name or paste the whole install command they have in
/// front of them (`InstallTestVersion MoonBot-F8`). `normalize_named_build` strips a leading
/// command-word TOKEN case-insensitively; `None` covers both an empty field and a value that is
/// ONLY the command word, and both mean "do nothing" -- there is no list to validate against, so
/// this is the only rejection the prompt can make.
///
/// Capping happens AFTER normalizing, never before: capping first could slice `InstallTestVersion`
/// mid-word and defeat the strip. Capped like `core_groups`' own sanitize shape, because this
/// travels unbounded over the MoonProto wire and is written verbatim into the durable
/// `cfg/core_updates.json` history otherwise.
///
/// Args:
///     raw: The field's current text.
///
/// Returns:
///     The build name to send, or `None` when there is nothing to send.
pub(super) fn typed_build_name(raw: &str) -> Option<String> {
    let normalized = moon_core::feed::normalize_named_build(raw)?;
    // Re-trim: truncation can leave a trailing space the normalized name had inside it.
    let typed: String = normalized
        .chars()
        .take(NAMED_BUILD_NAME_MAX)
        .collect::<String>()
        .trim()
        .to_string();
    (!typed.is_empty()).then_some(typed)
}

/// The scope's core names, in full, as a scrolling list.
///
/// Shared by this module's prompt and the footer's confirm so the two can never describe one scope
/// differently. Every name is drawn `whitespace_nowrap` and is NEVER truncated: the LIST scrolls
/// past its height cap instead. A core name is the operator's own text and the only thing that says
/// which machine is about to take a new build.
///
/// Args:
///     names: Verbatim core names, in the order they are drawn.
///     p: Active Moon palette.
///     cx: Application context used to scale the cap.
///
/// Returns:
///     The list element, or `None` for a single-core scope, which names its core in the prompt.
pub(super) fn scope_name_list(names: &[String], p: MoonPalette, cx: &App) -> Option<AnyElement> {
    if names.len() < 2 {
        return None;
    }
    let rows = names.len().min(NAMED_LIST_ROWS as usize) as f32;
    Some(
        v_flex()
            .id("core-update-scope-names")
            .w_full()
            .max_h(design::ui_px(cx, rows * NAMED_LIST_ROW_H))
            .overflow_y_scroll()
            // BOTH axes: a core name is arbitrary operator text and is never
            // shortened, so a name wider than the 360 px dialog has to be reachable
            // by scrolling rather than painted outside the list.
            .overflow_x_scroll()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .children(names.iter().map(|name| {
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .child(name.clone())
                    .into_any_element()
            }))
            .into_any_element(),
    )
}

/// Open the free-text "update to a named version" prompt for a scope.
///
/// Self-contained: the input's `Entity<MoonInputState>` is held only by this dialog's own builder
/// closure and needs no field anywhere else, following `core_group_dialogs::open_save_dialog`'s
/// shape (`crates/moon-ui-gpui/src/controls/core_group_dialogs.rs:140`). No list of builds is
/// offered -- MoonProto's `request_version_update` takes an arbitrary build name and the terminal
/// never learns what builds exist, so the field is deliberately free text.
///
/// Args:
///     backend: Shared terminal state, forwarded to the shared enqueue entry points on submit.
///     scope: Target cores and their verbatim names, captured at menu-click time.
///     window: Host window used to create the input and open the dialog.
///     app: Application context used to create the input and open the dialog.
///
/// Returns:
///     Nothing; opens the dialog as a side effect.
fn open_named_dialog(
    backend: Entity<Backend>,
    scope: Rc<UpdateScope>,
    window: &mut Window,
    app: &mut App,
) {
    let input = app.new(|cx| {
        MoonInputState::new(window, cx).placeholder(
            t!(
                "core_update.menu.named_ph",
                cmd = moon_core::feed::CORE_UPDATE_COMMAND_WORD
            )
            .to_string(),
        )
    });
    input
        .clone()
        .update(app, |input, cx| input.focus(window, cx));

    window.open_unique_moon_dialog(NAMED_DIALOG_ID, app, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let field = input.clone();
        let confirm_input = input.clone();
        let confirm_backend = backend.clone();
        let names = scope.names.clone();
        let cores = scope.targets.clone();
        let prompt = if names.len() > 1 {
            t!("core_update.menu.named_prompt_n", n = names.len()).to_string()
        } else {
            t!(
                "core_update.menu.named_prompt",
                core = names.first().cloned().unwrap_or_default()
            )
            .to_string()
        };
        let hint = t!("core_update.menu.named_hint").to_string();
        dialog
            .w(px(360.0))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .bg(moon(p.shell_high))
            .border_color(moon(p.border))
            .rounded(design::r_container(cx))
            .text_color(moon(p.text))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .border_b_1()
                    .border_color(moon(p.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("core_update.menu.named").to_string()),
            )
            .content(move |content, _window, cx| {
                content.child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .child(div().text_color(moon(p.text_muted)).child(prompt.clone()))
                        .children(scope_name_list(&names, p, cx))
                        .child(
                            MoonInput::new("core-update-named-input")
                                .state(&field)
                                .small(),
                        )
                        .child(div().text_color(moon(p.text_muted)).child(hint.clone())),
                )
            })
            .footer(named_footer(confirm_input, confirm_backend, cores, p))
    });
}

/// Cancel and Done for the named-build prompt.
///
/// Args:
///     input: The dialog's own input state, read once on confirm.
///     backend: Shared terminal state, forwarded on a non-empty confirm.
///     cores: Target cores captured at menu-click time.
///     p: Active Moon palette.
///
/// Returns:
///     The rendered footer row.
fn named_footer(
    input: Entity<MoonInputState>,
    backend: Entity<Backend>,
    cores: Rc<[CoreId]>,
    p: MoonPalette,
) -> gpui::AnyElement {
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .text_color(moon(p.text))
        .child(
            MoonButton::new("core-update-named-cancel")
                .ghost()
                .size(MoonButtonSize::Micro)
                .label(t!("dialogs.cancel").to_string())
                .on_click(move |_, window, cx| window.close_dialog(cx))
                .render(),
        )
        .child(
            MoonButton::new("core-update-named-confirm")
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Blue)
                .label(t!("dialogs.done").to_string())
                .on_click(move |_, window, cx| {
                    let typed = typed_build_name(&input.read(cx).value());
                    window.close_dialog(cx);
                    let Some(typed) = typed else {
                        return;
                    };
                    enqueue(&backend, &cores, UpdateTarget::Named(typed), cx);
                })
                .render(),
        )
        .into_any_element()
}
