//! Shared hotkey recognition and backend action execution for group windows and detached chart
//! windows. Key matching previously lived in scattered `pressed()` checks under
//! `Shell::on_hotkey`, leaving some configured bindings inactive and detached chart windows with
//! no handling. The shared flow is now:
//!
//! - [`resolve`] maps a key-down event plus configuration to a semantic [`HotkeyAction`], comparing
//!   only `modifiers` and `key` rather than full `Keystroke` equality;
//! - [`apply`] executes shared backend actions against the caller-supplied active core and chart
//!   market. Actions requiring caller-level context return `false`: scale belongs to the calling
//!   window; cursor placement and cancellation follow the globally tracked hovered chart;
//!   switching and active-chart closing are group-local; reset and close-all are application-global.
//!
//! Configured shortcuts use GPUI's `Keystroke::parse` syntax. Shipped keyboard defaults are
//! Moonbot's own keys, read off its Hotkeys page — mostly `alt-`, with `ctrl-` where Moonbot uses it
//! and for the Terminal's own drawing tools, which Moonbot has no equivalent of. The literal
//! modifier is used on both Windows and macOS, as Moonbot does; bindings that need no modifier keep
//! their bare function-key or Delete forms.

mod layout;
pub mod meta;
#[cfg(test)]
mod tests;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, Keystroke, KeystrokeEvent,
    Modifiers, ModifiersChangedEvent, Window,
};
use moon_core::config::{HotkeysConfig, KeySlot, SHIFT_PERCENT, SPLIT_ORDER_PARTS};
use moon_core::feed::ClientSettingsEdit;
use moon_core::figures::FigureTool;
use moon_core::session::CoreId;
use moon_ui::{MoonHotkeyCapture, MoonHotkeyModifierWatch, MoonInputState};

use crate::Backend;

/// Semantic hotkey action independent of its configured key binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Select one of the current window group's order-size presets.
    OrderSize(usize),
    /// Select one of the current window group's fixed-sell slots, making that preset the effective
    /// take profit instead of the main TP setting.
    SellPreset(usize),
    /// Select the indexed manual strategy in the active core's header picker and enable manual
    /// strategy mode, matching a click on that picker item.
    ManualStrategy(usize),
    /// Cancel pending buy orders for the active chart's market.
    CancelBuy,
    /// Cancel all pending buy orders for the active core across every market.
    CancelAllBuys,
    /// Toggle panic sell for the active chart's market.
    PanicSell,
    /// Immediately close the active chart market's position with a market sell.
    PanicSellOne,
    /// Join sell orders for the active chart's market using its current position side.
    JoinSells,
    /// Shift the active market's orders of one side by a percent, as Moonbot's ±1% does.
    ///
    /// `sell = false` addresses the buy phase, `sell = true` the sell phase; the core selects the
    /// orders by their own status. `up` chooses the sign.
    ShiftOrder {
        /// Address sell-phase orders when true, or buy-phase orders when false.
        sell: bool,
        /// Move up when true, down when false.
        up: bool,
    },
    /// Toggle the Sells-to-zone drawing mode — Moonbot's "sells to rectangle".
    ///
    /// The key draws rather than sends: it arms the Zone tool, and every pair of Ctrl+clicks then
    /// names the prices one band of sells is spread across, after which that band disappears
    /// instead of joining the chart's figures. The mode stays on until this key, Escape, or a tool
    /// pick ends it. Spreading an existing Zone or Rect is the right-click entry on that figure.
    SellsToRect,
    /// Split a sell order for the active chart's market into `parts`.
    ///
    /// The count comes from the binding that matched: plain Split Order always splits into
    /// [`SPLIT_ORDER_PARTS`], as Moonbot does, while `Split N` uses the configured `split_parts`.
    SplitOrder { parts: i32 },
    /// Place a manual long order at the hovered chart price.
    ///
    /// The caller routes this through `Backend::hovered_chart`, because only the chart can map the
    /// cursor's pane-relative Y coordinate to a price.
    NewLong,
    /// Place a manual short order at the hovered chart price through the caller.
    NewShort,
    /// Select or toggle a drawing tool in the global figure state.
    FigTool(FigureTool),
    /// Cycle to the next drawing tool and keep drawing mode enabled.
    SwitchFigure,
    /// Switch the group window's active fullscreen Main chart to the next chart.
    ///
    /// The group-window caller routes this through its revision mechanism; detached chart windows
    /// do not handle it.
    SwitchCharts,
    /// Delete the selected figure.
    FigDelete,
    /// Delete the LAST figure drawn on the chart the pointer rests on — Moonbot's Ctrl+Z.
    ///
    /// Addressed by the POINTER and by nothing else, like the manual-order keys: [`pre_dispatch`]
    /// gives the hovered PANE the key and keeps it there even when that pane has nothing to delete,
    /// and with the pointer on no chart the action goes unhandled rather than falling back to a
    /// chart the user is not looking at — this one deletes, and an armed figure's deletion travels
    /// to the core. Not an undo stack in either terminal: nothing brings the figure back, and a
    /// move or an edit is not what it reverts.
    FigUndo,
    /// Toggle the selected figure's chart alert.
    FigAlert,
    /// Close the group window's active Main chart with the built-in plain Escape binding.
    ///
    /// Matching Moonbot, Escape closes the chart without disabling drawing mode. The group-window
    /// caller executes this action; detached chart windows leave it unhandled.
    CloseActiveChart,
    /// Reset every application window to an on-screen position with built-in Ctrl+Shift+F10.
    ///
    /// The caller executes this because it requires application-wide window access.
    ResetWindows,
    /// Cancel the order under the cursor for the built-in unmodified Tab/Delete binding.
    ///
    /// The caller routes this through `Backend::hovered_chart`, whose `order_hover` state identifies
    /// the order. With no hovered order the action remains unhandled, allowing Tab focus navigation.
    CancelHoveredOrder,
    /// Close every group's Main-stack charts with the built-in Shift+Escape binding.
    ///
    /// The caller increments a global revision observed by every `ChartTabs` instance.
    CloseAllCharts,
    /// Step the active chart's Y scale UP a preset through the calling window — a wider price
    /// band, which is zooming OUT. Moonbot's own reading of "+": see `controls::step_scale`.
    ScalePlus,
    /// Step the active chart's Y scale DOWN a preset — a tighter band, zooming IN.
    ScaleMinus,
    /// Copy an image of the active chart to the system clipboard - Moonbot's "make shot".
    ///
    /// The caller executes this because it needs the OS window behind the chart and the
    /// clipboard, neither of which [`apply`] is handed. See
    /// [`crate::panels::chart::shot`] for which chart is chosen, which rectangle is read and how
    /// it reaches the clipboard.
    ChartShot,
}

