//! Hosts the core-settings popover opened by the gear button beside the header core selector.
//! Owns open state, the staged draft both tabs edit, the two blacklist editors, and the Cancel All
//! confirmation. `crate::shell::core_settings_popup` renders the content.
//!
//! Everything below the tab strip commits on OK through [`Shell::commit_core_draft`]; the actions
//! ABOVE it (restart, emulator, cancel all) act immediately, because none of them is a value to be
//! reviewed before committing.

pub(crate) mod draft;
pub(crate) mod editors;

use std::time::Instant;

use gpui::*;

use moon_ui::{MoonButton, MoonPalette};

use moon_core::session::CoreId;

use rust_i18n::t;

use crate::shell::core_settings_popup::{self, CoreSettingsTab};

use super::Shell;

/// A request to open one core's settings on its blacklist and highlight `coin`.
///
/// Chart filter lines and the places dialog both queue this on `Backend`. Nothing reads it until
/// the shell drains it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoreSettingsReveal {
    /// Core whose settings should open.
    pub core: CoreId,
    /// Coin token to highlight in that core's blacklist.
    pub coin: String,
}

/// Where a blacklist reveal is shown. There is no arm that changes the active trading core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GotoAction {
    /// The compact gear popup, only when it can write this core.
    CompactPopup,
    /// The application-wide expert window, bound to the requested core.
    ExpertPage,
}

/// Coin highlighted on a blacklist, and when its amber outline started.
#[derive(Clone, Debug)]
pub(crate) struct CoinHighlight {
    /// Token painted amber and named on the remove button.
    pub(crate) coin: String,
    /// When the outline started. Alpha is derived from this.
    pub(crate) started: Instant,
    /// Refusal text beside the remove button. `None` after a clean frame.
    pub(crate) error: Option<String>,
}

/// What one frame of a blacklist reveal draws. The coin is borrowed from [`CoinHighlight`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlacklistReveal<'a> {
    /// Token painted amber.
    pub(crate) coin: &'a str,
    /// Outline strength, 1 down to 0.
    pub(crate) alpha: f32,
    /// Refusal text, when the last remove failed.
    pub(crate) error: Option<&'a str>,
}

/// How long the amber blacklist outline takes to fade.
/// Choose the compact popup or the expert page for one blacklist reveal.
///
/// The compact popup writes only through [`resolve_core_settings_write`], which requires the
/// seeded core to be the active one, so it cannot host any other core. Expert mode and Auto
/// Overview do not draw the popup at all.
///
/// Args:
///     target: Core the reveal named.
///     active: This shell's active trading core, if it has one.
///     popup_available: Whether this shell still draws the gear as a popup.
///
/// Returns:
///     [`GotoAction::CompactPopup`] only when the popup is available and `target` is active.
pub(crate) fn core_settings_goto_action(
    target: CoreId,
    active: Option<CoreId>,
    popup_available: bool,
) -> GotoAction {
    if popup_available && active == Some(target) {
        GotoAction::CompactPopup
    } else {
        GotoAction::ExpertPage
    }
}

/// Whether an open shell's group currently holds `core`.
///
/// Args:
///     backend: Application state with the group-window tables and the live sessions.
///     core: Core the reveal named.
///
/// Returns:
///     `true` when some open or opening group window owns `core`.
fn shell_group_owns_core(backend: &crate::Backend, core: CoreId) -> bool {
    backend
        .group_windows
        .keys()
        .chain(backend.opening_group_windows.iter())
        .any(|group| backend.core_belongs_to_group(group, core))
}

