// Do not use `super::*`: the parent re-exports GPUI's `test` attribute macro through its imports,
// which would shadow the built-in `#[test]`.
use super::layout::us_letter;
use gpui::Keystroke;
use moon_core::config::HotkeysConfig;

use super::{binding_id, same_binding};

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

#[test]
fn the_hotkey_channel_prefix_still_matches_this_module() {
    // Same guard as `panels::chart::trade::tests`, and for the same reason: moon-core names a
    // prefix rooted at the BINARY, which it cannot verify from its own side. Left unchecked, a
    // `[[bin]]` rename or a module move turns the switch inert while every gate stays green —
    // which is exactly what happened to `log.chart_input` and went unnoticed for its whole life.
    let prefix = moon_core::diagnostics::HOTKEYS_TARGET;
    assert!(
        module_path!().starts_with(prefix),
        "log.hotkeys matches {prefix:?}, but this module logs as {:?}",
        module_path!()
    );
}

/// Pins the line between a press the focused field consumes and one that still reaches a binding.
///
/// Plausible breakage: the rule is rewritten as "anything without a modifier", which takes Escape
/// and the function keys — where the order-size and sell-preset defaults live — away from a user
/// whose caret sits in a search box; or the modifier cut goes, and an Option press on macOS, which
/// DOES carry a character, stops firing the `alt-` bindings that are most of the shipped keymap.
/// Both read as "the gate works" from the bug it was written for.
#[test]
fn only_the_presses_a_field_consumes_belong_to_it() {
    use super::belongs_to_the_field;
    use gpui::Keystroke;

    // `with_simulated_ime` fills `key_char` the way GPUI itself models a press, so these assert
    // against the runtime's own table rather than against hand-spelled fixtures.
    for raw in ["t", "1", "shift-a", "space", "tab"] {
        let key = Keystroke::parse(raw).unwrap().with_simulated_ime();
        assert!(
            belongs_to_the_field(&key),
            "{raw} is what the field is being typed into"
        );
    }
    // Windows reports no character for Tab, which is the whole reason the rule names the key as
    // well: read without the simulated fill, this is the press that platform delivers.
    assert!(
        belongs_to_the_field(&Keystroke::parse("tab").unwrap()),
        "tab belongs to the form even where the platform reports no character for it"
    );
    for raw in ["escape", "f1", "shift-f7", "delete"] {
        let key = Keystroke::parse(raw).unwrap().with_simulated_ime();
        assert!(
            !belongs_to_the_field(&key),
            "{raw} reports no character, so it stays bound"
        );
    }
    // The modifier cut, which only these can pin: a press that DOES carry a character and is a
    // binding anyway — Option on macOS, AltGr on Windows. `->` is GPUI's syntax for spelling out
    // what the platform reported, because `with_simulated_ime` deliberately fills none of these.
    for raw in ["alt-c->c", "ctrl-alt-e->e", "cmd-k->k"] {
        let key = Keystroke::parse(raw).unwrap();
        assert!(
            !belongs_to_the_field(&key),
            "{raw} is a binding, and the character is the one the user did not ask for"
        );
    }
}

/// One press, two spellings, and the tree really produces both: `Keystroke::unparse` writes a
/// recorded key as `ctrl-alt-win-shift-k` on Windows while `moonbot_import::shortcut` writes
/// `ctrl-alt-shift-cmd-k` for the same press. Every collision question in the app turns on these
/// being one binding.
///
/// Plausible breakage: comparing the strings again — in the clash captions, in the core pull, or in a
/// third place added later — which reads a taken key as free and double-binds the file.
#[test]
fn one_press_spelled_two_ways_is_one_binding() {
    // Modifier order.
    assert!(same_binding("shift-ctrl-z", "ctrl-shift-z"));
    // Case, in both the modifier and a named key.
    assert!(same_binding("Ctrl-F10", "ctrl-f10"));
    // `cmd`, `super` and `win` are one modifier to `Keystroke::parse`, and the two producers in this
    // tree disagree on which word they write.
    assert!(same_binding("ctrl-alt-win-shift-k", "ctrl-alt-shift-cmd-k"));
    assert!(same_binding("super-k", "cmd-k"));
    // An uppercase single character IS shift plus the lowercase one, by the parser's own rule.
    assert!(same_binding("ctrl-Z", "ctrl-shift-z"));
}

/// The other direction, or the comparison would report every row as colliding with every other.
#[test]
fn different_presses_and_unusable_strings_are_not_one_binding() {
    assert!(!same_binding("ctrl-z", "ctrl-shift-z"));
    assert!(!same_binding("alt-1", "alt-2"));
    // Two slots that can never fire do not collide with each other: an empty row must carry no
    // clash caption, and neither must a row holding something the parser rejects.
    assert!(!same_binding("", ""));
    assert!(!same_binding("   ", "ctrl-z"));
    assert!(binding_id("ctrl-a-b").is_none(), "the key must come last");
    assert!(binding_id("").is_none());
}

/// The built-ins resolve through the same exact-modifier match as every slot, so the list can
/// carry them as keystroke strings rather than as hand-written branches.
///
/// Plausible breakage: a built-in spelled so that a modified press satisfies it — Ctrl+Escape
/// closing the chart, or Shift+Tab cancelling an order — which the old `if` chain excluded by
/// testing the modifiers one by one.
#[test]
fn the_builtins_match_their_press_exactly() {
    use super::{HotkeyAction as A, resolve_binding};
    let hk = HotkeysConfig::default();
    let at = |raw: &str| resolve_binding(&Keystroke::parse(raw).unwrap(), &hk);
    assert_eq!(at("shift-escape"), Some(A::CloseAllCharts));
    assert_eq!(at("escape"), Some(A::CloseActiveChart));
    assert_eq!(at("ctrl-escape"), None, "a modified Escape is nobody's");
    assert_eq!(at("ctrl-shift-f10"), Some(A::ResetWindows));
    assert_eq!(at("tab"), Some(A::CancelHoveredOrder));
    assert_eq!(
        at("delete"),
        Some(A::FigDelete),
        "the figure slot ships on Delete, above the cancel"
    );
    assert_eq!(at("shift-tab"), None);
}

/// A slot's key and its action are one table: every slot in the dispatch order resolves, when its
/// own key is pressed, to `action_of` that slot — which is what lets a mouse gesture bound to the
/// slot execute the same action by the same name.
#[test]
fn every_slot_resolves_to_its_own_action() {
    use super::{action_of, resolve_binding, slots_in_dispatch_order};
    let mut hk = HotkeysConfig::default();
    // A distinct, otherwise-unbound key per slot, so the first match is the slot itself.
    for (n, slot) in slots_in_dispatch_order().enumerate() {
        hk.set_key(slot, format!("ctrl-alt-shift-f{}", n % 12 + 1));
    }
    // With twelve function keys and forty-eight slots the keys repeat; give each slot its own turn
    // by clearing the others first.
    for slot in slots_in_dispatch_order() {
        let mut one = hk.clone();
        for other in slots_in_dispatch_order() {
            if other != slot {
                one.set_key(other, String::new());
            }
        }
        let raw = one.key(slot).to_string();
        let got = resolve_binding(&Keystroke::parse(&raw).unwrap(), &one);
        assert_eq!(got, Some(action_of(slot, &one)), "{slot:?} on {raw}");
    }
}