/// What the dispatcher actually compares a keystroke by: its modifiers and its key, nothing else.
///
/// The two halves of a configured binding's identity, and the ONE definition of it. Every collision
/// question in the app — the settings page's clash captions, the core pull's "already bound
/// elsewhere" gate — is "do these two configured strings name the same press", and answering it by
/// comparing the STRINGS is wrong in ways nobody spots by reading: `Keystroke::parse` is
/// case-insensitive, accepts the modifiers in any order, and takes `cmd`, `super` and `win` as one
/// modifier. `hotkeys.toml` is a plain file the user may hand-edit or paste into, so `Ctrl-F10` and
/// `shift-ctrl-Z` are spellings that really arrive; and a file written by an older build carries
/// `ctrl-alt-shift-cmd-k` where this one now writes `ctrl-alt-win-shift-k` for the same press. Same
/// press, several strings, and a literal compare calls them free of each other.
///
/// The two producers themselves were the worst of it and no longer disagree:
/// `moonbot_import::shortcut::to_gpui_keystroke` was aligned with `Keystroke::unparse` on
/// 2026-09-10. What remains is every file written before that, and every file written by hand.
pub type BindingId = (Modifiers, String);

/// A configured binding string as a keystroke, or `None` for one that binds nothing.
///
/// The one statement of "unbound" in the app: empty, whitespace, or a spelling the parser rejects.
/// All three mean the same thing to every caller — a slot holding one fires on no press, takes a key
/// from nobody and loses one to nobody — and it used to be written out separately in [`pressed`], in
/// [`binding_id`] and in the settings page.
pub fn parse_binding(raw: &str) -> Option<Keystroke> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Keystroke::parse(raw).ok()
}

/// The identity of a CONFIGURED binding string, or `None` for one that can never fire.
///
/// This is the string-vs-string sibling of [`pressed`], and deliberately does NOT carry its
/// physical-letter fallback: that one exists because a LAYOUT decides what the platform calls the
/// key the user physically struck, and both sides here are spellings their author chose, on no
/// layout in particular.
pub fn binding_id(raw: &str) -> Option<BindingId> {
    let k = parse_binding(raw)?;
    Some((k.modifiers, k.key))
}

