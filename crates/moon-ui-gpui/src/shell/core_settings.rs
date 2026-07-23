//! Hosts the core-settings popover opened by the gear button beside the header core selector.
//! Owns open state, seeds persistent editors from the active core, synchronizes blacklist editors,
//! and tracks Cancel All confirmation. `crate::shell::core_settings_popup` renders the content.

use gpui::*;

use moon_ui::MoonPalette;

use crate::shell::core_settings_popup;

use super::Shell;

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
            self.seed_core_settings_popup(window, cx);
        }
        cx.notify();
    }

    /// Seed Global TP, Trailing, V-Stop, and blacklist editors from the active core snapshot.
    /// Slider values are clamped to their supported ranges; disabled zero-valued stops preserve
    /// their last displayed numeric controls rather than being clamped to a nonzero minimum.
    fn seed_core_settings_popup(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (gtp, trailing, vstop, bl_text) = {
            let b = self.backend.read(cx);
            let cs = b
                .active_trade_core(&self.group)
                .and_then(|c| b.session.store().core(c))
                .and_then(|d| d.client_settings.as_ref());
            match cs {
                Some(s) => (
                    s.global_take_profit_pct as f32,
                    s.trailing_drop_pct,
                    s.vol_drop_level,
                    s.blacklist_text.clone(),
                ),
                None => return,
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
    }

    /// Commit blacklist text to the currently active core while preserving its current enabled flag.
    /// Shared by Blur/Enter subscriptions for both editors and by the expansion toggle.
    pub(super) fn commit_blacklist_text(&self, text: String, cx: &Context<Self>) {
        let b = self.backend.read(cx);
        let Some(core) = b.active_trade_core(&self.group) else {
            return;
        };
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

    /// Require one confirmation click before cancelling all orders for the active core.
    pub(super) fn core_settings_cancel_all_click(&mut self, cx: &mut Context<Self>) {
        if !self.core_settings_cancel_confirm {
            self.core_settings_cancel_confirm = true;
            cx.notify();
            return;
        }
        self.core_settings_cancel_confirm = false;
        let b = self.backend.read(cx);
        if let Some(core) = b.active_trade_core(&self.group) {
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
