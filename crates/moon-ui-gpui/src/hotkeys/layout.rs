//! Layout-independent letter keys for hotkey matching.
//!
//! On Windows the platform names a letter key by the character the ACTIVE layout puts on it:
//! `moon-gpui-windows` builds `Keystroke.key` through `MapVirtualKeyW(vkey, MAPVK_VK_TO_CHAR)`
//! (`keyboard.rs`), so with a Russian layout the `A` key arrives as `"ф"` and `S` as `"ы"`. A
//! hotkey stored as `alt-z` would then be dead in Cyrillic and alive in Latin — while Moonbot,
//! which compares Delphi virtual-key codes, works in both. The scan code never reaches us, so the
//! physical key is recovered here instead: the character is asked back through the same layout for
//! the virtual key that produces it, and letter virtual keys ARE the ASCII uppercase codes
//! (`VK_A == 0x41`), which makes the answer the US letter by definition.
//!
//! macOS needs none of this — its backend already switches to the command layout for a non-Latin
//! one — and neither do function keys, digits, arrows or `delete`, which never carry a letter.

/// Return the US-layout letter a non-ASCII key name stands for, or `None` when there is nothing to
/// translate (an ASCII name, a multi-character name like `f7`, or a symbol that is not a letter).
#[cfg(target_os = "windows")]
pub(crate) fn us_letter(key: &str) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, VkKeyScanExW};

    let mut chars = key.chars();
    let ch = chars.next()?;
    // Only a single non-ASCII character can be a Cyrillic (or other) letter standing in for a
    // Latin one. Everything else already matches, or is not a letter key at all.
    if chars.next().is_some() || ch.is_ascii() {
        return None;
    }
    // Layout of THIS thread, which is the UI thread the key arrived on.
    let layout = unsafe { GetKeyboardLayout(0) };
    let scan = unsafe { VkKeyScanExW(ch as u16, layout) };
    if scan == -1 {
        return None;
    }
    let vkey = (scan as u16 & 0xFF) as u8;
    (0x41..=0x5A)
        .contains(&vkey)
        .then(|| char::from(vkey).to_ascii_lowercase().to_string())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn us_letter(_key: &str) -> Option<String> {
    None
}
