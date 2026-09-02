//! Hosts the core-settings popover opened by the gear button beside the header core selector.
//! Owns open state, the staged draft both tabs edit, the two blacklist editors, and the Cancel All
//! confirmation. `crate::shell::core_settings_popup` renders the content.
//!
//! Everything below the tab strip commits on OK through [`Shell::commit_core_draft`]; the actions
//! ABOVE it (restart, emulator, cancel all) act immediately, because none of them is a value to be
//! reviewed before committing.

pub(crate) mod draft;

use gpui::*;

use moon_ui::MoonPalette;

use moon_core::session::CoreId;

use crate::shell::core_settings_popup::{self, CoreSettingsTab};

use super::Shell;

/// The core a core-settings write may address, or `None` when it must be dropped.
///
/// The gear popup's draft is SEEDED from one core when it opens and then lives on unchanged, while
/// the active trading core can move underneath it — the header selector, a Main-chart coin switch
/// through [`crate::Backend::active_trade_core`]'s fallback, or a core going away entirely.
/// Resolving the core again at event time would write the values the user is looking at into a core
/// they are not looking at.
///
/// [`Shell::reconcile_core_settings_popup`] takes such a popup off the screen, but only at the next
/// render, and repaints pass three stacked throttles — a slider drag lands inside that window. So
/// the check has to happen HERE too, at event time, exactly as [`crate::controls::MetricTarget`]
/// does for the toolbar metric popups.
///
/// A `None` seed means the popup belongs to no core — the group had no active core when it opened.
/// It is deliberately NOT treated as "any core will do": there is nothing on screen to commit.
pub(crate) fn resolve_core_settings_write(
    seeded: Option<CoreId>,
    active: Option<CoreId>,
) -> Option<CoreId> {
    match (seeded, active) {
        (Some(seeded), Some(active)) if seeded == active => Some(seeded),
        _ => None,
    }
}

impl Shell {
    /// Handle `MoonPopover::on_open_change` for the core-settings gear button.
    ///
    /// Opening collapses the blacklist editor, records the core the popup belongs to and seeds the
    /// draft from it; closing drops the draft, which is what makes dismissing the popover behave
    /// like Cancel rather than like a silent half-commit.
    pub(crate) fn set_core_settings_open(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.core_settings_open == open {
            return;
        }
        self.core_settings_open = open;
        self.core_settings_cancel_confirm = false;
        if open {
            self.core_settings_bl_expanded = false;
            self.core_settings_target = self.backend.read(cx).active_trade_core(&self.group);
            // The draft stays empty when the core's full configuration has not arrived yet; the
            // next render seeds it once it does.
            if self.seed_core_draft(cx) {
                self.seed_blacklist_editors(window, cx);
            }
        } else {
            self.close_core_settings_popup();
        }
        cx.notify();
    }

    /// Drop the gear popup once it no longer belongs to the core it was seeded from.
    ///
    /// The active trading core moves without producing an event to hang this on — the header
    /// selector, a Main-chart coin switch, a core being disabled — so it is reconciled at render
    /// like the state the panels poll, mirroring [`Shell::reconcile_metric_popup`].
    ///
    /// The draft is DISCARDED rather than flushed: it was never confirmed with OK, and the core it
    /// describes is no longer the one on screen.
    pub(super) fn reconcile_core_settings_popup(&mut self, cx: &App) {
        if !self.core_settings_open {
            return;
        }
        let b = self.backend.read(cx);
        let active = b.active_trade_core(&self.group);
        let Some(core) = resolve_core_settings_write(self.core_settings_target, active) else {
            if self.core_settings_draft.is_some() {
                log::info!("core settings popup closed with unsaved edits: the active core moved");
            }
            self.close_core_settings_popup();
            return;
        };
        // A DIFFERENT MoonBot process now answers on this connection: the store drops the retained
        // configuration, and the draft copied from it describes the instance that went away. The
        // popup has to go with it, or OK would write the departed process's whole page into its
        // replacement — which is what dropping the store's copy alone cannot prevent, since the
        // draft is only ever seeded while it is `None`.
        let core_config_gone = b
            .session
            .store()
            .core(core)
            .is_none_or(|d| d.core_config.is_none());
        if core_config_gone && self.core_settings_draft.is_some() {
            log::info!("core settings popup closed: the core it was opened over is gone");
            self.close_core_settings_popup();
        }
    }

    /// Clear every piece of gear-popup state in one place, so no path can close it halfway.
    pub(super) fn close_core_settings_popup(&mut self) {
        self.core_settings_open = false;
        self.core_settings_target = None;
        self.core_settings_cancel_confirm = false;
        self.core_settings_draft = None;
        self.core_settings_seed = None;
        // The editors go with the draft on purpose: one retained past its draft would seed the NEXT
        // core's tab with the previous core's text on its first frame, before the generation check
        // in `core_settings_input` had a value to correct it with.
        self.core_settings_inputs.clear();
        self.core_settings_sliders.clear();
    }

    /// Switch the visible tab, taking the blacklist text with it.
    ///
    /// Leaving the General tab unmounts its blacklist editor, and this codebase already documents
    /// that replacing that element can prevent its Blur from arriving — so the text is staged here
    /// rather than left to an event that may never fire.
    pub(crate) fn set_core_settings_tab(&mut self, tab: CoreSettingsTab, cx: &mut Context<Self>) {
        if self.core_settings_tab == tab {
            return;
        }
        if self.core_settings_tab == CoreSettingsTab::General {
            self.stage_blacklist_text(cx);
        }
        self.core_settings_tab = tab;
        cx.notify();
    }

