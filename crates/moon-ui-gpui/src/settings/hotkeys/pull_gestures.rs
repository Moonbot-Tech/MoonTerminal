//! The gesture half of "pull layout from core", beside [`super::pull`]'s keyboard half.
//!
//! One direction only, like the keys: the core's layout is read and written into the terminal's
//! single local set. Nothing here sends anything to a core — the only code that writes gestures
//! back is the Core Expert window, whose job is editing Moonbot's own settings, and a terminal that
//! serves twenty-six cores has one layout, not twenty-six.
//!
//! Pure and arg-taking for the same reason the keyboard half is: the decode, the verdicts and the
//! apply are testable without a window, and building a preview can have no side effect on the
//! draft.
//!
//! Two facts of the wire shape this file, and both come from `feed::GestureSettings`:
//!
//! - A gesture and a move kind travel as an ORDINAL into the terminal's own list —
//!   `MouseGestureBinding::ALL` and `MoveKind::ALL` are declared in Moonbot's order, which is what
//!   `core_expert::pages::hotkeys` already relies on to draw the core's own values. Anything past
//!   the end of either list is a core we do not understand, and it is refused rather than guessed.
//! - With `same_hotkeys_for_move` set, a SHORT field is not what fires — the long one is, and the
//!   short field may hold something stale. That is true on both sides of every short row, so the
//!   core side is read through `GestureSettings::move_gesture` and the local side through
//!   `HotkeysConfig::move_gestures`, never off either field. A preview that stated a binding
//!   neither end actually fires would be worse than no preview.

use moon_core::config::{GestureSlot, HotkeysConfig, MouseGestureBinding, MoveKind, MoveKindSlot};
use moon_core::feed::{GestureSettings, MoveRow};
use rust_i18n::t;

use super::pull::PullVerdict;
use super::registry;

/// What one gesture row of the preview writes into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum GestureTarget {
    /// One of the twelve order gestures.
    Gesture(GestureSlot),
    /// One of the four "move kind" selectors.
    Kind(MoveKindSlot),
    /// The switch that makes the short columns follow the long ones.
    SameForMove,
}

/// A decoded incoming value, kept typed so the apply cannot write it to the wrong field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum GestureValue {
    Gesture(MouseGestureBinding),
    Kind(MoveKind),
    Flag(bool),
}

/// One row of the gesture preview: one local field against what the core holds for it.
#[derive(Clone)]
pub(super) struct GesturePullRow {
    pub target: GestureTarget,
    /// The local value as the row shows it.
    pub current: String,
    /// The core's value as the row shows it, or a dash when there is nothing to show.
    pub incoming: String,
    /// The decoded value to write, or `None` when this row is not applying anything.
    pub new: Option<GestureValue>,
    /// What the core holds, decoded, whatever the verdict — kept because the mirror switch has to
    /// re-aim the four short rows from it even where they scored `Unchanged`.
    pub incoming_value: Option<GestureValue>,
    pub verdict: PullVerdict,
}

/// The twelve gestures, each with the core field or resolver that answers for it.
///
/// The move rows go through [`GestureSettings::move_gesture`] rather than reading a field, which is
/// the whole reason this is a function and not a table of field offsets.
fn core_gesture(g: &GestureSettings, slot: GestureSlot) -> Option<u8> {
    if let Some(half) = slot.move_half() {
        return Some(g.move_gesture(wire_row(half.row), half.short));
    }
    Some(match slot {
        GestureSlot::BuySet => g.buy_set_click,
        GestureSlot::ShortSet => g.short_set_click,
        GestureSlot::PendingLong => g.pending_order_set_click,
        GestureSlot::PendingShort => g.pending_short_set_click,
        // Deleting a figure by pointing is the Terminal's own: `GestureSettings` carries the twelve
        // order gestures and no figure gesture at all, so a pull has nothing to say about this row.
        GestureSlot::FigDelete => return None,
        // Every move half was answered above; the arm is here so a new variant has to say which
        // side of the wire it is on. Should `move_half` ever stop covering one of these, the row
        // drops out of the pull — and `registry::tests` fails, because its mark still says the
        // pull writes it.
        GestureSlot::BuyMove
        | GestureSlot::SellMove
        | GestureSlot::BuyMove2
        | GestureSlot::SellMove2
        | GestureSlot::ShortBuyMove
        | GestureSlot::ShortSellMove
        | GestureSlot::ShortBuyMove2
        | GestureSlot::ShortSellMove2 => return None,
    })
}

