//! Draft state for the core-settings popup.
//!
//! Both tabs edit ONE draft and commit it under a single OK, the way the Moonbot settings window
//! they reproduce does. Nothing reaches the core while the user types: a write sends the core's
//! whole safe-share configuration, so staging is both what the user expects from an OK/Cancel dialog
//! and what keeps a slider drag off the wire.
//!
//! The draft is seeded from the core's projected configuration when the popup opens (or when that
//! configuration first arrives, if the popup opened before it), and is dropped on Cancel, on OK, and
//! whenever the popup stops belonging to its core.

use std::collections::hash_map::Entry;

use gpui::*;
use moon_ui::{MoonInputEvent, MoonInputState, MoonSliderEvent, MoonSliderState};

use moon_core::feed::CoreConfig;

use crate::shell::Shell;
use crate::shell::core_settings::resolve_core_settings_write;

/// Slider bounds `(minimum, maximum, step)` used by the popup's rows.
///
/// The wire carries plain numbers with no advertised range, so these bound the CONTROL, not the
/// value: nothing clamps what the core is sent beyond what the slider itself can reach.
pub(crate) const ERRORS_LEVEL_BOUNDS: (f32, f32, f32) = (1.0, 50.0, 1.0);
pub(crate) const PING_LEVEL_BOUNDS: (f32, f32, f32) = (100.0, 5000.0, 50.0);
/// Global take profit, as a positive percentage above entry.
pub(crate) const TAKE_PROFIT_BOUNDS: (f32, f32, f32) = (0.5, 100.0, 0.1);
/// Trailing stop, as a negative percentage below the peak.
pub(crate) const TRAILING_BOUNDS: (f32, f32, f32) = (-10.0, -0.1, 0.1);
/// V-Stop, as a negative whole percentage of BID volume.
pub(crate) const VSTOP_BOUNDS: (f32, f32, f32) = (-50.0, 0.0, 1.0);

/// Highest fixed-leverage multiplier any supported venue offers, applied when the page is sent.
const MAX_FIX_LEVERAGE: i32 = 125;

/// Parse a number the way every other numeric field in this terminal does, accepting the decimal
/// comma a Russian keyboard produces.
pub(crate) fn parse_num(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse::<f64>().ok()
}

/// Parse an `HH:MM` work-time boundary into minutes since midnight.
///
/// Returns `None` for anything that is not a valid time, so a half-typed value leaves the draft
/// alone instead of jumping the window to midnight on the way through "2".
pub(crate) fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u16 = h.trim().parse().ok()?;
    let m: u16 = m.trim().parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Format minutes since midnight as `HH:MM`.
pub(crate) fn fmt_hhmm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

impl Shell {
    /// Seed the draft from the core the popup belongs to.
    ///
    /// Returns `false` when that core has no configuration yet, which is the normal state for the
    /// first few seconds after connecting: the runtime is still fetching the full snapshot, and the
    /// tabs render their waiting placeholder until it lands.
    pub(super) fn seed_core_draft(&mut self, cx: &App) -> bool {
        let Some(core) = self.core_settings_target else {
            return false;
        };
        let Some(config) = self
            .backend
            .read(cx)
            .session
            .store()
            .core(core)
            .and_then(|d| d.core_config.clone())
        else {
            return false;
        };
        self.core_settings_seed = Some(config.clone());
        self.core_settings_draft = Some(config);
        // A new generation is what tells the retained editors to take the core's values back; see
        // [`Self::core_settings_input`].
        self.core_settings_seed_gen = self.core_settings_seed_gen.wrapping_add(1);
        true
    }

    /// Apply one staged change to the draft, if a draft exists.
    ///
    /// A change arriving without a draft is dropped rather than creating one: a draft built from a
    /// single edited field would send defaults for every other field the projection covers.
    pub(crate) fn edit_core_draft(
        &mut self,
        apply: impl FnOnce(&mut CoreConfig),
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.core_settings_draft.as_mut() else {
            return;
        };
        let before = draft.clone();
        apply(draft);
        if *draft != before {
            cx.notify();
        }
    }

