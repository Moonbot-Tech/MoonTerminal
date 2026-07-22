use super::*;

#[test]
fn empty_shortcut() {
    assert_eq!(decode(0), DecodedShortcut::Empty);
}

#[test]
fn plain_f1() {
    // VK_F1 = 0x70 without modifiers.
    assert_eq!(
        decode(0x0070),
        DecodedShortcut::Key {
            mods: ShortcutMods::default(),
            key: ShortcutKey::F(1),
        }
    );
}

#[test]
fn shift_f7() {
    assert_eq!(
        decode(MOD_SHIFT | 0x76),
        DecodedShortcut::Key {
            mods: ShortcutMods {
                shift: true,
                ..Default::default()
            },
            key: ShortcutKey::F(7),
        }
    );
}

#[test]
fn ctrl_delete() {
    assert_eq!(
        decode(MOD_CTRL | 0x2E),
        DecodedShortcut::Key {
            mods: ShortcutMods {
                ctrl: true,
                ..Default::default()
            },
            key: ShortcutKey::Delete,
        }
    );
}

#[test]
fn letters_digits_and_oem() {
    assert_eq!(
        decode(MOD_ALT | 0x44), // Alt+D
        DecodedShortcut::Key {
            mods: ShortcutMods {
                alt: true,
                ..Default::default()
            },
            key: ShortcutKey::Char('D'),
        }
    );
    assert_eq!(
        decode(0x0031),
        DecodedShortcut::Key {
            mods: ShortcutMods::default(),
            key: ShortcutKey::Char('1'),
        }
    );
    assert_eq!(
        decode(0x00C0),
        DecodedShortcut::Key {
            mods: ShortcutMods::default(),
            key: ShortcutKey::Oem('`'),
        }
    );
}

#[test]
fn unknown_vk_is_unsupported_not_guessed() {
    // 0xE5 (VK_PROCESSKEY) is absent from the table, so it is Unsupported rather than guessed.
    assert_eq!(decode(0x00E5), DecodedShortcut::Unsupported { raw: 0x00E5 });
    // Nonzero unused bits 0x0F00 are also Unsupported.
    assert_eq!(decode(0x0170), DecodedShortcut::Unsupported { raw: 0x0170 });
}

#[test]
fn gpui_keystroke_format() {
    assert_eq!(to_gpui_keystroke(decode(0)), None);
    assert_eq!(to_gpui_keystroke(decode(0x00E5)), None);
    assert_eq!(to_gpui_keystroke(decode(0x0070)).unwrap(), "f1");
    assert_eq!(
        to_gpui_keystroke(decode(MOD_SHIFT | 0x76)).unwrap(),
        "shift-f7"
    );
    assert_eq!(
        to_gpui_keystroke(decode(MOD_CTRL | 0x2E)).unwrap(),
        "ctrl-delete"
    );
    assert_eq!(to_gpui_keystroke(decode(MOD_ALT | 0x44)).unwrap(), "alt-d");
    assert_eq!(
        to_gpui_keystroke(decode(MOD_CTRL | MOD_SHIFT | 0x21)).unwrap(),
        "ctrl-shift-pageup"
    );
}

#[test]
fn display_labels() {
    assert_eq!(display(decode(0)), "—");
    assert_eq!(
        display(decode(MOD_CTRL | MOD_SHIFT | 0x76)),
        "Ctrl+Shift+F7"
    );
    assert_eq!(display(decode(MOD_ALT | 0x44)), "Alt+D");
    assert_eq!(display(decode(0x00E5)), "VK 0xE5?");
}
