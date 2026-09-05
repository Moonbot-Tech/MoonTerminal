//! Staged core-settings page: the popup's draft, and the send both faces of the gear share.
//!
//! Both tabs edit ONE draft and commit it under a single OK, the way the Moonbot settings window
//! they reproduce does. Nothing reaches the core while the user types: a write sends the core's
//! whole safe-share configuration, so staging is both what the user expects from an OK/Cancel dialog
//! and what keeps a slider drag off the wire.
//!
//! The draft is seeded from the core's projected configuration when the popup opens (or when that
//! configuration first arrives, if the popup opened before it), and is dropped on Cancel, on OK, and
//! whenever the popup stops belonging to its core.

use gpui::*;
use moon_ui::{MoonInputState, MoonSliderState};

use moon_core::feed::{CoreConfig, FieldMask};

use moon_core::session::CoreId;

use crate::Backend;
use crate::shell::Shell;
use crate::shell::core_settings::{editors, resolve_core_settings_write};

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
///
/// A non-finite value is REFUSED, not passed on: Rust parses "nan" and "inf" happily, and these
/// fields are thresholds the core compares against — a NaN one compares false to everything, which
/// turns a panic-sell or a watchdog off while the checkbox beside it still reads as on. There is no
/// second finiteness check between here and the wire.
pub(crate) fn parse_num(s: &str) -> Option<f64> {
    s.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        // Canonical zero. "-0" parses to `-0.0`, which the projection's `total_cmp` equality orders
        // BELOW `0.0` — so a core echoing a plain zero would never match the draft, and every OK on
        // that page would burn its retry budget. One place, rather than a special case in each of
        // the five hand-written comparisons downstream.
        .map(|v| if v == 0.0 { 0.0 } else { v })
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
        self.core_settings_editors.reseeded();
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
        let Some(draft) = self.core_settings_draft.clone() else {
            return;
        };
        if !send_core_config(
            &self.backend,
            &self.group,
            self.core_settings_target,
            draft,
            FieldMask::RENDERED_SECTIONS,
            cx,
        ) {
            // The page reached nothing. Closing anyway is what makes a refused write look like a
            // save, so the popup stays up: pressing OK again once the core is back is the recovery,
            // and dismissing it is still the way out. The expert window answers the same case with
            // a banner, which a popover has no room for.
            cx.notify();
            return;
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
        self.core_settings_editors.reseeded();
        true
    }

    /// Discard the draft and close the popup, as Moonbot's Cancel does.
    pub(crate) fn cancel_core_draft(&mut self, cx: &mut Context<Self>) {
        self.close_core_settings_popup();
        cx.notify();
    }

    /// Retained editor for one row, through the store both faces of the gear share.
    pub(crate) fn core_settings_input(
        &mut self,
        id: &'static str,
        value: String,
        stage: fn(&mut CoreConfig, &str),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        editors::input_state(self, id, value, stage, window, cx)
    }

    /// Retained slider for one row, through the store both faces of the gear share.
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
        editors::slider_state(self, id, bounds, value, stage, mirror, window, cx)
    }
}

impl editors::CoreDraftHost for Shell {
    fn editors(&mut self) -> &mut editors::EditorStore {
        &mut self.core_settings_editors
    }

    fn stage_draft(&mut self, apply: impl FnOnce(&mut CoreConfig), cx: &mut Context<Self>) {
        self.edit_core_draft(apply, cx);
    }

    fn editor_window(&self) -> AnyWindowHandle {
        self.window_handle
    }
}

