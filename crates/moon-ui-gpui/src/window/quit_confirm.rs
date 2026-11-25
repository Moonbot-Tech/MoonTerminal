//! The question asked before the last main window's close quits the whole terminal (#1170).
//!
//! The main window's cross sits in the same corner as a chart window's, and the taskbar cannot
//! tell the two apart, so the click that quits the terminal is the one aimed at a chart. With
//! `AppConfig::confirm_quit` on, that close asks first; with it off — the default — nothing here
//! intervenes and the close quits at once, exactly as before.
//!
//! The hook is the window's `on_window_should_close`, which every user close of a main window
//! already passes through: on Windows the frame's cross (a `window_control_area` hit-test the
//! platform turns into `WM_CLOSE`), Alt+F4, the system menu and the taskbar; on Linux MoonUI's
//! client-drawn cross (`Window::request_close`) and the native close message; on macOS the red
//! traffic light (`windowShouldClose:`). A programmatic `remove_window` — group windows closed by
//! a Settings rebuild, detached charts dying with their owner, the quit itself — never reaches it.
//!
//! Not asked, deliberately: a main window that is not the last one (closing it does not quit),
//! a chart window's cross (it returns the tab), the minimize dash, and an application quit that
//! closes no main window — macOS Dock "Quit" or logout reach AppKit's `terminate:`, which the
//! MoonUI fork gives the application no veto over. The core's own «Спрашивать при выходе» expert
//! checkbox is MoonBot's and is not read here.

use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonWindowExt as _, h_flex};
use rust_i18n::t;

use crate::{Backend, design};

/// The one dialog id, so a second close request while the question is up replaces it.
const DIALOG_ID: &str = "quit-confirm";

/// What a main window's close request does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseRequest {
    /// Let the window close now; whatever followed that close before still follows it.
    Close,
    /// Keep the window and ask whether to quit the terminal.
    Ask,
}

/// Decide a main window's close request.
///
/// Asks only when every condition holds: the user turned the question on, the application is not
/// already quitting (its own teardown must never stop on a dialog), and the close would quit the
/// terminal because no other main window is left. The dialog's Close answers with
/// `remove_window`, which does not consult this policy again, so no "already confirmed" state is
/// needed.
///
/// Args:
///     ask_enabled: The saved `confirm_quit` setting.
///     quitting: Whether `on_app_quit` already runs.
///     quits_terminal: Whether closing this window removes the last main window.
///
/// Returns:
///     `Ask` when the close must wait for the user's answer, `Close` otherwise.
pub(crate) fn decide(ask_enabled: bool, quitting: bool, quits_terminal: bool) -> CloseRequest {
    if ask_enabled && !quitting && quits_terminal {
        CloseRequest::Ask
    } else {
        CloseRequest::Close
    }
}

/// Whether closing `closing` would leave no main window, the condition under which the
/// `on_window_closed` handler in `startup/boot.rs` quits the terminal.
///
/// Mirrors `Backend::close_group_window`: the closed window must be a registered main window, and
/// the registry must be empty once it is gone. An unregistered window does not quit on close, so it
/// is not asked about either.
///
/// Generic over the id so the rule is testable without a live window registry.
///
/// Args:
///     registered: Ids of every registered main window.
///     closing: Id of the window whose close is requested.
///
/// Returns:
///     `true` when `closing` is registered and no other main window is.
fn closes_last_main_window<T: PartialEq>(
    registered: impl IntoIterator<Item = T>,
    closing: T,
) -> bool {
    let mut found = false;
    for id in registered {
        if id == closing {
            found = true;
        } else {
            return false;
        }
    }
    found
}

/// Install the quit question on a newly opened main window.
///
/// The setting is read from the SAVED config at the moment of the close, so ticking the box in
/// Settings takes effect on Save, and an unsaved tick changes nothing.
///
/// Args:
///     window: The main window being created.
///     backend: Shared state holding the saved config and the main-window registry.
///     cx: Application context the policy is registered with.
pub(crate) fn install(window: &mut Window, backend: Entity<Backend>, cx: &App) {
    window.on_window_should_close(cx, move |window, app| {
        let closing = window.window_handle().window_id();
        let (enabled, quitting, last) = {
            let b = backend.read(app);
            (
                b.config.confirm_quit,
                b.quitting,
                closes_last_main_window(b.group_windows.values().map(|h| h.window_id()), closing),
            )
        };
        match decide(enabled, quitting, last) {
            CloseRequest::Close => true,
            CloseRequest::Ask => {
                ask(window, app);
                false
            }
        }
    });
}

/// Ask whether to quit the terminal. Stay — or closing the dialog any other way — leaves
/// everything as it was; Close removes the window, and its removal reaches the same
/// `on_window_closed` handler an unasked close does, so the quit and the layout written on the way
/// out are exactly as before.
///
/// The window is activated first: a close from the taskbar can arrive while it is minimized or
/// behind other windows, and a question nobody can see reads as a cross that does nothing.
///
/// Args:
///     window: The main window whose close is on hold.
///     app: Application context.
fn ask(window: &mut Window, app: &mut App) {
    window.activate_window();
    window.open_unique_moon_dialog(DIALOG_ID, app, move |dialog, _, cx| {
        let body = t!("shell.quit_confirm.body").to_string();
        crate::panels::order_dialog_chrome(
            dialog,
            420.0,
            t!("shell.quit_confirm.title").to_string(),
            cx,
        )
        // Scaled with the UI zoom, like the text inside it.
        .w(design::ui_px(cx, 420.0))
        .content(move |content, _, _| content.child(div().w_full().child(body.clone())))
        .footer(
            h_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .justify_end()
                .child(
                    MoonButton::new("quit-confirm-stay")
                        .outline()
                        .label(t!("shell.quit_confirm.stay").to_string())
                        .on_click(|_, window, cx| window.close_dialog(cx))
                        .render(),
                )
                .child(
                    MoonButton::new("quit-confirm-quit")
                        .variant(MoonButtonVariant::Danger)
                        .label(t!("shell.quit_confirm.quit").to_string())
                        .on_click(|_, window, cx| {
                            window.close_dialog(cx);
                            window.remove_window();
                        })
                        .render(),
                ),
        )
    });
}

#[cfg(test)]
mod tests;