    /// Send the draft to the core the popup belongs to and close the popup, as Moonbot's OK does.
    ///
    /// Refused outright when the active core has moved since the popup opened — the values on
    /// screen describe the seeded core, and writing them into another one is exactly the confusion
    /// [`resolve_core_settings_write`] exists to prevent.
    pub(crate) fn commit_core_draft(&mut self, cx: &mut Context<Self>) {
        // Clicking a button does not blur a MoonInput in this codebase, so the blacklist editor's
        // Blur has NOT fired by the time OK runs: without this the popup would commit the text the
        // field held when it was last blurred and silently discard what the user just typed.
        self.stage_blacklist_text(cx);
        let Some(mut draft) = self.core_settings_draft.clone() else {
            return;
        };
        // One clamp for the whole page, here rather than per keystroke: the exchange refuses a
        // non-positive multiplier and no supported venue offers more than 125x, but clamping while
        // the user types would make the field disagree with what OK sends.
        draft.leverage.fix_lev = draft.leverage.fix_lev.clamp(1, MAX_FIX_LEVERAGE);
        let b = self.backend.read(cx);
        let active = b.active_trade_core(&self.group);
        let Some(core) = resolve_core_settings_write(self.core_settings_target, active) else {
            // Silence here would be indistinguishable from a successful save: the popup closes
            // either way, and the user pressed OK expecting the values on screen to be applied.
            log::warn!("core settings OK ignored: the active core moved since the popup opened");
            self.close_core_settings_popup();
            cx.notify();
            return;
        };
        if let Err(error) = b.session.edit_core_config(core, draft.clone()) {
            log::warn!("core config edit failed: {error:#}");
        }
        // The blacklist-delta filter has a client-side half that moonproto applies to its own
        // retained analytics: the core's copy alone would leave this terminal's deltas unchanged
        // until a restart. Nothing is cached for it here — the checkbox reads the core's own value
        // out of the draft.
        let exclude = draft.general.exclude_blacklisted_from_deltas;
        if let Err(error) = b.session.set_exclude_blacklisted_delta(core, exclude) {
            log::warn!("exclude delta failed: {error:#}");
        }
        self.close_core_settings_popup();
        cx.notify();
    }

    /// Re-seed an UNTOUCHED popup from the core's newest configuration.
    ///
    /// A write sends the whole projection, so a popup left open while a coin is blacklisted from
    /// the context menu (or a toolbar control moves) would put the old page back on OK. Re-seeding
    /// is safe only while the draft still equals what it was seeded with: once the user has changed
    /// anything, their edits outrank the core's newer values, and overwriting them mid-edit is the
    /// worse failure.
    ///
    /// Returns whether the draft was replaced, so the caller re-seeds the text editors bound to it
    /// — and ONLY then: writing them on every render would move the caret while the user types.
    pub(super) fn refresh_untouched_core_draft(&mut self, cx: &App) -> bool {
        let Some(core) = self.core_settings_target else {
            return false;
        };
        let untouched = match (&self.core_settings_draft, &self.core_settings_seed) {
            (Some(draft), Some(seed)) => draft == seed,
            _ => return false,
        };
        if !untouched {
            return false;
        }
        let latest = self
            .backend
            .read(cx)
            .session
            .store()
            .core(core)
            .and_then(|d| d.core_config.clone());
        let Some(latest) = latest else {
            return false;
        };
        if self.core_settings_seed.as_ref() == Some(&latest) {
            return false;
        }
        self.core_settings_seed = Some(latest.clone());
        self.core_settings_draft = Some(latest);
        self.core_settings_seed_gen = self.core_settings_seed_gen.wrapping_add(1);
        true
    }