/// Amber-outlined «Убрать <COIN>» button.
///
/// Args:
///     id: Element id, unique on the surface that draws it.
///     coin: Token the label names.
///     on_click: Removal. The caller writes the seeded core and updates its own draft.
///
/// Returns:
///     The button.
pub(crate) fn coin_blacklist_remove_button(
    id: impl Into<ElementId>,
    coin: &str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    // MoonButton's handler also receives the click. Callers of this helper only need the window
    // and the app, which is what a removal writes through.
    MoonButton::new(id)
        .outline()
        .label(t!("coin_bl_places.remove_coin", coin = coin).to_string())
        .size(crate::design::CONTROL_TIER)
        .on_click(move |_event, window, app| on_click(window, app))
        .into_any_element()
}

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

    /// Whether the header still draws the gear as a `MoonPopover` at all.
    ///
    /// Two conditions take the popover away, and everything that has to react to one has to react
    /// to the other: expert mode replaces it with a plain button that opens
    /// [`crate::core_expert`]'s window, and Auto Overview drops the whole per-core cluster. An open
    /// popup then has nothing rendering it and no `on_open_change` to dismiss it, `Shell::render`
    /// keeps rebuilding two dozen editors for it every repaint, and focus is left on a handle
    /// nothing draws.
    ///
    /// Named once here so a third face of the gear — or a third reason to hide it — is one edit
    /// rather than four.
    pub(crate) fn gear_popup_unavailable(&self, cx: &App) -> bool {
        let b = self.backend.read(cx);
        b.core_settings_expert() || b.is_auto_overview_scope(&self.group)
    }

    /// Drop the gear popup once it no longer belongs to the core it was seeded from — or once the
    /// group has no single core for it to belong to at all.
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
        // Ahead of the active-core resolution below, and deliberately: `active_trade_core` falls
        // through to the group's FIRST core in Overview and would happily keep the popup alive over
        // an arbitrary server. Expert mode is the other half of the same predicate — and the half
        // that reaches the OTHER group's window, which its own gear never can, because the
        // preference is application-wide.
        if self.gear_popup_unavailable(cx) {
            if self.core_settings_draft.is_some() {
                log::info!(
                    "core settings popup closed with unsaved edits: the gear stopped drawing a popover"
                );
            }
            self.close_core_settings_popup();
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
        self.core_settings_editors.clear();
        self.core_settings_bl_highlight = None;
        self.core_settings_bl_expanded = false;
    }

    /// Close the compact popup because the gear now opens the expert window instead.
    ///
    /// The staged draft is DISCARDED, exactly as dismissing the popover discards it: ticking
    /// "expert mode" is not a confirmation of the values on screen, and the expert window seeds its
    /// own page from the core.
    ///
    /// The blur is not optional, for the reason `Shell::render` states about entering Overview: the
    /// gear stops being a `MoonPopover` in the SAME frame this flag clears, so the popover never
    /// renders again and never runs its own open-to-closed focus hand-back. Focus would sit on a
    /// handle nothing draws and every hotkey in the window would be dead until the next click.
    pub(crate) fn leave_popup_for_expert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.core_settings_open;
        self.close_core_settings_popup();
        if had_focus {
            window.blur();
        }
        cx.notify();
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

    /// Take `Backend.core_settings_goto` when this shell should show it.
    ///
    /// The shell whose group owns the core consumes it. When no open shell's group owns the core,
    /// the first shell to render consumes it and opens the expert page, which is application-wide.
    /// A shell that does not own the core leaves the request for the shell that does.
    ///
    /// Args:
    ///     window: This shell's window, used to seed the popup and to place the expert window.
    ///     cx: View context.
    pub(super) fn drain_core_settings_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(peek) = self.backend.read(cx).core_settings_goto.clone() else {
            return;
        };
        let consume = {
            let backend = self.backend.read(cx);
            let mine = backend.core_belongs_to_group(&self.group, peek.core);
            mine || !shell_group_owns_core(backend, peek.core)
        };
        if !consume {
            return;
        }
        let Some(reveal) = self
            .backend
            .update(cx, |backend, _| backend.core_settings_goto.take())
        else {
            return;
        };
        let active = self.backend.read(cx).active_trade_core(&self.group);
        let popup_available = !self.gear_popup_unavailable(cx);
        let mine = self
            .backend
            .read(cx)
            .core_belongs_to_group(&self.group, reveal.core);
        let action = if mine {
            core_settings_goto_action(reveal.core, active, popup_available)
        } else {
            GotoAction::ExpertPage
        };
        match action {
            GotoAction::CompactPopup => {
                let already = self.core_settings_open;
                self.set_core_settings_open(true, window, cx);
                if already {
                    self.stage_blacklist_text(cx);
                }
                self.set_core_settings_tab(CoreSettingsTab::General, cx);
                self.core_settings_bl_expanded = true;
                let text = self
                    .core_settings_draft
                    .as_ref()
                    .map(|draft| draft.general.blacklist_text.clone());
                self.blacklist_area.update(cx, |state, context| {
                    if let Some(text) = text {
                        state.set_value(text, window, context);
                    }
                });
                // Focus after this render. The drain runs from `Shell::render`, and focusing
                // the area in that pass fights the frame that is opening the popover.
                let area = self.blacklist_area.clone();
                let handle = window.window_handle();
                cx.defer(move |app| {
                    let _ = handle.update(app, move |_, window, app| {
                        area.update(app, |state, cx| state.focus(window, cx));
                    });
                });
                self.core_settings_bl_highlight = Some(CoinHighlight {
                    coin: reveal.coin,
                    started: Instant::now(),
                    error: None,
                });
                cx.notify();
            }
            GotoAction::ExpertPage => {
                let owner_display = crate::window::windowing::window_display_id(window, cx);
                let backend = self.backend.clone();
                let owner = window.window_handle();
                // Open after this render. The drain runs from `Shell::render`, and a window opened
                // mid-render draws at once and clears the element arena this frame still holds.
                cx.defer(move |app| {
                    crate::core_expert::open_core_expert_on(
                        backend,
                        Some(owner),
                        owner_display,
                        reveal.core,
                        reveal.coin,
                        app,
                    );
                });
            }
        }
    }

    /// Lift the highlighted coin from the seeded core and from the open draft.
    ///
    /// `Ok` including `Ok(false)` rewrites the draft with [`moon_core::symbol::coin_list::remove`]
    /// and both blacklist editors, then clears the highlight. Other staged fields stay. `Err`
    /// keeps the draft, the editors, and the highlight, and shows the failure beside the button.
    /// No draft is created when none exists.
    ///
    /// Args:
    ///     window: Window the editors need in order to take the rewritten text.
    ///     cx: View context for the write and the repaint.
    pub(super) fn remove_highlighted_core_coin(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(highlight) = self.core_settings_bl_highlight.clone() else {
            return;
        };
        let Some(core) = resolve_core_settings_write(
            self.core_settings_target,
            self.backend.read(cx).active_trade_core(&self.group),
        ) else {
            return;
        };
        let coin = highlight.coin;
        let outcome = self.backend.update(cx, |backend, _| {
            crate::controls::coin_menu::write_core_blacklist(backend, core, &coin, true)
        });
        match outcome {
            Ok(_) => {
                if let Some(seed) = self.core_settings_seed.as_mut() {
                    seed.general.blacklist_text =
                        moon_core::symbol::coin_list::remove(&seed.general.blacklist_text, &coin);
                }
                if self.core_settings_draft.is_some() {
                    let current = self
                        .core_settings_draft
                        .as_ref()
                        .map(|draft| draft.general.blacklist_text.clone())
                        .unwrap_or_default();
                    let next = moon_core::symbol::coin_list::remove(&current, &coin);
                    self.edit_core_draft(|draft| draft.general.blacklist_text = next.clone(), cx);
                    let area = next.clone();
                    self.blacklist_input
                        .update(cx, |state, context| state.set_value(next, window, context));
                    self.blacklist_area
                        .update(cx, |state, context| state.set_value(area, window, context));
                }
                self.core_settings_bl_highlight = None;
                cx.notify();
            }
            Err(_) => {
                if let Some(item) = self.core_settings_bl_highlight.as_mut() {
                    item.error = Some(t!("coin_bl_places.remove_failed", coin = coin).to_string());
                }
                cx.notify();
            }
        }
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
        if self
            .core_settings_bl_highlight
            .as_ref()
            .is_some_and(|item| crate::controls::coin_token::flash_alpha(item.started) > 0.0)
        {
            window.request_animation_frame();
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
        // Built here, not inside the content builder: the strip renders through a lifted palette,
        // which needs `window` and `&mut cx` that `core_settings_content` does not hold.
        let tab_strip = core_settings_popup::core_settings_tab_strip(
            self.core_settings_tab,
            p,
            &view,
            window,
            cx,
        );
        let reveal = self
            .core_settings_bl_highlight
            .as_ref()
            .map(|item| BlacklistReveal {
                coin: item.coin.as_str(),
                alpha: crate::controls::coin_token::flash_alpha(item.started),
                error: item.error.as_deref(),
            });
        core_settings_popup::core_settings_content(
            &ctx,
            self.core_settings_tab,
            self.core_settings_draft.as_ref(),
            &widgets,
            &editors,
            self.core_settings_bl_expanded,
            self.core_settings_cancel_confirm,
            reveal,
            &view,
            tab_strip,
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
