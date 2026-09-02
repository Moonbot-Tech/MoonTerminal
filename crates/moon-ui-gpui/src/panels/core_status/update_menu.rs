//! Right-click context menu for a Core Status core row: "update to release" -- the same action
//! the row button fires -- and, one level deeper, a free-text prompt for a named beta/test build.
//! Both dispatch through the shared `controls::core_update::update_core`, so the menu can never do
//! anything the row button could not already do.
//!
//! The fitted menu keeps the named-build input in a self-contained `MoonDialog`, so its transient
//! state does not leak into `CoreStatusView`. Right-click wiring leaves the current table selection
//! unchanged: opening a menu must not move the selection the menu is about to act on. Resolving a
//! `MoonDataTable` line index back to a row, and the row-level `on_right_click_row` hook, follow
//! `crates/moon-ui-gpui/src/panels/report/render.rs:89-111` -- a line index addresses a LINE, not a
//! core, because the grouped view draws each heading as its own synthetic row.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonContextMenuWindowExt as _, MoonInput,
    MoonInputState, MoonMenuItem, MoonPalette, MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::feed::{ConnStatus, UpdateTarget};
use moon_core::session::CoreId;
use moon_core::session::core_update::CoreUpdatePhase;

use crate::Backend;
use crate::controls::core_update::{OfferState, offer_state, update_core};
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

/// Whether `core` may be enqueued for an update right now: connected and settled (`Ready`), with
/// a known build and a known address, and no live attempt already tracked for it.
///
/// Delegates to [`offer_state`] -- the same rule the row's own update button already draws
/// from -- rather than keeping a second, weaker copy that checked only `status` and `update`.
/// MoonProto's lifecycle events do not arrive in a fixed order, so a core can be `Ready` before
/// it has reported a `server_version` or an endpoint; the weaker copy enabled the menu in exactly
/// that window, `enqueue_core_update` silently rejected the click, and the user got a
/// `log::warn!` they never saw. Found by three independent review passes.
///
/// Args:
///     status: The row's current connection status.
///     server_version: The row's last reported build, when it reported one.
///     endpoint_known: Whether the row's address has reached the store.
///     update: The row's currently tracked update phase, if any.
///
/// Returns:
///     Whether the row meets the standard offer-state conditions that enable the menu entries.
pub(super) fn core_updatable(
    status: &ConnStatus,
    server_version: Option<u32>,
    endpoint_known: bool,
    update: Option<&CoreUpdatePhase>,
) -> bool {
    matches!(
        offer_state(status, server_version, endpoint_known, update),
        OfferState::Offerable
    )
}