/// Whether two configured binding strings name the same press.
///
/// Two strings that cannot fire are not "the same binding": an unbound slot does not collide with
/// another unbound slot, and reporting that would put a clash caption on every empty row.
pub fn same_binding(a: &str, b: &str) -> bool {
    match (binding_id(a), binding_id(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Return whether an event matches a configured GPUI keystroke string.
///
/// Empty or invalid strings do not match. Comparison uses only `modifiers` and `key`: Windows
/// events also carry `key_char`, while a parsed `Keystroke` does not, so full equality previously
/// prevented Ctrl-plus-letter bindings from matching.
///
/// A letter is compared against the PHYSICAL key as well as the name the platform gave it, so a
/// binding does not die when the keyboard layout changes — see [`layout::us_letter`]. That half is
/// why this cannot be [`binding_id`] on both sides: an event's `key` is what the layout produced,
/// not what the author typed into the settings page.
fn pressed(raw: &str, event: &Keystroke) -> bool {
    let Some(k) = parse_binding(raw) else {
        return false;
    };
    k.modifiers == event.modifiers
        && (k.key == event.key
            || layout::us_letter(&event.key).is_some_and(|physical| k.key == physical))
}

/// Give the window root its focus back when nothing at all holds it.
///
/// GPUI dispatches a key event down the path of the FOCUSED node, and with the window blurred
/// `Window::focus_node_id_in_rendered_frame` falls back to the dispatch tree's bare ROOT node — a
/// path carrying no element listeners. Every window root's `on_key_down` is then skipped and EVERY
/// hotkey dies silently, with nothing anywhere to say so, until the user happens to click something
/// focusable. Two paths in the UI stack leave the window in exactly that state and neither hands
/// the focus on: `MoonPopover` blurs when it closes with no previous holder to restore, and GPUI
/// blurs when the focused handle's owner is dropped (`App::release_dropped_focus_handles`).
///
/// Measured 2026-08-26: after an order was cancelled through a popover menu, four consecutive
/// New Long presses logged `window focus=NONE, dispatch depth=0` and did nothing whatever; one
/// click restored both the focus and the hotkey. Repaired HERE rather than in the fork, per the
/// project's rule about editing MoonUI, and from `render` because that is the first thing to run
/// after the frame that dropped the focus.
///
/// Acts only when NOTHING holds focus, so it can never take it from a field being typed in. That
/// bound is also its limit, and worth knowing: a focus handle whose OWNER outlives the element —
/// a popup's input parked in a permanent field of its host — still resolves, so the window reads as
/// focused while the dispatch path has already collapsed, and this cannot tell. GPUI exposes no way
/// to ask whether a focus id is in the rendered frame. Such a case has to be prevented where it is
/// created, by whoever stops rendering a focused element; see
/// [`crate::controls::coin_search::release_focus`].
pub fn restore_root_focus(root: &FocusHandle, window: &mut Window, cx: &mut App) {
    if window.focused(cx).is_none() {
        window.focus(root, cx);
    }
}

/// Take the keyboard off `field`, and only off that field.
///
/// The one primitive behind every "this input is done" in the app: blur when it is the holder, do
/// nothing when it is not. Both halves matter. A field that keeps the focus after the user has
/// visibly moved on eats the editing shortcuts outright — Ctrl+Z is Undo inside a text field and
/// Ctrl+X is Cut, both ordinary keys to bind an action to — and a field that stops being rendered
/// while still focused is worse, because [`restore_root_focus`] cannot repair THAT one: the handle
/// belongs to a permanent member of its host, so it still resolves and the window reads as focused
/// while the dispatch path has already collapsed. The conditional half is what keeps a caller from
/// reaching across the window and emptying the caret out of an unrelated field — some exits are not
/// clicks on the field at all.
///
/// Blur rather than focus something else: each window root re-takes an unheld focus on its next
/// frame, so this states the one thing that is true here and lets the window decide where it goes.
///
/// Args:
///     field: The input being released.
///     window: The window whose focus is being released.
///     cx: Application context used to read the field's handle.
pub fn release_field_focus(field: &Entity<MoonInputState>, window: &mut Window, cx: &App) {
    let held = window
        .focused(cx)
        .is_some_and(|focused| focused == field.read(cx).focus_handle(cx));
    if held {
        window.blur();
    }
}

/// Note a keystroke at the app's interceptor, which sees it whatever the focus is doing.
///
/// The one measurement [`trace_key_arrived`] cannot make. GPUI dispatches a key event down the path
/// of the FOCUSED node; when the focus id no longer resolves to a node in the rendered frame it
/// falls back to the dispatch tree's ROOT node, whose path holds no element listeners — so every
/// window-root handler is skipped and the press leaves no trace anywhere. `context_stack` is the
/// tell: a real path is several contexts deep, a collapsed one is empty.
///
/// The event's own `action` is deliberately NOT reported: `dispatch_keystroke_interceptors` builds
/// the event with `action: None` unconditionally, because interceptors run BEFORE action matching.
/// Logging it would print a constant "nothing claimed this key" next to keys an action then eats.
pub fn trace_key_intercepted(ev: &KeystrokeEvent, window: &Window, cx: &App) {
    log::debug!(
        target: HOTKEYS_TRACE,
        "key {} intercepted: window focus={}, dispatch depth={}",
        ev.keystroke,
        if window.focused(cx).is_some() {
            "set"
        } else {
            "NONE"
        },
        ev.context_stack.len()
    );
}

/// Note that a key reached the window root at all, before anything can consume it.
///
/// Called from every window's CAPTURE-phase listener, which is the only place that sees a press
/// unconditionally: an action binding or a focused field takes the event before the bubble-phase
/// hotkey listener runs, and from there a swallowed key and an unbound one look identical — both
/// are silence. Pairing this line with [`resolve`]'s tells them apart, which is the whole reason
/// the `log.hotkeys` switch exists.
pub fn trace_key_arrived(ev: &KeyDownEvent) {
    log::debug!(
        "key {} reached the window root{}",
        ev.keystroke,
        if ev.is_held { " (auto-repeat)" } else { "" }
    );
}

/// Whether this press is one the focused text field itself consumes.
///
/// The question the typing gate turns on, and it is narrower than "is anything focused". A field
/// only takes a press the platform turned into TEXT, and the character travels its own way there —
/// `WM_CHAR` on Windows, the input context on macOS — so the two never negotiate: resolving a
/// binding for such a press runs the action AND costs the character, because the window then
/// reports the key handled and `translate_accelerator` skips the `TranslateMessage` that would
/// have produced it. Typing a coin name into the search box armed a drawing tool and swallowed the
/// letter; that is the whole bug.
///
/// `key_char` is the test, because it is the platform's own answer to "did this press produce a
/// character" — layout, dead keys and IME included, which no list of key names could track. Tab is
/// named on top of it for Windows, which reports no character for Tab where macOS reports `\t`;
/// both hand it to the form. Enter is the same split and is deliberately left alone: macOS calls it
/// a character, Windows does not, and no shipped binding sits on it either way.
///
/// The modifier cut is the deliberate half. Ctrl, Alt or Cmd usually means no character at all —
/// Windows drops control characters and routes an Alt press as `WM_SYSKEYDOWN`, which reaches no
/// input handler — but not always: macOS reports a character for Option combinations, and Windows
/// does for AltGr, which arrives as control plus alt. Those presses are still left to their
/// bindings, because most of the shipped keymap is `alt-` (`moon_core::config::HotkeysConfig`) and
/// giving Option back to the field would take the trading keys away on the very platform where
/// Option is how you press them. A binding wins there; a user who needs the character rebinds.
///
/// What stays alive mid-word follows from the same test rather than from a list: Escape, the
/// function keys — which is where the order-size and sell-preset defaults live — and every
/// modified binding. None of them takes anything from the field. Caps Lock and lone modifiers
/// never arrive as a press at all and are gated where they are recognised, in
/// [`resolve_modifiers`].
///
/// Split out from [`resolve`] so the rule can be unit-tested without a `Window`.
///
/// Args:
///     keystroke: The press as delivered to the window.
///
/// Returns:
///     Whether the field, rather than a binding, is what this press is for.
fn belongs_to_the_field(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform {
        return false;
    }
    keystroke.key_char.is_some() || keystroke.key == "tab"
}

/// Resolve a key-down event to the action bound to it.
///
/// `typing` withholds every binding the focused field consumes, and it is a parameter for the same
/// reason [`resolve_modifiers`] takes one: the policy belongs to this module, not to each window
/// that calls it. A third window root cannot inherit the resolver and miss the rule — the
/// signature asks for the answer.
///
/// Args:
///     ev: The press as delivered to the window root.
///     hk: The window's effective hotkey configuration.
///     typing: Whether the focused element is taking typed text right now.
///
/// Returns:
///     The bound action, or `None` when nothing is bound or the press is the field's.
pub fn resolve(ev: &KeyDownEvent, hk: &HotkeysConfig, typing: bool) -> Option<HotkeyAction> {
    if typing && belongs_to_the_field(&ev.keystroke) {
        // Its own line, and the reason this is logged rather than silently dropped: `log.hotkeys`
        // already separates a key that reached the root from one that matched no binding, and a
        // suppressed press otherwise looks exactly like an unbound one. The report that comes back
        // is "my hotkey stopped working", with nothing to tell the two apart.
        log::debug!("key {} belongs to the focused text field", ev.keystroke);
        return None;
    }
    let action = resolve_binding(&ev.keystroke, hk);
    match &action {
        Some(action) => log::debug!("key {} resolved to {action:?}", ev.keystroke),
        // Reaching here means the key was NOT eaten upstream — it simply matches no binding. Worth
        // a line of its own: it is the difference between "fix your shortcut" and "something else
        // owns this key", and the two have nothing in common but the symptom.
        None => log::debug!("key {} matches no binding", ev.keystroke),
    }
    action
}

/// Resolve a modifiers-changed event to the action bound to Caps Lock or to a lone modifier.
///
/// Neither key reaches an application as a key press: both platforms report them as a change of
/// modifier state, so `resolve` alone can never see them and a binding on Alt or Caps Lock would be
/// dead however it was recorded. [`MoonHotkeyModifierWatch`] turns the event stream back into a
/// press — Caps Lock on its state flip, a lone modifier on the release that follows nothing else —
/// and this reads that press against the same bindings as every other key.
///
/// `typing` suppresses the binding while the focused element is taking text: Caps Lock is a
/// perfectly ordinary key to press mid-word, and running a market order instead of shifting the
/// case is the one failure this feature can cause. The watch is still fed, so it stays in step with
/// the keyboard and the first press after the field is left is read correctly.
///
/// Args:
///     watch: Per-window watch state; one press spans several events, so it cannot be local.
///     ev: The modifiers-changed event as delivered to the window root.
///     hk: The window's effective hotkey configuration.
///     typing: Whether the focused element is taking typed text right now.
///
/// Returns:
///     The bound action, or `None` when the event is not a press or nothing is bound to it.
pub fn resolve_modifiers(
    watch: &mut MoonHotkeyModifierWatch,
    ev: &ModifiersChangedEvent,
    hk: &HotkeysConfig,
    typing: bool,
) -> Option<HotkeyAction> {
    match watch.modifiers_changed(ev.modifiers, ev.capslock, !typing) {
        MoonHotkeyCapture::Commit(keystroke) => resolve_binding(&keystroke, hk),
        _ => None,
    }
}

impl HotkeyAction {
    /// Whether a held key's repeats must NOT re-run this action.
    ///
    /// The line is what one repeat COSTS. Everything that creates, multiplies or closes something
    /// pays per press and nothing downstream deduplicates it — moonproto gives these commands no
    /// unique key — so a key left held would queue one order, one split or one market sell per OS
    /// repeat. A TOGGLE is worse than useless repeated: it flaps at the repeat rate, and a figure
    /// alert flaps on the wire, upserting and deleting a core-side object tens of times a second.
    /// Cycling through the drawing tools at that rate is the same kind of nonsense.
    ///
    /// The order SHIFTS used to be the exception — a held key nudged a line by one price step and
    /// the per-order `move_order` behind it was coalesced last-writer-wins by moonproto. They moved
    /// to a bulk percent command, which is sent with no unique key and coalesced nowhere, so a held
    /// key would now compound a ±1% move on the whole market tens of times a second.
    ///
    /// The chart shot is here for a different reason than the rest: it costs nothing REMOTE, but
    /// one press is a desktop `BitBlt` over the chart slot, a PNG encode and a seizure of clipboard
    /// ownership. A
    /// held key would do that tens of times a second and leave the clipboard thrashing.
    ///
    /// The ones still repeating: cancels (the second press finds nothing left to cancel) and the
    /// presets (setting a value twice sets it once).
    fn suppress_on_repeat(self) -> bool {
        matches!(
            self,
            Self::SplitOrder { .. }
                | Self::ShiftOrder { .. }
                | Self::SellsToRect
                | Self::NewLong
                | Self::NewShort
                | Self::JoinSells
                | Self::PanicSell
                | Self::PanicSellOne
                | Self::FigAlert
                // A held key repeats at the system rate, and each repeat would take another
                // figure: unlike `FigDelete`, which runs out after the one selected figure, this
                // one walks backwards through the whole chart until the finger comes off.
                | Self::FigUndo
                | Self::FigTool(_)
                | Self::SwitchFigure
                | Self::ChartShot
        )
    }
}

/// Resolve a key-down event to the first matching configured or built-in action.
///
/// Branch order defines collision precedence: configured figure actions; built-in Shift+Escape,
/// Escape, reset, and Tab/Delete; configured scale actions and the chart shot; order-size and
/// fixed-sell presets; active-market and active-core trading actions; configured `switch_charts`;
/// then manual strategies. Returns `None` when no binding matches.
fn resolve_binding(event: &Keystroke, hk: &HotkeysConfig) -> Option<HotkeyAction> {
    DISPATCH.iter().find_map(|step| match step {
        Step::Slot(slot) => pressed(hk.key(*slot), event).then(|| action_of(*slot, hk)),
        Step::Builtin(builtin) => builtin
            .strokes
            .iter()
            .any(|raw| pressed(raw, event))
            .then_some(builtin.action),
    })
}

/// One step of [`DISPATCH`]: a configurable slot, or a binding the user cannot edit.
#[derive(Clone, Copy)]
pub enum Step {
    Slot(KeySlot),
    Builtin(Builtin),
}

/// A built-in binding: the keystrokes it answers, the action, and the locale key naming it on the
/// settings page.
#[derive(Clone, Copy)]
pub struct Builtin {
    pub strokes: &'static [&'static str],
    pub action: HotkeyAction,
    pub name: &'static str,
}

const fn builtin(
    strokes: &'static [&'static str],
    action: HotkeyAction,
    name: &'static str,
) -> Step {
    Step::Builtin(Builtin {
        strokes,
        action,
        name,
    })
}

/// The order a keystroke is tested against the bindings — the ONE list that says which of two
/// holders of a key fires.
///
/// [`resolve_binding`] returns on the first match, so every later holder of the same keystroke is
/// dead; the settings page's clash captions read this same list to say so, which is why it is data
/// and not a chain of `if`s. The order is deliberate and worth keeping in view:
///
/// - the drawing layer is tested FIRST, which is the whole reason the figure slots can take a
///   built-in key away and everything below them cannot;
/// - the built-ins sit next: below the figure slots, above everything else. Shift+Escape before
///   plain Escape is only a reading aid — [`pressed`] matches modifiers exactly, so neither can
///   answer the other's press. Exactly, and that is one deliberate narrowing against the `if`
///   chain this replaced: that chain never looked at the Fn modifier, so Fn+Shift+Escape closed
///   every chart; now it is nobody's press, the same rule every configurable slot has always had.
///   Only the macOS backend sets the bit (`moon-gpui-windows` hardcodes it false), so this is a
///   Mac-only change to one built-in, made on purpose rather than carried as an exception;
/// - the window-local Y scale and the chart shot sit ABOVE the preset arrays: those are
///   user-editable, and a Moonbot import can move one onto any key at all;
/// - the trading actions and the manual-strategy presets close the list.
pub const DISPATCH: &[Step] = &[
    Step::Slot(KeySlot::DrawHline),
    Step::Slot(KeySlot::DrawHorizontalRay),
    Step::Slot(KeySlot::DrawSegment),
    Step::Slot(KeySlot::DrawTriangle),
    Step::Slot(KeySlot::DrawChannel),
    Step::Slot(KeySlot::SwitchFigure),
    Step::Slot(KeySlot::FigDelete),
    Step::Slot(KeySlot::FigAlert),
    Step::Slot(KeySlot::FigUndo),
    builtin(
        &["shift-escape"],
        HotkeyAction::CloseAllCharts,
        "hotkeys.clash.builtin.close_all",
    ),
    builtin(
        &["escape"],
        HotkeyAction::CloseActiveChart,
        "hotkeys.clash.builtin.esc_close",
    ),
    builtin(
        &["ctrl-shift-f10"],
        HotkeyAction::ResetWindows,
        "hotkeys.clash.builtin.reset_windows",
    ),
    builtin(
        &["tab", "delete"],
        HotkeyAction::CancelHoveredOrder,
        "hotkeys.clash.builtin.cancel_hover",
    ),
    Step::Slot(KeySlot::ScalePlus),
    Step::Slot(KeySlot::ScaleMinus),
    Step::Slot(KeySlot::ChartShot),
    Step::Slot(KeySlot::OrderSize(0)),
    Step::Slot(KeySlot::OrderSize(1)),
    Step::Slot(KeySlot::OrderSize(2)),
    Step::Slot(KeySlot::OrderSize(3)),
    Step::Slot(KeySlot::OrderSize(4)),
    Step::Slot(KeySlot::OrderSize(5)),
    Step::Slot(KeySlot::SellPreset(0)),
    Step::Slot(KeySlot::SellPreset(1)),
    Step::Slot(KeySlot::SellPreset(2)),
    Step::Slot(KeySlot::SellPreset(3)),
    Step::Slot(KeySlot::SellPreset(4)),
    Step::Slot(KeySlot::SellPreset(5)),
    Step::Slot(KeySlot::CancelBuy),
    Step::Slot(KeySlot::CancelAllBuys),
    Step::Slot(KeySlot::PanicSell),
    Step::Slot(KeySlot::PanicSellOne),
    Step::Slot(KeySlot::JoinSells),
    Step::Slot(KeySlot::SplitOrder),
    Step::Slot(KeySlot::SplitOrderX),
    Step::Slot(KeySlot::SellsToRect),
    Step::Slot(KeySlot::NewLong),
    Step::Slot(KeySlot::NewShort),
    Step::Slot(KeySlot::ShiftBuyUp),
    Step::Slot(KeySlot::ShiftBuyDown),
    Step::Slot(KeySlot::ShiftSellUp),
    Step::Slot(KeySlot::ShiftSellDown),
    Step::Slot(KeySlot::SwitchCharts),
    Step::Slot(KeySlot::ManualStrategy(0)),
    Step::Slot(KeySlot::ManualStrategy(1)),
    Step::Slot(KeySlot::ManualStrategy(2)),
    Step::Slot(KeySlot::ManualStrategy(3)),
    Step::Slot(KeySlot::ManualStrategy(4)),
    Step::Slot(KeySlot::ManualStrategy(5)),
    Step::Slot(KeySlot::ManualStrategy(6)),
    Step::Slot(KeySlot::ManualStrategy(7)),
    Step::Slot(KeySlot::ManualStrategy(8)),
    Step::Slot(KeySlot::ManualStrategy(9)),
];

/// Every configurable slot in [`DISPATCH`], in its order — for the tests that hold the list to
/// the config's own slot list.
#[cfg(test)]
pub(crate) fn slots_in_dispatch_order() -> impl Iterator<Item = KeySlot> {
    DISPATCH.iter().filter_map(|step| match step {
        Step::Slot(slot) => Some(*slot),
        Step::Builtin(_) => None,
    })
}

/// The action one slot performs.
///
/// Total over the slots, so a gesture bound to a slot (`hotkeys.toml` `[action_clicks]`) and a key
/// bound to it resolve to the same action through the same table. Split Order splits into a fixed
/// three, as Moonbot does; Split Order X into the configured count, which is why the config comes
/// along. Both split slots resolve to the SAME action, which `pre_dispatch` offers to the hovered
/// order before any market-level split.
pub fn action_of(slot: KeySlot, hk: &HotkeysConfig) -> HotkeyAction {
    use HotkeyAction as A;
    match slot {
        KeySlot::OrderSize(i) => A::OrderSize(i),
        KeySlot::SellPreset(i) => A::SellPreset(i),
        KeySlot::ManualStrategy(i) => A::ManualStrategy(i),
        KeySlot::CancelBuy => A::CancelBuy,
        KeySlot::PanicSell => A::PanicSell,
        KeySlot::PanicSellOne => A::PanicSellOne,
        KeySlot::CancelAllBuys => A::CancelAllBuys,
        KeySlot::JoinSells => A::JoinSells,
        KeySlot::SwitchCharts => A::SwitchCharts,
        KeySlot::NewLong => A::NewLong,
        KeySlot::NewShort => A::NewShort,
        KeySlot::SplitOrder => A::SplitOrder {
            parts: SPLIT_ORDER_PARTS,
        },
        KeySlot::SplitOrderX => A::SplitOrder {
            parts: hk.split_n_parts(),
        },
        KeySlot::SellsToRect => A::SellsToRect,
        KeySlot::ShiftBuyUp => A::ShiftOrder {
            sell: false,
            up: true,
        },
        KeySlot::ShiftBuyDown => A::ShiftOrder {
            sell: false,
            up: false,
        },
        KeySlot::ShiftSellUp => A::ShiftOrder {
            sell: true,
            up: true,
        },
        KeySlot::ShiftSellDown => A::ShiftOrder {
            sell: true,
            up: false,
        },
        KeySlot::ScalePlus => A::ScalePlus,
        KeySlot::ScaleMinus => A::ScaleMinus,
        KeySlot::SwitchFigure => A::SwitchFigure,
        KeySlot::ChartShot => A::ChartShot,
        KeySlot::DrawHline => A::FigTool(FigureTool::HLine),
        KeySlot::DrawHorizontalRay => A::FigTool(FigureTool::HorizontalRay),
        KeySlot::DrawSegment => A::FigTool(FigureTool::Segment),
        KeySlot::DrawTriangle => A::FigTool(FigureTool::Triangle),
        KeySlot::DrawChannel => A::FigTool(FigureTool::Channel),
        KeySlot::FigDelete => A::FigDelete,
        KeySlot::FigAlert => A::FigAlert,
        KeySlot::FigUndo => A::FigUndo,
    }
}

/// Rewrite a recorded keystroke onto the physical key, so the settings file is layout-independent.
///
/// A key recorded under a Cyrillic layout arrives named `"ф"`; stored as such it would work only in
/// that layout, and the file could not be shared with anyone using another one. Recording is where
/// this belongs: matching accepts both forms anyway, and the user sees the Latin letter they
/// physically pressed.
pub fn recorded_keystroke(mut keystroke: Keystroke) -> Keystroke {
    if let Some(physical) = layout::us_letter(&keystroke.key) {
        keystroke.key = physical;
    }
    keystroke
}

/// Cancel the order under the cursor through `Backend::hovered_chart`.
///
/// This is shared by the built-in Tab/Delete route and the caller's `FigDelete` fallback when no
/// figure is selected. The default `fig_delete = Delete` resolves before the built-in branch and
/// would otherwise shadow hovered-order cancellation. Returns `false` when no hovered chart or
/// order exists, allowing the key event to continue propagating.
pub fn cancel_hovered_order(backend: &Entity<Backend>, cx: &mut App) -> bool {
    with_hovered_chart(backend, cx, |panel, pcx| panel.cancel_hovered_order(pcx))
}

/// Log target for the manual-order trace that this module contributes to.
///
/// Stated instead of taken from `module_path!()`: the trace belongs to the chart's
/// `log.chart_input` diagnostic channel, and this router sits outside the subtree that channel
/// matches. Built FROM the filter's own constant rather than beside it — a second spelling of the
/// prefix is exactly how this channel came to be inert in the first place, and
/// `panels::chart::trade::tests` holds the constant against the real `module_path!()`.
const CHART_TRADE_TRACE: &str = moon_core::diagnostics::CHART_INPUT_TARGET;

/// This module's own channel, stated for the one trace that fires from `startup::boot` — the app
/// interceptor is registered there and would otherwise log outside `log.hotkeys` entirely.
const HOTKEYS_TRACE: &str = moon_core::diagnostics::HOTKEYS_TARGET;

/// Place a manual order at the cursor price through the chart under the pointer.
///
/// Shared by every window's `on_hotkey`, because the binding is addressed by the POINTER and not by
/// focus: the chart the pointer rests on owns both the market and the pane-Y-to-price conversion,
/// whichever window happens to hold the keyboard. This is the only refusal visible from HERE; the
/// rest live in [`crate::panels::ChartPanel::place_order_at_cursor`] and trace themselves there.
pub fn place_order_at_hovered_chart(backend: &Entity<Backend>, short: bool, cx: &mut App) -> bool {
    let Some(chart) = hovered_chart(backend, cx) else {
        log::debug!(
            target: CHART_TRADE_TRACE,
            "manual order refused: no chart under the pointer for the new-{} hotkey",
            if short { "short" } else { "long" }
        );
        return false;
    };
    chart.update(cx, |panel, pcx| panel.place_order_at_cursor(short, pcx))
}

/// The chart under the pointer, whichever window holds it, or `None` when there is none.
///
/// One lookup for every cursor-addressed action. Placement needs the handle itself, to tell "no
/// chart" from "the chart refused" and trace the difference; the rest go through
/// [`with_hovered_chart`]. Both read it HERE so that a future refinement — a stale weak handle
/// dropped, hover resolved differently — cannot send cancel and placement to different charts.
fn hovered_chart(backend: &Entity<Backend>, cx: &App) -> Option<Entity<crate::panels::ChartPanel>> {
    backend
        .read(cx)
        .hovered_chart
        .clone()
        .and_then(|w| w.upgrade())
}

/// Run `f` against the globally hovered chart panel, if there is one.
///
/// `false` means no chart is hovered, which leaves the key event free to keep propagating.
fn with_hovered_chart(
    backend: &Entity<Backend>,
    cx: &mut App,
    f: impl FnOnce(&mut crate::panels::ChartPanel, &mut Context<crate::panels::ChartPanel>) -> bool,
) -> bool {
    match hovered_chart(backend, cx) {
        Some(chart) => chart.update(cx, f),
        None => false,
    }
}

/// Apply the policies that belong to the action itself, before a window's own routing.
///
/// Every window's `on_hotkey` calls this once, ahead of its own match, so a third window inherits
/// both rules instead of restating them. `true` means the event is spent and the caller stops.
///
/// - **Auto-repeat**: a held key repeats at the system rate. That is harmless for a toggle and
///   multiplies anything that creates orders, so a repeat of such an action is consumed and does
///   nothing — consumed rather than merely ignored, or the repeats of a bound key would sail on to
///   whatever else that key means in the window.
/// - **Cursor-addressed target**: a hotkey has no click, so the order the pointer rests on is the
///   one the user means, the rule the built-in Tab/Delete cancellation already follows. Split needs
///   it; with nothing hovered it falls through to [`apply`]'s market-level split, which the core
///   performs only when the market has exactly one active sell order.
///
/// Escape's own rule lives in [`escape_leaves_sells_zone`], which the same callers run first.
pub fn pre_dispatch(
    action: HotkeyAction,
    is_held: bool,
    backend: &Entity<Backend>,
    cx: &mut App,
) -> bool {
    if is_held && action.suppress_on_repeat() {
        return true;
    }
    match action {
        HotkeyAction::SplitOrder { parts } => with_hovered_chart(backend, cx, |panel, pcx| {
            panel.split_hovered_order(parts, pcx)
        }),
        // The chart under the pointer owns the figures the user has been drawing, whichever window
        // holds the keyboard.
        HotkeyAction::FigUndo => match hovered_chart(backend, cx) {
            // Consumed whether or not anything was deleted, which is where this parts company with
            // the split above: falling through from a chart that has nothing drawn would send a
            // DESTRUCTIVE key on to the calling window's active chart — a different chart, off to
            // the side, that the user is not looking at. `FigAlert` refuses on the same grounds.
            Some(chart) => {
                // `target_at_cursor` and NOT `active_target`: a panel holds several PANES, and the
                // one the pointer is in is the chart the user means. `active_target` answers the
                // panel's own active pane, so over a stack it would delete the newest figure of a
                // NEIGHBOURING coin — and send that coin's alert delete to the core.
                if let Some((core, market)) = chart.read(cx).target_at_cursor() {
                    backend.update(cx, |b, bcx| {
                        if b.undo_last_figure(core, &market) {
                            bcx.notify();
                        }
                    });
                }
                true
            }
            // The pointer is on no chart at all, so there is no chart this means. Unhandled, like
            // every other cursor-addressed action with nothing under the cursor.
            None => false,
        },
        _ => false,
    }
}

/// Perform a slot's action for a press on a chart, exactly as a key press would.
///
/// The click half of a keyboard slot (`hotkeys.toml` `[action_clicks]`) resolves to the same
/// [`HotkeyAction`] as the key, and it must take the same road: [`pre_dispatch`] first — the
/// cursor-addressed policies, split on the hovered order, figure undo on the hovered chart — then
/// the WINDOW's own dispatch, which is where the scale, the chart switch, the shot and the
/// placement of a manual order are routed. Reached through the window's root view rather than by
/// a second copy of that routing. Every window's root is `moon_ui::Root`, which carries the real
/// view — a group window's [`Shell`], a detached chart window's [`DetachedChartHost`] — as an
/// `AnyView`; that inner view is what is downcast, and each keeps its own `dispatch_hotkey`.
///
/// Called DEFERRED from the chart's mouse handler (`Window::defer`), never inline: the handler
/// runs with the panel leased, and both `pre_dispatch` and the shell's routing reach for the
/// hovered chart — which is that very panel — through `Entity::update`. Inline, the second lease
/// would panic; deferred, the press is consumed at once and the action runs when the lease is
/// released, still within the same event's turn.
///
/// Returns:
///     Whether a window routed the action. `false` for a window whose root wraps neither host —
///     the diagnostics debug window is the one that draws a chart, and it routes no key press
///     either, so a click there answers exactly as a key does: nothing, with a warning.
///
/// `clicked` is the pane the press landed on. A group window's routing finds it on its own — the
/// hovered chart is the clicked one — but a detached window's key routing acts on the WINDOW's
/// market, which for a multi-coin window is the anchor or nothing; a click names its pane instead.
pub fn dispatch_from_chart(
    action: HotkeyAction,
    clicked: Option<(CoreId, String)>,
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if pre_dispatch(action, false, backend, cx) {
        return true;
    }
    // Not `Root::read`: that one panics on a window whose root is something else, and a warning
    // is the right answer to a window nobody expected a chart in.
    let inner = window
        .root::<moon_ui::Root>()
        .flatten()
        .map(|root| root.read(cx).view().clone());
    let routed = inner.and_then(|inner| match inner.downcast::<crate::shell::Shell>() {
        Ok(shell) => {
            Some(shell.update(cx, |shell, scx| shell.dispatch_hotkey(action, window, scx)))
        }
        Err(other) => other
            .downcast::<crate::chart_tabs::DetachedChartHost>()
            .ok()
            .map(|host| {
                host.update(cx, |host, hcx| {
                    host.dispatch_hotkey_at(action, clicked, window, hcx)
                })
            }),
    });
    routed.unwrap_or_else(|| {
        log::warn!("a chart click resolved to {action:?}, but its window has no hotkey routing");
        false
    })
}

/// Let Escape leave the Sells-to-zone mode before anything else acts on it.
///
/// Matched on the RAW key rather than on a resolved action: the mode's own posture is Ctrl held
/// down, and `resolve` only reads Escape as "close the chart" when no modifier is present — so
/// Ctrl+Escape, which is what a hand still on the modifier actually presses, would otherwise reach
/// nothing at all. A mode entered by a key needs a way out that is not the same key.
///
/// Consumes the press when it disarms, so the SECOND Escape closes the chart exactly as before.
/// Every window calls this ahead of its own routing, which is why it takes the event and not an
/// action.
pub fn escape_leaves_sells_zone(
    ev: &KeyDownEvent,
    backend: &Entity<Backend>,
    cx: &mut App,
) -> bool {
    if ev.keystroke.key != "escape" {
        return false;
    }
    backend.update(cx, |b, bcx| {
        if !b.sells_zone_armed() {
            return false;
        }
        b.disarm_sells_zone();
        bcx.notify();
        true
    })
}

/// Execute a shared backend action against the caller's trading context.
///
/// `target` is the calling window's active chart market as `(core, market)`, and `active_core` is
/// its active trading core. `group` owns size and exit hotkeys even when no core is live. `true`
/// means the action was handled and the caller should stop key propagation; for command-sending
/// actions it does not guarantee remote success. Actions that require caller-level window,
/// hovered-chart, group-revision, or application context return `false` for caller routing.
pub fn apply(
    action: HotkeyAction,
    b: &mut Backend,
    bcx: &mut Context<Backend>,
    group: &str,
    target: Option<(CoreId, String)>,
    active_core: Option<CoreId>,
) -> bool {
    use HotkeyAction as A;
    match action {
        A::FigTool(tool) => {
            // The armed mode ends first, so the toggle below tests the user's OWN tool: the mode
            // leaves `Channel` selected, and testing that instead would make the Zone binding turn
            // drawing off rather than select the Zone.
            b.disarm_sells_zone();
            // Repeating the active tool disables drawing; another tool selects it and enables drawing.
            if b.fig_draw_mode && b.fig_tool == tool {
                b.fig_draw_mode = false;
            } else {
                b.select_fig_tool(tool);
            }
            bcx.notify();
            true
        }
        A::SwitchFigure => {
            // Cycle through tools and keep drawing mode enabled, matching Moonbot's switch-figure
            // action. This action never exits drawing mode; the Cursor entry or the active-tool
            // toggle does. Disarming first means the cycle starts from the tool the mode
            // interrupted rather than from the `Channel` it forced.
            b.disarm_sells_zone();
            b.select_fig_tool(b.next_fig_tool());
            bcx.notify();
            true
        }
        A::FigAlert => {
            // Consumed whenever a figure is selected, even if arming was refused — a tool the core
            // has no type for, or a figure shared across cores, refuses the toggle, and letting the
            // key fall through would be a surprise rather than a refusal. With NO selection the key
            // is not ours, exactly as `FigDelete` leaves it.
            if b.fig_selected.is_none() {
                false
            } else {
                if b.toggle_selected_figure_alert() {
                    bcx.notify();
                }
                true
            }
        }
        A::FigDelete => {
            if let Some((core, market, id)) = b.fig_selected.clone() {
                b.remove_figure(core, &market, id);
                bcx.notify();
                true
            } else {
                false
            }
        }
        A::OrderSize(i) => {
            b.set_order_size_sel(group, i);
            b.order_size_rev = b.order_size_rev.wrapping_add(1);
            bcx.notify();
            true
        }
        // Auto Overview draws no manual-strategy cluster (`chrome::terminal_chrome::header`), so
        // the hotkey has nothing to report back through: `active_core` there is the group's
        // FALLBACK core, and firing this would enable MS and re-seed that one server's exits with
        // no switch, no pill and no summary on screen to say it happened — and the next order
        // would go out under it. Refusing lets the key propagate, exactly like the no-core arm.
        A::ManualStrategy(_) if b.is_auto_overview_scope(group) => false,
        A::ManualStrategy(i) => match active_core {
            Some(core) => {
                if crate::controls::select_manual_strategy(b, core, i) {
                    bcx.notify();
                    true
                } else {
                    false
                }
            }
            None => false,
        },
        A::SellPreset(i) => {
            if b.edit_group_exit(group, ClientSettingsEdit::SelectFixedSellSlot(i + 1)) {
                b.order_size_rev = b.order_size_rev.wrapping_add(1);
                bcx.notify();
                true
            } else {
                false
            }
        }
        A::CancelBuy => match target {
            Some((core, market)) => {
                b.cancel_buy_orders(core, &market);
                true
            }
            None => false,
        },
        A::CancelAllBuys => match active_core {
            Some(core) => {
                b.cancel_all_buys_for_core(core);
                true
            }
            None => false,
        },
        // The only debounce on the Panic Sell path: an impatient re-jab within the hotkey's
        // debounce window is absorbed as a no-op. The direct chart-button path is deliberately
        // unguarded because it is an explicit click on the labelled control.
        A::PanicSell => match target {
            Some((core, market)) => {
                if b.panic_sell_hotkey(core, market) {
                    bcx.notify();
                }
                // Consumed rather than merely ignored, same as `pre_dispatch`: the key is ours
                // even when the press itself was absorbed, and must not fall through.
                true
            }
            None => false,
        },
        A::PanicSellOne => match target {
            Some((core, market)) => {
                if let Err(error) = b.session.market_sell_position(core, market) {
                    log::warn!("hotkey market sell position failed: {error}");
                }
                true
            }
            None => false,
        },
        A::JoinSells => match target {
            Some((core, market)) => {
                let short = b.market_position_short(core, &market);
                if let Err(error) = b.session.join_sells(core, market, short) {
                    log::warn!("hotkey join sells failed: {error}");
                }
                true
            }
            None => false,
        },
        A::SplitOrder { parts } => match target {
            Some((core, market)) => {
                if let Err(error) = b.session.split_order_for_market(core, market, parts) {
                    log::warn!("hotkey split order failed: {error}");
                }
                true
            }
            None => false,
        },
        A::ShiftOrder { sell, up } => match target {
            Some((core, market)) => shift_orders(b, core, &market, sell, up),
            None => false,
        },
        // Arms the draw; it sends nothing by itself, so it needs no target. The band is placed on
        // the chart that is clicked, and THAT chart's market is the one the command addresses.
        A::SellsToRect => {
            b.toggle_sells_zone_arm();
            bcx.notify();
            true
        }
        // Caller-routed actions have mixed scope: scale is window-specific; cursor placement,
        // cancellation and the figure undo use `Backend::hovered_chart`; switching and active-chart
        // closing are group-local; reset and close-all are application-global; and the chart shot
        // needs the OS window plus the clipboard, neither of which reaches this function.
        A::ScalePlus
        | A::ScaleMinus
        | A::NewLong
        | A::NewShort
        | A::FigUndo
        | A::SwitchCharts
        | A::ResetWindows
        | A::CancelHoveredOrder
        | A::CloseAllCharts
        | A::CloseActiveChart
        | A::ChartShot => false,
    }
}

/// Spread the market's sell orders across a price band, as Moonbot's "sells to rectangle" does.
///
/// `a` and `z` are the band's two prices in the order they were drawn or stored; ordering them is
/// [`moon_core::session::Session::sells_to_zone`]'s job. ONE command goes out, carrying only the
/// market, the side and the two prices: the core selects the sells and does the spreading, and the
/// moved orders come back through the ordinary order stream.
///
/// Shared by both ways to name a band — a band drawn in Ctrl+S mode, whose figure is never
/// stored, and the right-click entry on a Zone or Rect that is. Every refusal below is logged rather than
/// reported: nothing upstream can act on the difference, and the band is gone from the screen by
/// the time this runs.
pub(crate) fn sells_to_zone(b: &mut Backend, core: CoreId, market: &str, a: f64, z: f64) {
    // Refuse a band no thinner than one price STEP of this market — the authoritative number, not a
    // constant picked here: below it every sell rounds onto the same level, which is the opposite of
    // spreading them. An unknown step falls back to demanding two distinct prices. Logged, like the
    // other refusals, because the ways this key can legitimately do nothing — no band, a flat one,
    // no sell order, a core that is not connected — are otherwise indistinguishable from a command
    // that went out, and this one moves live money.
    let thin = match b.session.market_source().price_step(core, market) {
        Some(step) => (a - z).abs() < step,
        None => a == z,
    };
    if thin || !(a.is_finite() && z.is_finite()) || a <= 0.0 || z <= 0.0 {
        log::warn!(
            "sells to zone: core={} market={market} zone {a:.8}..{z:.8} is thinner than one price step or out of range, nothing sent",
            moon_core::feed::core_label(core)
        );
        return;
    }
    let short = b.market_position_short(core, market);
    // Logged AFTER the send, so the line means "this left the terminal" rather than "this was about
    // to": in a mode that draws band after band, an overstating log is the only record anyone has.
    match b
        .session
        .sells_to_zone(core, market.to_string(), a, z, short)
    {
        Ok(()) => log::info!(
            "sells to zone: core={} market={market} zone={:.8}..{:.8} short={short}",
            moon_core::feed::core_label(core),
            a.min(z),
            a.max(z),
        ),
        Err(error) => log::warn!("sells to zone failed: {error}"),
    }
}

/// Shift the active market's orders of one side by [`SHIFT_PERCENT`], as Moonbot's ±1% does.
///
/// One command per press, not one per order: the core selects the orders by their own phase and
/// does the arithmetic, which is both what Moonbot's action means and the only way the two sides
/// stay consistent — the previous local loop replaced each order individually by one price STEP,
/// a different action wearing the same name.
///
/// That hands the selection to the core, and it does not draw the same line the chart's own drag
/// does: a partially filled entry is still in its buy phase and moves here, while dragging its line
/// refuses it. The core's phase is the authoritative answer, and matching Moonbot is the point.
fn shift_orders(b: &mut Backend, core: CoreId, market: &str, sell: bool, up: bool) -> bool {
    // Local pre-check so the ways this press can do nothing — no order of that phase, a market this
    // core does not hold, a core that is not connected — are not all logged as a sent command;
    // moonproto drops a candidate-less bulk move silently and tells nobody.
    //
    // It asks the SAME question the core's gate asks — `OrderWorkerStatus`, the authoritative
    // lifecycle phase — rather than inferring one from the lines. A Buy line exists for an order's
    // whole life, including long after the entry filled, so "the line is there" would answer yes on
    // every market that ever bought.
    let phase = if sell { "SellSet" } else { "BuySet" };
    let has_phase = b.session.store().core(core).is_some_and(|data| {
        data.orders
            .iter()
            .any(|order| order.market == market && order.status == phase)
    });
    if !has_phase {
        log::warn!(
            "hotkey shift orders: core={} market={market} has no order in {phase}, nothing sent",
            moon_core::feed::core_label(core),
        );
        return false;
    }
    let percent = if up { SHIFT_PERCENT } else { -SHIFT_PERCENT };
    log::info!(
        "hotkey shift orders: core={} market={market} side={} percent={percent:+}",
        moon_core::feed::core_label(core),
        if sell { "sell" } else { "buy" },
    );
    if let Err(error) = b
        .session
        .shift_orders_percent(core, market.to_string(), sell, percent)
    {
        log::warn!("hotkey shift orders failed: {error}");
    }
    true
}
