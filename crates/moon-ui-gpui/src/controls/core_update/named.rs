//! The named-build prompt: a free-text field for a beta/test build name, the hint that spells out
//! every form the field accepts, and the dialog that sends what was typed.
//!
//! Shared rather than owned by one panel, because three entry points ask for a build name: the
//! Core Status row menu, the footer confirm (which embeds the field pieces in its own dialog), and
//! the retry of a failed NAMED attempt -- which reopens the field with the old name in it instead
//! of sending that name blind a second time. No list of builds is offered: MoonProto's
//! `request_version_update` takes an arbitrary name and the terminal never learns what builds
//! exist, so the field is deliberately free text.

use std::rc::Rc;

use gpui::*;
use moon_core::feed::{CORE_UPDATE_ALL_WORD, CORE_UPDATE_COMMAND_WORD, UpdateTarget};
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonInput, MoonInputState, MoonPalette, MoonWindowExt as _,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::update_scope;
use crate::Backend;
use crate::design::{self, moon};

/// Unique id for the named-build prompt dialog.
const NAMED_DIALOG_ID: &str = "core-update-named-dialog";

/// Largest named-build name kept, in characters -- this field has no sibling constant of its own
/// to reuse (unlike a saved core group's name), so it picks a sane length rather than staying
/// unbounded over the wire and in the persisted history.
const NAMED_BUILD_NAME_MAX: usize = 64;

/// How many core names a scope list shows before the LIST starts scrolling.
///
/// The list scrolls; a NAME never shortens. A core name is what the operator typed into Moonbot and
/// is the only thing identifying which machine is about to be updated, so clipping one to fit is
/// the one economy this dialog may not make.
const NAMED_LIST_ROWS: f32 = 7.0;

/// Height of one name row in a scope list.
const NAMED_LIST_ROW_H: f32 = 18.0;

/// Normalize what the operator typed into the build name that goes over the wire.
///
/// A tester may type either the bare build name or paste the whole install command they have in
/// front of them (`InstallTestVersion MoonBot-F8`, optionally with a trailing `ALL`).
/// `normalize_named_build` strips both; `None` covers an empty field and a value that is ONLY the
/// command word, and both mean "do nothing" -- there is no list to validate against, so this is
/// the only rejection the prompt can make.
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
pub(crate) fn typed_build_name(raw: &str) -> Option<String> {
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

/// Create the focused build-name field.
///
/// The placeholder is only the bare example name: the dialog is 360 px wide, and the old
/// placeholder -- the name AND the whole command -- was cut mid-word. The full forms live in
/// [`build_name_hint`] under the field, where they wrap instead of clipping.
///
/// Args:
///     initial: Text to start with -- the name a failed attempt used, so a retry corrects it
///         instead of retyping it. `None` starts empty.
///     window: Window that will host the field.
///     cx: Application context used to create and focus the input.
///
/// Returns:
///     The input state, already focused.
pub(crate) fn build_name_input(
    initial: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<MoonInputState> {
    let input = cx.new(|cx| {
        let state = MoonInputState::new(window, cx);
        match initial {
            Some(text) => state.default_value(text),
            None => state,
        }
    });
    input
        .clone()
        .update(cx, |input, cx| input.focus(window, cx));
    input
}

/// The hint under the build-name field: every accepted form written out in full, and what `ALL`
/// does NOT do here.
///
/// The three forms are assembled from the same constants `normalize_named_build` strips, so the
/// hint cannot advertise a spelling the field would not accept. Each sits on its own mono line and
/// never truncates. The last line says `ALL` does not widen the scope: in MoonBot's own command it
/// means every bot on the machine, here only the cores listed in the dialog are updated.
///
/// Args:
///     p: Active Moon palette.
///     cx: Application context used for the text tiers.
///
/// Returns:
///     The hint block.
pub(crate) fn build_name_hint(p: MoonPalette, cx: &App) -> AnyElement {
    let example = t!("core_update.menu.named_ph").to_string();
    let forms = [
        example.clone(),
        format!("{CORE_UPDATE_COMMAND_WORD} {example}"),
        format!("{CORE_UPDATE_COMMAND_WORD} {example} {CORE_UPDATE_ALL_WORD}"),
    ];
    v_flex()
        .w_full()
        .gap_0p5()
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted))
        .child(div().child(t!("core_update.menu.named_hint").to_string()))
        .children(forms.into_iter().map(|form| {
            div()
                .pl(design::ui_px(cx, 8.0))
                .font_family(design::mono())
                .text_color(moon(p.text_soft))
                .child(form)
        }))
        .child(
            div().child(
                t!(
                    "core_update.menu.named_hint_all",
                    all = CORE_UPDATE_ALL_WORD
                )
                .to_string(),
            ),
        )
        .into_any_element()
}

