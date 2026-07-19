//! Декодер Delphi `TShortCut` (u16): старшие биты — модификаторы, младший байт —
//! Windows virtual-key code. Только ЯВНЫЙ маппинг известных VK: неизвестный код —
//! [`DecodedShortcut::Unsupported`], угадывать строку GPUI нельзя (ТЗ §6).

/// Бит модификатора Command (macOS-легаси Delphi; в Windows-конфигах не встречается).
pub const MOD_CMD: u16 = 0x1000;
pub const MOD_SHIFT: u16 = 0x2000;
pub const MOD_CTRL: u16 = 0x4000;
pub const MOD_ALT: u16 = 0x8000;

/// Модификаторы сочетания.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShortcutMods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub cmd: bool,
}

/// Явно поддержанная клавиша (US-раскладка для OEM-символов).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutKey {
    /// `A`–`Z` (заглавная) или `0`–`9` верхнего ряда.
    Char(char),
    /// F1–F24 (1..=24).
    F(u8),
    Escape,
    Enter,
    Backspace,
    Tab,
    Space,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    /// OEM-клавиша по её символу US-раскладки: `;` `=` `,` `-` `.` `/` `` ` `` `[` `\` `]` `'`.
    Oem(char),
}

/// Результат декодирования.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedShortcut {
    /// 0 — сочетание не назначено.
    Empty,
    /// Известная клавиша + модификаторы.
    Key {
        mods: ShortcutMods,
        key: ShortcutKey,
    },
    /// Неизвестный VK или неиспользуемые биты — переносить нельзя, показываем как есть.
    Unsupported { raw: u16 },
}

/// Разобрать сырое `TShortCut`-значение.
pub fn decode(raw: u16) -> DecodedShortcut {
    if raw == 0 {
        return DecodedShortcut::Empty;
    }
    // Биты 0x0F00 в младшей части TShortCut не используются: ненулевые — не наш формат.
    if raw & 0x0F00 != 0 {
        return DecodedShortcut::Unsupported { raw };
    }
    let mods = ShortcutMods {
        ctrl: raw & MOD_CTRL != 0,
        shift: raw & MOD_SHIFT != 0,
        alt: raw & MOD_ALT != 0,
        cmd: raw & MOD_CMD != 0,
    };
    match vk_to_key((raw & 0xFF) as u8) {
        Some(key) => DecodedShortcut::Key { mods, key },
        None => DecodedShortcut::Unsupported { raw },
    }
}

/// Явная таблица Windows VK → клавиша. Всё, чего здесь нет, — неподдерживаемо.
fn vk_to_key(vk: u8) -> Option<ShortcutKey> {
    use ShortcutKey::*;
    Some(match vk {
        0x08 => Backspace,
        0x09 => Tab,
        0x0D => Enter,
        0x1B => Escape,
        0x20 => Space,
        0x21 => PageUp,   // VK_PRIOR
        0x22 => PageDown, // VK_NEXT
        0x23 => End,
        0x24 => Home,
        0x25 => Left,
        0x26 => Up,
        0x27 => Right,
        0x28 => Down,
        0x2D => Insert,
        0x2E => Delete,
        0x30..=0x39 => Char(vk as char), // '0'..'9' — VK совпадает с ASCII
        0x41..=0x5A => Char(vk as char), // 'A'..'Z' — VK совпадает с ASCII
        0x70..=0x87 => F(vk - 0x70 + 1), // F1..F24
        0xBA => Oem(';'),                // VK_OEM_1
        0xBB => Oem('='),                // VK_OEM_PLUS
        0xBC => Oem(','),                // VK_OEM_COMMA
        0xBD => Oem('-'),                // VK_OEM_MINUS
        0xBE => Oem('.'),                // VK_OEM_PERIOD
        0xBF => Oem('/'),                // VK_OEM_2
        0xC0 => Oem('`'),                // VK_OEM_3
        0xDB => Oem('['),                // VK_OEM_4
        0xDC => Oem('\\'),               // VK_OEM_5
        0xDD => Oem(']'),                // VK_OEM_6
        0xDE => Oem('\''),               // VK_OEM_7
        _ => return None,
    })
}

/// Сочетание в формате `gpui::Keystroke::parse` (`ctrl-shift-f7`, `alt-d`) — формат
/// хранения `HotkeysConfig`. `None` для пустого/неподдерживаемого (переносить нечего).
pub fn to_gpui_keystroke(short: DecodedShortcut) -> Option<String> {
    let DecodedShortcut::Key { mods, key } = short else {
        return None;
    };
    let mut s = String::new();
    if mods.ctrl {
        s.push_str("ctrl-");
    }
    if mods.alt {
        s.push_str("alt-");
    }
    if mods.shift {
        s.push_str("shift-");
    }
    if mods.cmd {
        s.push_str("cmd-");
    }
    use ShortcutKey::*;
    match key {
        Char(c) => s.push(c.to_ascii_lowercase()),
        F(n) => s.push_str(&format!("f{n}")),
        Escape => s.push_str("escape"),
        Enter => s.push_str("enter"),
        Backspace => s.push_str("backspace"),
        Tab => s.push_str("tab"),
        Space => s.push_str("space"),
        Insert => s.push_str("insert"),
        Delete => s.push_str("delete"),
        Home => s.push_str("home"),
        End => s.push_str("end"),
        PageUp => s.push_str("pageup"),
        PageDown => s.push_str("pagedown"),
        Left => s.push_str("left"),
        Right => s.push_str("right"),
        Up => s.push_str("up"),
        Down => s.push_str("down"),
        Oem(c) => s.push(c),
    }
    Some(s)
}

/// Человекочитаемая подпись для preview: `Ctrl+Shift+F7`, `Alt+D`, `—` для пустого,
/// `VK 0xE5?` для неподдерживаемого.
pub fn display(short: DecodedShortcut) -> String {
    match short {
        DecodedShortcut::Empty => "—".to_string(),
        DecodedShortcut::Unsupported { raw } => format!("VK 0x{:02X}?", raw & 0xFF),
        DecodedShortcut::Key { mods, key } => {
            let mut s = String::new();
            if mods.ctrl {
                s.push_str("Ctrl+");
            }
            if mods.alt {
                s.push_str("Alt+");
            }
            if mods.shift {
                s.push_str("Shift+");
            }
            if mods.cmd {
                s.push_str("Cmd+");
            }
            use ShortcutKey::*;
            match key {
                Char(c) => s.push(c),
                F(n) => s.push_str(&format!("F{n}")),
                Escape => s.push_str("Esc"),
                Enter => s.push_str("Enter"),
                Backspace => s.push_str("Backspace"),
                Tab => s.push_str("Tab"),
                Space => s.push_str("Space"),
                Insert => s.push_str("Insert"),
                Delete => s.push_str("Delete"),
                Home => s.push_str("Home"),
                End => s.push_str("End"),
                PageUp => s.push_str("PageUp"),
                PageDown => s.push_str("PageDown"),
                Left => s.push_str("Left"),
                Right => s.push_str("Right"),
                Up => s.push_str("Up"),
                Down => s.push_str("Down"),
                Oem(c) => s.push(c),
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_shortcut() {
        assert_eq!(decode(0), DecodedShortcut::Empty);
    }

    #[test]
    fn plain_f1() {
        // VK_F1 = 0x70 без модификаторов.
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
        // 0xE5 (VK_PROCESSKEY) не в таблице — Unsupported, не угадываем.
        assert_eq!(decode(0x00E5), DecodedShortcut::Unsupported { raw: 0x00E5 });
        // Ненулевые неиспользуемые биты 0x0F00 — тоже Unsupported.
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
}
