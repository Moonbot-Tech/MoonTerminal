//! Hosts the core-settings popover opened by the gear button beside the header core selector.
//! Owns open state, seeds persistent editors from the active core, synchronizes blacklist editors,
//! and tracks Cancel All confirmation. `crate::shell::core_settings_popup` renders the content.

use gpui::*;

use moon_ui::MoonPalette;

use moon_core::session::CoreId;

use crate::shell::core_settings_popup;

use super::Shell;

/// The core a core-settings write may address, or `None` when it must be dropped.
///
/// The gear popup's editors are SEEDED from one core when it opens and then live on unchanged,
/// while the active trading core can move underneath them — the header selector, a Main-chart coin
/// switch through [`crate::Backend::active_trade_core`]'s fallback, or a core going away entirely.
/// Resolving the core again at event time would write the value the user is looking at into a core
/// they are not looking at.
///
/// [`Shell::reconcile_core_settings_popup`] takes such a popup off the screen, but only at the next
/// render, and repaints pass three stacked throttles — a slider drag lands inside that window. So
/// the check has to happen HERE too, at event time, exactly as [`crate::controls::MetricTarget`]
/// does for the toolbar metric popups.
///
/// A `None` seed means the popup belongs to no core — the group had no active core when it opened.
/// It is deliberately NOT treated as "any core will do": there is nothing on screen to commit.
/// (A core that merely has no `ClientSettings` snapshot yet is still a real seed — see
/// [`Shell::seed_core_settings_popup`].)
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
    /// Opening collapses the blacklist editor, seeds persistent fields from the active core, and
    /// resets Cancel All confirmation; closing only resets confirmation and open state.
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
            // Recorded from the same read that seeds the editors, so the address and the displayed
            // values can never come from different cores.
            self.core_settings_target = self.seed_core_settings_popup(window, cx);
        } else {
            self.core_settings_target = None;
        }
        cx.notify();
    }

    /// Drop the gear popup once it no longer belongs to the core it was seeded from.
    ///
    /// The active trading core moves without producing an event to hang this on — the header
    /// selector, a Main-chart coin switch, a core being disabled — so it is reconciled at render
    /// like the state the panels poll, mirroring [`Shell::reconcile_metric_popup`].
    ///
    /// Closing rather than re-seeding is deliberate: re-seeding would swap the numbers under a user
    /// who is mid-edit, and the value they were about to commit would silently become another
    /// core's. The event-time guard in [`resolve_core_settings_write`] covers the throttle window
    /// before this render lands.
    ///
    /// The blacklist editor is flushed to the core it was typed against first. A normal close
    /// commits it through the editor's Blur, which fires AFTER the target is cleared and would then
    /// be refused — so without this the user's typing is silently discarded, and a core switch they
    /// did not cause (an idle Main chart being pruned) is enough to trigger it.
    pub(super) fn reconcile_core_settings_popup(&mut self, cx: &App) {
        if !self.core_settings_open {
            return;
        }
        let active = self.backend.read(cx).active_trade_core(&self.group);
        let Some(seeded) = self.core_settings_target else {
            self.close_core_settings_popup();
            return;
        };
        if resolve_core_settings_write(Some(seeded), active).is_none() {
            let editor = if self.core_settings_bl_expanded {
                &self.blacklist_area
            } else {
                &self.blacklist_input
            };
            let text = editor.read(cx).value().to_string();
            self.send_blacklist_text(seeded, text, cx);
            self.close_core_settings_popup();
        }
    }

    /// Clear every piece of gear-popup state in one place, so no path can close it halfway.
    fn close_core_settings_popup(&mut self) {
        self.core_settings_open = false;
        self.core_settings_target = None;
        self.core_settings_cancel_confirm = false;
    }

    /// Seed Global TP, Trailing, V-Stop, and blacklist editors from the active core snapshot.
    /// Slider values are clamped to their supported ranges; disabled zero-valued stops preserve
    /// their last displayed numeric controls rather than being clamped to a nonzero minimum.
    ///
    /// Returns the core the popup now belongs to, which becomes its write address — the ACTIVE core,
    /// whether or not it had a snapshot to seed from. A core still waiting for its first
    /// `ClientSettings` renders the placeholder state and its own runtime dots, and that popup is a
    /// live popup over a real core; only a group with no active core at all yields `None`.
    ///
    /// Note the contrast with [`crate::controls::MetricTarget`], whose `None` core means "not
    /// core-scoped, always live". Here `None` means the opposite — there is no core to write to.
    fn seed_core_settings_popup(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<CoreId> {
        let (core, gtp, trailing, vstop, bl_text) = {
            let b = self.backend.read(cx);
            let core = b.active_trade_core(&self.group)?;
            let cs = b
                .session
                .store()
                .core(core)
                .and_then(|d| d.client_settings.as_ref());
            match cs {
                Some(s) => (
                    core,
                    s.global_take_profit_pct as f32,
                    s.trailing_drop_pct,
                    s.vol_drop_level,
                    s.blacklist_text.clone(),
                ),
                // Nothing to seed yet. The popup still belongs to this core: it renders the
                // placeholder, and a write it does make is aimed at the core on screen.
                None => return Some(core),
            }
        };
        let clamp = |v: f32, (lo, hi, _): (f32, f32, f32)| v.clamp(lo, hi);
        // Trailing and V-Stop have no separate on/off flag on the wire: zero means disabled, and the
        // core does not retain the previous value. Delphi Moonbot remembers it only in its own UI as
        // `TrailingDropOld`. Do not overwrite these controls when the snapshot contains zero; retain
        // the last displayed value. An initially empty field truthfully represents the lack of a
        // core value, so do not invent a default.
        self.gtp_slider.update(cx, |st, c| {
            st.set_value(clamp(gtp, core_settings_popup::CORE_GTP_BOUNDS), window, c)
        });
        self.gtp_input
            .update(cx, |st, c| st.set_value(format!("{gtp:.1}"), window, c));
        if trailing.abs() > 1e-6 {
            self.trailing_slider.update(cx, |st, c| {
                st.set_value(
                    clamp(trailing, core_settings_popup::CORE_TRAILING_BOUNDS),
                    window,
                    c,
                )
            });
            self.trailing_input.update(cx, |st, c| {
                st.set_value(format!("{trailing:.2}"), window, c)
            });
        }
        if vstop != 0 {
            self.vstop_slider.update(cx, |st, c| {
                st.set_value(
                    clamp(vstop as f32, core_settings_popup::CORE_VSTOP_BOUNDS),
                    window,
                    c,
                )
            });
            self.vstop_input
                .update(cx, |st, c| st.set_value(format!("{vstop}"), window, c));
        }
        self.blacklist_input
            .update(cx, |st, c| st.set_value(bl_text.clone(), window, c));
        self.blacklist_area
            .update(cx, |st, c| st.set_value(bl_text, window, c));
        Some(core)
    }

    /// Commit blacklist text to the core the popup was seeded from, preserving its enabled flag.
    /// Shared by Blur/Enter subscriptions for both editors and by the expansion toggle.
    ///
    /// The text being committed was seeded from that core, so the write is refused outright once
    /// the active core has moved — see [`resolve_core_settings_write`].
    pub(super) fn commit_blacklist_text(&self, text: String, cx: &Context<Self>) {
        let active = self.backend.read(cx).active_trade_core(&self.group);
        let Some(core) = resolve_core_settings_write(self.core_settings_target, active) else {
            return;
        };
        self.send_blacklist_text(core, text, cx);
    }

    /// Send blacklist text to an already-resolved core, preserving that core's enabled flag.
    ///
    /// Split from [`Self::commit_blacklist_text`] for the one caller that has a core the guard
    /// would reject: [`Self::reconcile_core_settings_popup`] flushes to the core the text was typed
    /// against, which is by then no longer the active one.
    fn send_blacklist_text(&self, core: CoreId, text: String, cx: &App) {
        let b = self.backend.read(cx);
        let on = b
            .session
            .store()
            .core(core)
            .and_then(|d| d.client_settings.as_ref())
            .map(|s| s.use_blacklist)
            .unwrap_or(false);
        if let Err(error) = b.session.set_blacklist(core, on, text) {
            log::warn!("set blacklist text failed: {error:#}");
        }
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
        {
            if let Err(error) = b.session.cancel_all_orders(core) {
                log::warn!("cancel all orders failed: {error:#}");
            }
        }
        cx.notify();
    }

    /// Build content for the gear button's anchored `MoonPopover` while it is open.
    /// The popover owns trigger-relative positioning, avoiding the stale absolute coordinates used
    /// before the ticker was added to the header.
    pub(super) fn core_settings_popup_content(
        &self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        let toggle_view = view.clone();
        let content = core_settings_popup::core_settings_content(
            &self.gtp_slider,
            &self.trailing_slider,
            &self.vstop_slider,
            &self.gtp_input,
            &self.trailing_input,
            &self.vstop_input,
            &self.blacklist_input,
            &self.blacklist_area,
            &self.def_strategy_input,
            self.core_settings_bl_expanded,
            self.core_settings_cancel_confirm,
            self.core_settings_target,
            &self.backend,
            &self.group,
            p,
            cx,
            move |app| view.update(app, |this, cx| this.core_settings_cancel_all_click(cx)),
            move |window, app| {
                toggle_view.update(app, |this, cx| {
                    let expanding = !this.core_settings_bl_expanded;
                    this.core_settings_bl_expanded = expanding;
                    // Synchronize text between distinct single-line and multiline states. Reusing
                    // one state is intentionally avoided because textarea setup mutates it irreversibly.
                    if expanding {
                        let text = this.blacklist_input.read(cx).value().to_string();
                        this.blacklist_area
                            .update(cx, |st, c| st.set_value(text, window, c));
                    } else {
                        let text = this.blacklist_area.read(cx).value().to_string();
                        this.blacklist_input
                            .update(cx, |st, c| st.set_value(text.clone(), window, c));
                        // Collapsing finishes the edit explicitly because replacing the textarea
                        // element may prevent its Blur event from arriving.
                        this.commit_blacklist_text(text, cx);
                    }
                    cx.notify();
                })
            },
        );
        content
    }
}

#[cfg(test)]
mod tests;