    /// Copy the visible blacklist editor's text into the draft.
    ///
    /// Called from the editors' Blur/Enter subscriptions, from the expand toggle, and from a tab
    /// switch. Staging rather than sending: the blacklist belongs to the same OK as everything else
    /// below the tab strip.
    pub(crate) fn stage_blacklist_text(&mut self, cx: &mut Context<Self>) {
        let editor = if self.core_settings_bl_expanded {
            &self.blacklist_area
        } else {
            &self.blacklist_input
        };
        let text = editor.read(cx).value().to_string();
        self.edit_core_draft(|draft| draft.general.blacklist_text = text, cx);
    }

    /// Seed both blacklist editors from the draft.
    fn seed_blacklist_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self
            .core_settings_draft
            .as_ref()
            .map(|d| d.general.blacklist_text.clone())
        else {
            return;
        };
        self.blacklist_input
            .update(cx, |st, c| st.set_value(text.clone(), window, c));
        self.blacklist_area
            .update(cx, |st, c| st.set_value(text, window, c));
    }

    /// Require one confirmation click before cancelling all orders for the popup's core.
    ///
    /// The confirmation was armed against the core the popup was seeded from; if the active core
    /// moved between the two clicks, the second click cancels nothing rather than wiping the orders
    /// of a core the user never armed it for.
    pub(super) fn core_settings_cancel_all_click(&mut self, cx: &mut Context<Self>) {
        if !self.core_settings_cancel_confirm {
            self.core_settings_cancel_confirm = true;
            cx.notify();
            return;
        }
        self.core_settings_cancel_confirm = false;
        let b = self.backend.read(cx);
        if let Some(core) =
            resolve_core_settings_write(self.core_settings_target, b.active_trade_core(&self.group))
            && let Err(error) = b.session.cancel_all_orders(core)
        {
            log::warn!("cancel all orders failed: {error:#}");
        }
        cx.notify();
    }

    /// Build content for the gear button's anchored `MoonPopover` while it is open.
    /// The popover owns trigger-relative positioning, avoiding the stale absolute coordinates used
    /// before the ticker was added to the header.
    ///
    /// Takes `&mut self` and a window because both tabs create their editors on first render of
    /// each row, exactly as the strategy parameter form does: between them they hold two dozen, and
    /// building all of them up front would cost every session that never opens this popup.
    pub(super) fn core_settings_popup_content(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Seed here rather than in `set_core_settings_open` alone: the popup can be opened before
        // the core's full configuration has arrived, and this is the first place afterwards that has
        // both a window and a mutable Shell.
        if self.core_settings_open && self.core_settings_draft.is_none() && self.seed_core_draft(cx)
        {
            self.seed_blacklist_editors(window, cx);
        }
        // An open popup nobody has typed into follows the core; see `refresh_untouched_core_draft`.
        if self.refresh_untouched_core_draft(cx) {
            self.seed_blacklist_editors(window, cx);
        }
        let widgets = self.core_settings_widgets(window, cx);
        let view = cx.entity();
        let cancel_view = view.clone();
        let toggle_view = view.clone();
        let ctx = core_settings_popup::TabCtx {
            backend: &self.backend,
            group: &self.group,
            seeded: self.core_settings_target,
            p,
        };
        let editors = core_settings_popup::TextEditors {
            input: &self.blacklist_input,
            area: &self.blacklist_area,
        };
        core_settings_popup::core_settings_content(
            &ctx,
            self.core_settings_tab,
            self.core_settings_draft.as_ref(),
            &widgets,
            &editors,
            self.core_settings_bl_expanded,
            self.core_settings_cancel_confirm,
            &view,
            cx,
            move |app| cancel_view.update(app, |this, cx| this.core_settings_cancel_all_click(cx)),
            move |window, app| {
                toggle_view.update(app, |this, cx| {
                    let expanding = !this.core_settings_bl_expanded;
                    // Stage from the editor that is on screen NOW, before the flag flips: after it,
                    // `stage_blacklist_text` would read the empty one.
                    this.stage_blacklist_text(cx);
                    this.core_settings_bl_expanded = expanding;
                    // Synchronize text between distinct single-line and multiline states. Reusing
                    // one state is intentionally avoided because textarea setup mutates it
                    // irreversibly.
                    let text = this
                        .core_settings_draft
                        .as_ref()
                        .map(|d| d.general.blacklist_text.clone())
                        .unwrap_or_default();
                    if expanding {
                        this.blacklist_area
                            .update(cx, |st, c| st.set_value(text, window, c));
                    } else {
                        this.blacklist_input
                            .update(cx, |st, c| st.set_value(text, window, c));
                    }
                    cx.notify();
                })
            },
        )
    }

    /// Create or synchronize the editors both tabs render and hand them to the renderer.
    ///
    /// Returns an empty set while no draft exists, which is what the waiting placeholder renders
    /// against.
    fn core_settings_widgets(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> core_settings_popup::SettingsWidgets {
        let Some(draft) = self.core_settings_draft.clone() else {
            return core_settings_popup::SettingsWidgets::default();
        };
        let fields = core_settings_popup::field_specs(&draft)
            .into_iter()
            .map(|(id, value, stage, width)| core_settings_popup::NumField {
                state: self.core_settings_input(id, value, stage, window, cx),
                id,
                width,
            })
            .collect();
        let sliders = core_settings_popup::slider_specs(&draft)
            .into_iter()
            .map(
                |(id, bounds, value, stage, mirror)| core_settings_popup::SliderRow {
                    state: self.core_settings_slider(id, bounds, value, stage, mirror, window, cx),
                    id,
                },
            )
            .collect();
        core_settings_popup::SettingsWidgets { fields, sliders }
    }
}

#[cfg(test)]
mod tests;
