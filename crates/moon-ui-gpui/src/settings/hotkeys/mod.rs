//! Hotkeys tab: a Moonbot-compatible hotkey set organized by workflow.
//!
//! The slots themselves live beside the data they address — `moon_core::config::KeySlot`,
//! `GestureSlot` and `MoveKindSlot`, with the accessors on `HotkeysConfig` — and what each slot IS
//! lives beside the dispatcher (`crate::hotkeys::meta`). This page owns only what is the page's:
//! [`registry`] says which rows it shows and in what order, [`tab`] draws them, [`clash`] captions
//! the ones that collide, and [`pull`] and [`pull_gestures`] hold the pure preview/apply logic
//! behind the "pull layout from core" button — the keys and the mouse gestures respectively.

mod clash;
mod pull;
mod pull_gestures;
mod registry;
mod tab;

use moon_core::config::{GestureSlot, HotkeysConfig, MouseGestureBinding};

pub(in crate::settings) use registry::HotkeyGroup;

/// Writes one gesture the way the EDITOR must: carrying the mirror "same for move" demands.
///
/// The mirror is Moonbot's own behaviour and the reason the short rows are greyed out while the
/// flag is on. `HotkeysConfig::set_gesture` is the other half — it writes one field and nothing
/// else, which is what a layout transfer needs: a pull carries long rows and short rows together,
/// and mirroring the long write would overwrite a short value the preview had already decided to
/// leave alone.
fn set_gesture_mirrored(
    hotkeys: &mut HotkeysConfig,
    slot: GestureSlot,
    value: MouseGestureBinding,
) -> bool {
    let mut changed = hotkeys.set_gesture(slot, value);
    if hotkeys.same_hotkeys_for_move
        && let Some(twin) = slot.short_twin()
    {
        changed |= hotkeys.set_gesture(twin, value);
    }
    changed
}