    /// Discard the draft and close the popup, as Moonbot's Cancel does.
    pub(crate) fn cancel_core_draft(&mut self, cx: &mut Context<Self>) {
        self.close_core_settings_popup();
        cx.notify();
    }

    /// Retained editor for one numeric or text field, created on first render of that field.
    ///
    /// An existing editor is written to ONLY when the draft has been re-seeded since it last saw it
    /// — Cancel, a core switch, or the core's first configuration arriving. It deliberately does not
    /// follow the draft the way `strategies::StrategiesView::field_input_state` follows its staged
    /// value: there the staged text IS what the user typed, so the round trip is an identity, while
    /// here the draft holds a PARSED value that formats back differently. Re-synchronizing from it
    /// on every repaint would rewrite "5.5" as "5.50" mid-word, refill a field the user just
    /// cleared, and — since `sync_value` collapses the selection to the end — move the caret while
    /// they type.
    pub(crate) fn core_settings_input(
        &mut self,
        id: &'static str,
        value: String,
        stage: fn(&mut CoreConfig, &str),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let seed_gen = self.core_settings_seed_gen;
        if let Some((seen, state)) = self.core_settings_inputs.get_mut(id) {
            let state = state.clone();
            let stale = *seen != seed_gen;
            *seen = seed_gen;
            if stale && state.read(cx).value() != value {
                state.update(cx, |s, c| s.sync_value(value, c));
            }
            return state;
        }
        let state = cx.new(|c| MoonInputState::new(window, c).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let text = state.read(cx).value().to_string();
                this.edit_core_draft(|draft| stage(draft, &text), cx);
            }
        })
        .detach();
        self.core_settings_inputs
            .insert(id, (seed_gen, state.clone()));
        state
    }

    /// Retained slider for one numeric field, created on first render of that row.
    ///
    /// Unlike the text editors, sliders DO follow the draft on every render (the caller skips it
    /// mid-drag), because a slider has no partially typed state to protect and its thumb would
    /// otherwise ignore a value changed elsewhere — by Cancel, by a re-seed, or by the field beside
    /// it. `MoonSliderState::set_value` emits no Change, so this cannot loop back through the
    /// staging subscription below.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn core_settings_slider(
        &mut self,
        id: &'static str,
        bounds: (f32, f32, f32),
        value: f32,
        stage: fn(&mut CoreConfig, f32),
        mirror: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonSliderState> {
        let (min, max, step) = bounds;
        let value = value.clamp(min, max);
        match self.core_settings_sliders.entry(id) {
            Entry::Occupied(slot) => {
                let state = slot.get().clone();
                if !cx.has_active_drag() && state.read(cx).value().end() != value {
                    state.update(cx, |s, c| s.set_value(value, window, c));
                }
                state
            }
            Entry::Vacant(slot) => {
                let state = cx.new(|_| {
                    MoonSliderState::new()
                        .min(min)
                        .max(max)
                        .step(step)
                        .default_value(value)
                });
                cx.subscribe(&state, move |this, _state, ev: &MoonSliderEvent, cx| {
                    if let MoonSliderEvent::Change(v) = ev {
                        let v = v.end();
                        this.edit_core_draft(|draft| stage(draft, v), cx);
                        // A row that also shows the value in an editor writes it there directly:
                        // the editor only re-reads the draft on a re-seed (so typing survives), and
                        // without this the number would contradict the thumb the user is dragging.
                        // Every such pair in this popup is a whole count — an error level, a ping in
                        // milliseconds — so a rounded integer is the whole formatting rule.
                        if let Some(field) = mirror
                            .and_then(|m| this.core_settings_inputs.get(m))
                            .map(|(_, state)| state.clone())
                        {
                            this.live_set_field(field, format!("{}", v.round() as i64), cx);
                        }
                    }
                })
                .detach();
                slot.insert(state.clone());
                state
            }
        }
    }
}
