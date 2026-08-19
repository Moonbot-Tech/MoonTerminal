// Do not use `super::*`: the parent re-exports GPUI's `test` attribute macro through its imports,
// which would shadow the built-in `#[test]`.
use super::layout::us_letter;

/// Pins what the layout translation must NOT touch.
///
/// Plausible breakage: translating an ASCII name round-trips a letter through the active layout and
/// can return a DIFFERENT letter under a non-Latin one; translating a multi-character name would
/// take the first character of `f7` or `delete` and answer for it.
#[test]
fn only_single_non_ascii_key_names_are_translated() {
    for key in ["a", "z", "s", "f7", "delete", "escape", "up", "", "1"] {
        assert_eq!(us_letter(key), None, "{key:?} must be left alone");
    }
}

/// Pins the physical-key answer on Windows when the active layout IS Latin.
///
/// Plausible breakage: a translation that fires on ASCII would rewrite every letter hotkey on the
/// developer's own machine, which is exactly where it would go unnoticed.
#[test]
fn a_latin_layout_needs_no_translation() {
    // Whatever layout this machine runs, an ASCII name is already the physical key.
    assert_eq!(us_letter("q"), None);
    assert_eq!(us_letter("w"), None);
}

/// Pins that the two clicks of an armed Sells-to-zone draw ARE the two prices that go on the wire.
///
/// Plausible breakage: the Zone tool starts building its band from something other than its two
/// placed nodes — a snapped level, a single price plus a width — and the command silently addresses
/// a band the user did not draw, which nothing downstream can detect. Which TOOLS are bands is
/// pinned next to the tools themselves, in `moon_core::figures::tools`.
#[test]
fn the_zone_tool_sends_the_two_prices_that_were_clicked() {
    use moon_core::figures::{FigNode, FigureTool};

    let clicks = [FigNode::new(1_000.0, 42.5), FigNode::new(2_000.0, 41.0)];
    let def = FigureTool::Channel.def();
    assert_eq!(def.clicks, 2, "the mode is documented as a two-click draw");
    let kind = (def.make)(&clicks).expect("two nodes finish a Zone");
    assert_eq!(kind.price_band(), Some((42.5, 41.0)));
}

/// Pins that a binding on Caps Lock or on a lone modifier resolves at all.
///
/// Plausible breakage: `resolve_modifiers` reads the watch but resolves against something other
/// than the shared bindings — or the release path is dropped — and a shortcut the settings page
/// happily records simply never fires, with nothing on screen to say why.
#[test]
fn caps_lock_and_a_lone_modifier_resolve_to_their_bound_action() {
    use crate::hotkeys::{HotkeyAction, resolve_modifiers};
    use gpui::{Capslock, Modifiers, ModifiersChangedEvent};
    use moon_core::config::HotkeysConfig;
    use moon_ui::MoonHotkeyModifierWatch;

    let hk = HotkeysConfig {
        panic_sell: "capslock".to_string(),
        cancel_all_buys: "alt".to_string(),
        ..HotkeysConfig::default()
    };
    let event = |modifiers, on| ModifiersChangedEvent {
        modifiers,
        capslock: Capslock { on },
    };
    let mut watch = MoonHotkeyModifierWatch::default();
    watch.prime(Modifiers::none(), Capslock { on: false });

    assert_eq!(
        resolve_modifiers(&mut watch, &event(Modifiers::none(), true), &hk, false),
        Some(HotkeyAction::PanicSell),
        "flipping Caps Lock is its press"
    );
    assert_eq!(
        resolve_modifiers(&mut watch, &event(Modifiers::alt(), true), &hk, false),
        None,
        "a held modifier may still become a chord"
    );
    assert_eq!(
        resolve_modifiers(&mut watch, &event(Modifiers::none(), true), &hk, false),
        Some(HotkeyAction::CancelAllBuys),
        "releasing it with nothing pressed in between is the press"
    );
}

/// Pins that neither key fires while the focused element is taking typed text.
///
/// Plausible breakage: dropping the `typing` gate. Caps Lock is an ordinary key to press mid-word,
/// and with panic sell bound to it, shifting the case of a coin name would sell the position — the
/// one way this feature can cost money rather than a keystroke.
#[test]
fn typing_suppresses_a_modifier_binding_without_desynchronizing_it() {
    use crate::hotkeys::{HotkeyAction, resolve_modifiers};
    use gpui::{Capslock, Modifiers, ModifiersChangedEvent};
    use moon_core::config::HotkeysConfig;
    use moon_ui::MoonHotkeyModifierWatch;

    let hk = HotkeysConfig {
        panic_sell: "capslock".to_string(),
        ..HotkeysConfig::default()
    };
    let event = |on| ModifiersChangedEvent {
        modifiers: Modifiers::none(),
        capslock: Capslock { on },
    };
    let mut watch = MoonHotkeyModifierWatch::default();
    watch.prime(Modifiers::none(), Capslock { on: false });

    assert_eq!(
        resolve_modifiers(&mut watch, &event(true), &hk, true),
        None,
        "the field is taking text, so the key belongs to the field"
    );
    // The watch still followed that flip: the next press is read as a press, not as the first
    // observation of a state it missed.
    assert_eq!(
        resolve_modifiers(&mut watch, &event(false), &hk, false),
        Some(HotkeyAction::PanicSell)
    );
}