/// Open the core row's context menu at `pos`.
///
/// Both entries are always present and DISABLED (never hidden) when `updatable` is false, plus a
/// leading disabled row naming the reason -- the row must not silently offer nothing, and the menu
/// must not change shape between the enqueueable and non-enqueueable case.
///
/// Args:
///     backend: Shared terminal state, forwarded to `update_core` unchanged.
///     core: Row's core identity.
///     core_name: Row's display name, shown in the named-build prompt.
///     updatable: [`core_updatable`] for this row, computed by the caller from data it already
///         has so this module needs no extra read of the row.
///     pos: Window-coordinate open point -- the click position, or `window.mouse_position()` from
///         a `MoonDataTable` row handler that receives only a line index.
///     window: Host window used to open the fitted menu.
///     cx: Application context used to open the fitted menu.
///
/// Returns:
///     Nothing; opens the menu as a side effect.
pub(super) fn open_update_row_menu(
    backend: &Entity<Backend>,
    core: CoreId,
    core_name: String,
    updatable: bool,
    pos: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut items: Vec<MoonMenuItem> = Vec::new();
    if !updatable {
        items.push(
            MoonMenuItem::with_key(
                "core-update-unavailable",
                t!("core_update.menu.unavailable").to_string(),
            )
            .disabled(true),
        );
    }

    let mut release_item = MoonMenuItem::with_key(
        "core-update-release",
        t!("core_update.menu.release").to_string(),
    )
    .disabled(!updatable);
    if updatable {
        let backend_r = backend.clone();
        release_item = release_item.on_click(move |_, window, app| {
            window.close_context_menu(app);
            update_core(&backend_r, core, UpdateTarget::Release, app);
        });
    }
    items.push(release_item);

    items.push(MoonMenuItem::separator());

    let mut named_item = MoonMenuItem::with_key(
        "core-update-named",
        t!("core_update.menu.named").to_string(),
    )
    .disabled(!updatable);
    if updatable {
        let backend_n = backend.clone();
        let core_name = core_name.clone();
        named_item = named_item.on_click(move |_, window, app| {
            window.close_context_menu(app);
            open_named_dialog(backend_n.clone(), core, core_name.clone(), window, app);
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

/// Open the free-text "update to a named version" prompt.
///
/// Self-contained: the input's `Entity<MoonInputState>` is held only by this dialog's own builder
/// closure and needs no field anywhere else, following `core_group_dialogs::open_save_dialog`'s
/// shape (`crates/moon-ui-gpui/src/controls/core_group_dialogs.rs:140`). No list is offered --
/// MoonProto's `request_version_update` takes an arbitrary build name and the terminal never
/// learns what builds exist, so the field is deliberately free text. Submission normalizes a
/// pasted complete install command to its bare name; an empty result still means "do nothing".
///
/// Args:
///     backend: Shared terminal state, forwarded to `update_core` on submit.
///     core: Target core identity, captured at menu-click time.
///     core_name: Target core's display name, shown so a fleet-wide user can confirm the target
///         before typing a build name.
///     window: Host window used to create the input and open the dialog.
///     app: Application context used to create the input and open the dialog.
///
/// Returns:
///     Nothing; opens the dialog as a side effect.
fn open_named_dialog(
    backend: Entity<Backend>,
    core: CoreId,
    core_name: String,
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
        let prompt = t!("core_update.menu.named_prompt", core = core_name.clone()).to_string();
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
            .content(move |content, _window, _cx| {
                content.child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .child(div().text_color(moon(p.text_muted)).child(prompt.clone()))
                        .child(
                            MoonInput::new("core-update-named-input")
                                .state(&field)
                                .small(),
                        )
                        .child(div().text_color(moon(p.text_muted)).child(hint.clone())),
                )
            })
            .footer(named_footer(confirm_input, confirm_backend, core, p))
    });
}

/// Cancel and Done for the named-build prompt.
///
/// Args:
///     input: The dialog's own input state, read once on confirm.
///     backend: Shared terminal state, forwarded to `update_core` on a non-empty confirm.
///     core: Target core identity captured at menu-click time.
///     p: Active Moon palette.
///
/// Returns:
///     The rendered footer row.
fn named_footer(
    input: Entity<MoonInputState>,
    backend: Entity<Backend>,
    core: CoreId,
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
                    // A tester may type either the bare build name or paste the whole install
                    // command they have in front of them (`InstallTestVersion MoonBot-F8`).
                    // `normalize_named_build` strips a
                    // leading command-word TOKEN case-insensitively; `None` covers both an empty
                    // field and a value that is ONLY the command word, and both close the dialog
                    // without sending anything -- there is no list to validate against, so this is
                    // still the only rejection this dialog can make.
                    let Some(normalized) =
                        moon_core::feed::normalize_named_build(&input.read(cx).value())
                    else {
                        window.close_dialog(cx);
                        return;
                    };
                    // Cap AFTER normalizing, never before: capping first could slice
                    // `InstallTestVersion` mid-word and defeat the strip above. Capped like
                    // `core_groups`' own sanitize shape: this travels unbounded over the MoonProto
                    // wire and is written verbatim into the durable `cfg/core_updates.json` history
                    // otherwise.
                    let typed: String = normalized.chars().take(NAMED_BUILD_NAME_MAX).collect();
                    // Re-trim: truncation can leave a trailing space the normalized name had
                    // inside it.
                    let typed = typed.trim().to_string();
                    window.close_dialog(cx);
                    if typed.is_empty() {
                        return;
                    }
                    update_core(&backend, core, UpdateTarget::Named(typed), cx);
                })
                .render(),
        )
        .into_any_element()
}
