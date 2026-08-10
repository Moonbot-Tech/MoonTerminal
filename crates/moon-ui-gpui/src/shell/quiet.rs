//! Shell-side ownership of the header's quiet-mode ("sleep") settings popup: open state, seeding
//! the three text editors from the persisted configuration, and committing what was typed.
//!
//! [`super::quiet_popup`] renders the content; this module owns everything stateful about it, and
//! `chrome::quiet` owns the header toggle and the gear that opens it.
//!
//! Committing happens on Enter/Blur and once more when the popup CLOSES, and both are needed: a
//! click on a checkbox in the same popup blurs nothing, so the close is what catches a time typed
//! and then dismissed with the ✕ or an outside click. Deliberately NOT on Change — every keystroke
//! of an `HH:MM` field is a valid time of its own ("2" is 02:00), so a per-keystroke commit would
//! make half-typed hours the live schedule. Every commit re-parses from scratch and keeps the
//! stored value when the text is not a valid time.

use gpui::*;

use moon_core::config::quiet;

use super::Shell;

impl Shell {
    /// Handle `MoonPopover::on_open_change` for the quiet-mode gear button.
    ///
    /// Args:
    ///     open: Requested open state.
    ///     window: Window owning the editors being seeded.
    ///     cx: Shell context used to repaint.
    ///
    /// Returns:
    ///     Nothing; opening seeds the editors and closing commits them.
    pub(crate) fn set_quiet_settings_open(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quiet_settings_open == open {
            return;
        }
        self.quiet_settings_open = open;
        if open {
            self.seed_quiet_editors(window, cx);
        } else {
            self.commit_quiet_editors(cx);
        }
        cx.notify();
    }

    /// Fill the three editors from the persisted configuration.
    fn seed_quiet_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = self.backend.read(cx).quiet_cfg();
        let seeds = [
            (&self.quiet_from_input, quiet::fmt_hhmm(cfg.from_min_norm())),
            (&self.quiet_to_input, quiet::fmt_hhmm(cfg.to_min_norm())),
            (
                &self.quiet_charts_input,
                quiet::fmt_chart_list(&cfg.chart_bypass),
            ),
        ];
        for (input, text) in seeds {
            input.update(cx, |state, c| state.set_value(text, window, c));
        }
    }

    /// Parse all three editors into the configuration and store what is valid.
    ///
    /// Reading all three every time rather than only the field that fired is deliberate: they are
    /// one configuration, and a per-field write would need its own read-modify-write of a value the
    /// other fields may have changed in the same frame.
    pub(crate) fn commit_quiet_editors(&mut self, cx: &mut Context<Self>) {
        let from = self.quiet_from_input.read(cx).value().to_string();
        let to = self.quiet_to_input.read(cx).value().to_string();
        let charts = self.quiet_charts_input.read(cx).value().to_string();
        self.backend.update(cx, |b, bcx| {
            let mut cfg = b.quiet_cfg();
            // An unparsable time keeps the stored one: the field is edited character by character,
            // and every intermediate state is invalid.
            if let Some(minute) = quiet::parse_hhmm(&from) {
                cfg.from_min = minute;
            }
            if let Some(minute) = quiet::parse_hhmm(&to) {
                cfg.to_min = minute;
            }
            // The chart list has no invalid state — an empty or garbage field simply means "no
            // number bypasses" — so it is stored as parsed.
            cfg.chart_bypass = quiet::parse_chart_list(&charts);
            b.set_quiet_cfg(cfg, bcx);
        });
    }
}