/// The wire's row for one of the terminal's move rows — the one place the two numberings meet.
fn wire_row(row: MoveKindSlot) -> MoveRow {
    match row {
        MoveKindSlot::BuyMove => MoveRow::OpenPrimary,
        MoveKindSlot::SellMove => MoveRow::TpPrimary,
        MoveKindSlot::BuyMove2 => MoveRow::OpenSecondary,
        MoveKindSlot::SellMove2 => MoveRow::TpSecondary,
    }
}

fn core_kind(g: &GestureSettings, slot: MoveKindSlot) -> u8 {
    match slot {
        MoveKindSlot::BuyMove => g.replace_buy_kind,
        MoveKindSlot::SellMove => g.replace_sell_kind,
        MoveKindSlot::BuyMove2 => g.replace_buy_kind_2,
        MoveKindSlot::SellMove2 => g.replace_sell_kind_2,
    }
}

/// Decodes a wire ordinal into a gesture, refusing anything past the end of the list.
fn decode_gesture(raw: u8) -> Option<MouseGestureBinding> {
    MouseGestureBinding::ALL.get(raw as usize).copied()
}

fn decode_kind(raw: u8) -> Option<MoveKind> {
    MoveKind::ALL.get(raw as usize).copied()
}

fn kind_label(kind: MoveKind) -> String {
    let key = kind.locale_key();
    t!(key.as_str()).to_string()
}

fn flag_label(on: bool) -> String {
    if on {
        t!("hotkeys.pull.flag_on")
    } else {
        t!("hotkeys.pull.flag_off")
    }
    .to_string()
}

/// Builds one gesture or kind row, given the local value and the core's raw ordinal.
///
/// A zero ordinal is a VALUE here, unlike the keyboard half where it means "no key assigned".
///
/// `None_Click` and `RM_None` are entries a trader picks in Moonbot's own dialog, so zero says
/// "recognise nothing" rather than "never configured" — and the terminal never sees an unfilled
/// block anyway: `core_config` is `Some` only once a real projection has arrived, so there is no
/// synthetic default to mistake for a choice.
///
/// The tempting rule — "skip zero, it is probably just unset" — was tried and is wrong twice over.
/// It cannot be applied per field either: moonproto's own defaults are `replace_buy_kind: 3` and
/// `replace_sell_kind: 2` but `replace_buy_kind_2: 0`, so "is zero the default" differs between two
/// fields of the same block. And skipping it silently keeps a local gesture the core has switched
/// off, which is the opposite of what pulling a LAYOUT means.
fn build_row(
    target: GestureTarget,
    current: GestureValue,
    current_label: String,
    incoming: Option<GestureValue>,
) -> GesturePullRow {
    let (incoming_label, verdict, new) = match incoming {
        None => ("—".to_string(), PullVerdict::Unsupported, None),
        Some(value) => {
            let label = match value {
                GestureValue::Gesture(g) => g.menu_label(),
                GestureValue::Kind(k) => kind_label(k),
                GestureValue::Flag(on) => flag_label(on),
            };
            if value == current {
                (label, PullVerdict::Unchanged, None)
            } else {
                (label, PullVerdict::WillApply, Some(value))
            }
        }
    };
    GesturePullRow {
        target,
        current: current_label,
        incoming: incoming_label,
        new,
        incoming_value: incoming,
        verdict,
    }
}