/// Send one staged page to the core it was seeded from, as Moonbot's OK does.
///
/// Shared by the compact gear popup and by [`crate::core_expert`]'s window: both stage a whole
/// projection of ONE core, and both must refuse to write it into a core that moved underneath
/// them, so the clamp, the [`resolve_core_settings_write`] guard, the section mask and the
/// client-side half of the blacklist-delta filter live here once instead of being re-derived per
/// caller.
///
/// Args:
///     backend: Application state holding the session the page travels through.
///     group: Group whose active trading core the seed is checked against.
///     seeded: Core the page was seeded from; the only core it may reach.
///     draft: Staged page, taken by value because the leverage clamp below rewrites it.
///     cx: Application context used to read the session.
///
/// Returns:
///     Whether the page actually reached the session for the seeded core. A caller that closes on
///     OK must close only on `true`: closing on a refused write is indistinguishable from a save.
pub(crate) fn send_core_config(
    backend: &Entity<Backend>,
    group: &str,
    seeded: Option<CoreId>,
    mut draft: CoreConfig,
    sections: FieldMask,
    cx: &App,
) -> bool {
    // One clamp for the whole page, here rather than per keystroke: the exchange refuses a
    // non-positive multiplier and no supported venue offers more than 125x, but clamping while the
    // user types would make the field disagree with what OK sends.
    //
    // The one value spared is an untouched 0: `fix_lev` defaults to 0 on the wire and 0 is the
    // "none chosen" value gated by `auto_fix_lev`, so clamping THAT would rewrite the core's 0 to 1
    // on every OK — including one pressed on a surface drawing no leverage control at all. Anything
    // a user actually typed is still bounded here, which is the only place it is bounded:
    // `general::field_specs`' editor is deliberately unclamped so mid-typing digits do not fight
    // the field.
    if draft.leverage.auto_fix_lev || draft.leverage.fix_lev != 0 {
        draft.leverage.fix_lev = draft.leverage.fix_lev.clamp(1, MAX_FIX_LEVERAGE);
    }
    let b = backend.read(cx);
    let active = b.active_trade_core(group);
    let Some(core) = resolve_core_settings_write(seeded, active) else {
        // Silence here would be indistinguishable from a successful save: the surface closes either
        // way, and the user pressed OK expecting the values on screen to be applied.
        log::warn!("core settings OK ignored: the active core moved since the page was seeded");
        return false;
    };
    // The mask comes from the CALLER, because what a surface may write is what it DRAWS, and a
    // draft seeded when that surface opened is stale everywhere the user could not see it. The
    // compact popup draws all five rendered sections and names all five; the expert window names
    // only the sections of the PAGES its user actually edited, so its OK cannot write its own
    // frozen copy of a page nobody opened back over a change made elsewhere while it stood open.
    // Neither can name the manual block at all: no mask reachable from here carries it, checkbox on
    // or off.
    // Read before the page is handed over, so the send below can consume it without a clone of the
    // whole projection. Only meaningful when this write names `general`: the value is the surface's
    // own copy of that section, frozen when it was seeded, and applying it from a mask that does not
    // carry the section would set the CLIENT half from a stale number while the core kept the newer
    // one — the two halves of one filter, disagreeing.
    let exclude = sections
        .writes_general()
        .then_some(draft.general.exclude_blacklisted_from_deltas);
    // The same for the trade-derived deltas, under its own area for the same reason: the value is
    // this surface's frozen copy, and a mask that does not carry the area must not set the client
    // half from it.
    let by_trades = sections
        .writes_order_rules()
        .then_some(draft.order_rules.deltas_by_trades);
    if let Err(error) = b.session.edit_core_config(core, draft, sections) {
        // The page never reached the session. Reporting success here is what would let a caller
        // close on it.
        log::warn!("core config edit failed: {error:#}");
        return false;
    }
    // The blacklist-delta filter has a client-side half that moonproto applies to its own retained
    // analytics: the core's copy alone would leave this terminal's deltas unchanged until a
    // restart. Issued only after the page went out, so the two halves cannot diverge the other way
    // — this terminal filtering deltas the core was never told about. Nothing is cached for it here:
    // the checkbox reads the core's own value out of the draft.
    if let Some(exclude) = exclude
        && let Err(error) = b.session.set_exclude_blacklisted_delta(core, exclude)
    {
        log::warn!("exclude delta failed: {error:#}");
    }
    // moonproto keeps its own copy of this one too: the core's alone would leave this terminal's
    // retained short deltas in candle mode until a restart, disagreeing with the core it mirrors.
    if let Some(by_trades) = by_trades
        && let Err(error) = b.session.set_deltas_by_trades(core, by_trades)
    {
        log::warn!("deltas by trades failed: {error:#}");
    }
    true
}