/// The scope's core names, in full, as a scrolling list.
///
/// Shared by the named prompt and the footer's confirm so the two can never describe one scope
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
pub(crate) fn scope_name_list(names: &[String], p: MoonPalette, cx: &App) -> Option<AnyElement> {
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
/// shape (`crates/moon-ui-gpui/src/controls/core_group_dialogs.rs:140`).
///
/// Args:
///     backend: Shared terminal state, forwarded to the shared enqueue entry point on submit.
///     cores: Target cores, captured when the prompt was asked for.
///     names: Verbatim display names parallel to `cores`.
///     initial: Name to prefill -- the one a failed attempt used -- or `None` for an empty field.
///     window: Host window used to create the input and open the dialog.
///     app: Application context used to create the input and open the dialog.
///
/// Returns:
///     Nothing; opens the dialog as a side effect.
pub(crate) fn open_named_dialog(
    backend: Entity<Backend>,
    cores: Rc<[CoreId]>,
    names: Rc<[String]>,
    initial: Option<String>,
    window: &mut Window,
    app: &mut App,
) {
    let input = build_name_input(initial, window, app);

    window.open_unique_moon_dialog(NAMED_DIALOG_ID, app, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let field = input.clone();
        let confirm_input = input.clone();
        let confirm_backend = backend.clone();
        let names = names.clone();
        let cores = cores.clone();
        let prompt = if names.len() > 1 {
            t!(
                crate::display_text::plural_key(
                    "core_update.menu.named_prompt_n",
                    names.len() as u64
                ),
                n = names.len()
            )
            .to_string()
        } else {
            t!(
                "core_update.menu.named_prompt",
                core = names.first().cloned().unwrap_or_default()
            )
            .to_string()
        };
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
                                .placeholder(t!("core_update.menu.named_ph").to_string())
                                .size(design::INPUT_SIZE),
                        )
                        .child(build_name_hint(p, cx)),
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
///     cores: Target cores captured when the prompt was asked for.
///     p: Active Moon palette.
///
/// Returns:
///     The rendered footer row.
fn named_footer(
    input: Entity<MoonInputState>,
    backend: Entity<Backend>,
    cores: Rc<[CoreId]>,
    p: MoonPalette,
) -> AnyElement {
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .text_color(moon(p.text))
        .child(
            MoonButton::new("core-update-named-cancel")
                .ghost()
                .label(t!("dialogs.cancel").to_string())
                .on_click(move |_, window, cx| window.close_dialog(cx))
                .render(),
        )
        .child(
            MoonButton::new("core-update-named-confirm")
                .variant(MoonButtonVariant::Blue)
                .label(t!("dialogs.done").to_string())
                .on_click(move |_, window, cx| {
                    let typed = typed_build_name(&input.read(cx).value());
                    window.close_dialog(cx);
                    let Some(typed) = typed else {
                        return;
                    };
                    update_scope(&backend, &cores, UpdateTarget::Named(typed), window, cx);
                })
                .render(),
        )
        .into_any_element()
}