/// Every gesture, kind and the mirror flag, compared against the terminal's current set.
///
/// [`PullVerdict::Conflict`] is never produced here, and that is not an oversight: one gesture on
/// two rows is ordinary in Moonbot — the short rows hold the long one's value whenever the mirror
/// flag is set — so a duplicate check would fire on every healthy layout. Shadowing between
/// gestures is the conflict indicator's job, and it needs the scope model to say anything true.
pub(super) fn preview_core_gestures(
    hotkeys: &HotkeysConfig,
    g: &GestureSettings,
) -> Vec<GesturePullRow> {
    let mut rows = Vec::with_capacity(17);
    for slot in GestureSlot::ALL {
        let Some(raw) = core_gesture(g, slot) else {
            continue;
        };
        let current = hotkeys.gesture_in_effect(slot);
        rows.push(build_row(
            GestureTarget::Gesture(slot),
            GestureValue::Gesture(current),
            current.menu_label(),
            decode_gesture(raw).map(GestureValue::Gesture),
        ));
    }
    for slot in MoveKindSlot::ALL {
        let current = hotkeys.move_kind(slot);
        rows.push(build_row(
            GestureTarget::Kind(slot),
            GestureValue::Kind(current),
            kind_label(current),
            decode_kind(core_kind(g, slot)).map(GestureValue::Kind),
        ));
    }
    let current = hotkeys.same_hotkeys_for_move;
    rows.push(build_row(
        GestureTarget::SameForMove,
        GestureValue::Flag(current),
        flag_label(current),
        Some(GestureValue::Flag(g.same_hotkeys_for_move)),
    ));
    rows
}

/// Writes every [`PullVerdict::WillApply`] row, leaving every other row untouched.
///
/// Gestures are written VERBATIM (`HotkeysConfig::set_gesture`) rather than through the editor's
/// setter. The editor mirrors a long row onto its short twin while the mirror flag is set, and here
/// that would overwrite a short value this very preview decided to leave alone — a local flag set
/// over a core that has it clear is all it takes. A layout transfer copies what the core holds.
///
/// The flag row is applied LAST because the short-row repair below depends on knowing the switch
/// moved. The gesture writes themselves do not care about the order — every one of them goes
/// through `HotkeysConfig::set_gesture`, which the flag has no say in.
///
/// A change to the mirror flag ALSO re-aims the four short rows from what the core holds, whatever
/// their own verdict was. Both directions need it, and neither is repairable by pulling again:
///
/// - turning it ON, the short field stops being read (`HotkeysConfig::move_gestures` answers with
///   the long one) and its dropdown greys out, so a divergent value would sit there unreachable;
/// - turning it OFF, a short field that was being ignored becomes live again — and its row can
///   easily read `Unchanged`, because with the flag still on its "current" is the LONG value, which
///   may well equal what the core holds. The stale field would then fire.
///
/// A second pull cannot fix either: the rows score `Unchanged` again.
pub(super) fn apply_core_gestures(hotkeys: &mut HotkeysConfig, rows: &[GesturePullRow]) -> bool {
    let mut changed = false;
    let mut flag: Option<bool> = None;
    for row in rows {
        if row.verdict != PullVerdict::WillApply {
            continue;
        }
        match (row.target, row.new) {
            (GestureTarget::Gesture(slot), Some(GestureValue::Gesture(value))) => {
                changed |= hotkeys.set_gesture(slot, value);
            }
            (GestureTarget::Kind(slot), Some(GestureValue::Kind(value))) => {
                changed |= hotkeys.set_move_kind(slot, value);
            }
            (GestureTarget::SameForMove, Some(GestureValue::Flag(value))) => flag = Some(value),
            // A row whose target and value disagree cannot be built by `preview_core_gestures`;
            // ignoring it keeps a hand-made row from writing to the wrong field.
            _ => {}
        }
    }
    if let Some(value) = flag
        && hotkeys.same_hotkeys_for_move != value
    {
        hotkeys.same_hotkeys_for_move = value;
        changed = true;
        for row in rows {
            let (GestureTarget::Gesture(slot), Some(GestureValue::Gesture(incoming))) =
                (row.target, row.incoming_value)
            else {
                continue;
            };
            if slot.is_short_move() {
                changed |= hotkeys.set_gesture(slot, incoming);
            }
        }
    }
    changed
}

/// The row's own label, matching the editor row it will change.
///
/// A move kind has no title of its own in the editor — it is the second control on its gesture's
/// row — so it is named here as that row plus the column's name, which is how the trader sees it.
pub(super) fn target_label(target: GestureTarget) -> String {
    match target {
        GestureTarget::Gesture(slot) => registry::gesture_title(slot),
        GestureTarget::Kind(row) => format!(
            "{} · {}",
            registry::gesture_title(row.half(false)),
            t!("hotkeys.move_kind.title")
        ),
        GestureTarget::SameForMove => t!("hotkeys.mouse.same_move").to_string(),
    }
}

#[cfg(test)]
mod tests;
