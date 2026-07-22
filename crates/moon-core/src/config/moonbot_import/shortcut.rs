//! Decoder for Delphi `TShortCut` (u16): high bits are modifiers and the low byte is a
//! Windows virtual-key code. Only EXPLICITLY mapped known VK values are accepted; an unknown
//! code becomes [`DecodedShortcut::Unsupported`] rather than a guessed GPUI string (spec section 6).

/// Command modifier bit, a Delphi macOS legacy value not found in Windows configurations.
pub const MOD_CMD: u16 = 0x1000;
pub const MOD_SHIFT: u16 = 0x2000;
pub const MOD_CTRL: u16 = 0x4000;
pub const MOD_ALT: u16 = 0x8000;

/// Shortcut modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShortcutMods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub cmd: bool,
}

/// Explicitly supported key, using the US layout for OEM symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutKey {
    /// Uppercase `A`–`Z` or top-row `0`–`9`.
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
    /// OEM key represented by its US-layout symbol: `;` `=` `,` `-` `.` `/` `` ` `` `[` `\` `]` `'`.
    Oem(char),
}

/// Decoding result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedShortcut {
    /// A zero value means no shortcut is assigned.
    Empty,
    /// Known key with modifiers.
    Key {
        mods: ShortcutMods,
        key: ShortcutKey,
    },
    /// Unknown VK or unused bits; cannot be imported and is displayed as-is.
    Unsupported { raw: u16 },
}

/// Decodes a raw `TShortCut` value.
pub fn decode(raw: u16) -> DecodedShortcut {
    if raw == 0 {
        return DecodedShortcut::Empty;
    }
    // Bits 0x0F00 in the lower portion of TShortCut are unused; nonzero means another format.
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

/// Explicit Windows VK → key table. Anything absent is unsupported.
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
        0x30..=0x39 => Char(vk as char), // For '0'..'9', VK matches ASCII.
        0x41..=0x5A => Char(vk as char), // For 'A'..'Z', VK matches ASCII.
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

/// Converts to the `gpui::Keystroke::parse` format (`ctrl-shift-f7`, `alt-d`) stored by
/// `HotkeysConfig`. Returns `None` for empty/unsupported values because there is nothing to import.
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

/// Returns a human-readable preview label: `Ctrl+Shift+F7`, `Alt+D`, `—` for empty, or
/// `VK 0xE5?` for unsupported.
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
mod tests;
