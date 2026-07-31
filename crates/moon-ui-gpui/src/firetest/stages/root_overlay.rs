//! Stage `root_overlay_contract`: the MoonUI `Root`-owned overlay layer behaves on a real window.
//!
//! Popups, dialogs and notifications belong to `Root`, never to a panel child — that is what gives
//! them correct z-order and dismissal. This stage drives the whole contract on the live Strategies
//! window: a context menu becomes active, opening a dialog dismisses it, a unique dialog id
//! replaces rather than stacks, a notification is counted, and cleanup leaves nothing behind.

use gpui::{Context, IntoElement, ParentElement, div, px};
use moon_ui::MoonNotification;

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;

impl Runtime {
    /// Run the full overlay contract inside the Strategies window and clean up after itself.
    pub(in crate::firetest) fn verify_root_overlay_contract(
        &self,
        backend: &mut Backend,
        cx: &mut Context<Backend>,
    ) -> Result<(), String> {
        let Some(handle) = backend.strategies_window else {
            return Err("strategies tool window is required for Root overlay contract".into());
        };
        handle
            .update(cx, |root, window, cx| {
                root.close_context_menu(window, cx);
                root.close_all_dialogs(window, cx);
                root.clear_notifications(window, cx);

                root.open_context_menu(|_window, _cx| div().into_any_element(), window, cx);
                if !root.has_active_context_menu() {
                    return Err("Root context menu did not become active".to_string());
                }

                root.open_unique_moon_dialog(
                    "firetest-root-dialog",
                    |dialog, _window, _cx| {
                        dialog
                            .w(px(260.0))
                            .title(div().child("FireTest dialog"))
                            .content(|content, _window, _cx| {
                                content.child(div().child("Root-owned overlay"))
                            })
                    },
                    window,
                    cx,
                );
                if root.has_active_context_menu() {
                    return Err("Root context menu stayed active after dialog open".to_string());
                }
                if root.active_dialog_count() != 1 {
                    return Err(format!(
                        "Root unique dialog count after first open is {}, expected 1",
                        root.active_dialog_count()
                    ));
                }

                root.open_unique_moon_dialog(
                    "firetest-root-dialog",
                    |dialog, _window, _cx| {
                        dialog
                            .w(px(260.0))
                            .title(div().child("FireTest dialog replacement"))
                            .content(|content, _window, _cx| {
                                content.child(div().child("Replacement"))
                            })
                    },
                    window,
                    cx,
                );
                if root.active_dialog_count() != 1 {
                    return Err(format!(
                        "Root unique dialog replacement created {} dialogs",
                        root.active_dialog_count()
                    ));
                }

                root.push_notification(
                    MoonNotification::error("FireTest root notification").autohide(false),
                    window,
                    cx,
                );
                if root.notification_count(cx) != 1 {
                    return Err(format!(
                        "Root notification count is {}, expected 1",
                        root.notification_count(cx)
                    ));
                }

                root.close_all_dialogs(window, cx);
                root.clear_notifications(window, cx);
                if root.active_dialog_count() != 0 || root.notification_count(cx) != 0 {
                    return Err("Root overlay cleanup left dialog or notification active".into());
                }

                firetest_info(
                    "[firetest] root_overlay_contract context_menu dialog notification ok",
                );
                Ok(())
            })
            .map_err(|error| format!("Root overlay contract window update failed: {error}"))?
    }
}
