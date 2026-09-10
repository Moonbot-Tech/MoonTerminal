//! Pure preview/apply logic for "pull hotkey layout from core".
//!
//! [`preview_core_hotkeys`] and [`apply_core_hotkeys`] take no UI/GPUI state — they are plain
//! functions over [`HotkeysConfig`] and [`CoreHotkeyLayout`] so the conflict gate itself (rather
//! than only the decoder and `bound_keys()`) is directly testable: a conflicting core key must be
//! DISPLAYED by `preview_core_hotkeys` but EXCLUDED by `apply_core_hotkeys`, a non-conflicting key
//! must apply only when `apply_core_hotkeys` is actually called (never as a side effect of
//! building the preview), and the model `apply_core_hotkeys` produces must equal the confirmed
//! preview.

use moon_core::config::HotkeysConfig;
use moon_core::config::moonbot_import::shortcut::{self, DecodedShortcut};
use moon_core::feed::{CoreHotkeyAction, CoreHotkeyLayout};

use std::collections::HashSet;

use crate::hotkeys::{BindingId, binding_id, same_binding};

use moon_core::config::KeySlot;

const ORDER_SIZE_SLOTS: usize = moon_core::config::ORDER_SIZE_KEYS;
const SELL_PRESET_SLOTS: usize = moon_core::config::SELL_PRESET_KEYS;
const MANUAL_STRATEGY_SLOTS: usize = moon_core::config::MANUAL_STRATEGY_KEYS;

/// One row of the pull preview: one terminal [`KeySlot`] compared against the core's incoming
/// key for that same slot.
#[derive(Clone)]
pub(super) struct PullRow {
    pub slot: KeySlot,
    /// Terminal's current stored value for `slot` (`gpui::Keystroke::parse` format, "" = unbound).
    pub current: String,
    pub core_decoded: DecodedShortcut,
    /// `core_decoded` converted to the terminal's storage format, or `None` for
    /// [`PullVerdict::Empty`]/[`PullVerdict::Unsupported`] — there is nothing to write.
    pub new_key: Option<String>,
    pub verdict: PullVerdict,
}

/// Shared with [`super::pull_gestures`], whose rows carry a gesture rather than a key: read
/// "value" for "key" below. Two variants behave differently there and that module says why — a
/// zero ordinal is a real value rather than `Empty` (Moonbot lets a trader pick "none"), and
/// `Conflict` is never produced, because one gesture on two rows is ordinary once the mirror
/// switch is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PullVerdict {
    /// The core reports no value for this slot (`core_raw == 0`).
    Empty,
    /// The core's value does not decode to anything this build understands.
    Unsupported,
    /// The core's value already matches the terminal's own.
    Unchanged,
    /// The core's value differs and is free — applying will write it.
    WillApply,
    /// The core's key differs and is already bound elsewhere (another terminal slot, or another
    /// slot in this SAME incoming batch) — shown, but excluded from apply. Keys only.
    Conflict,
}

/// Every [`CoreHotkeyAction`] this terminal has a matching hotkey for. `None` for an action with
/// no Terminal command behind it at all; there is nothing here to preview or apply.
///
/// `MakeShot` maps to `KeySlot::ChartShot`: `chart_shot` needs no protocol command at all,
/// only a way to read the chart's own pixels (`HotkeysConfig`'s own "no command" note,
/// `config/hotkeys.rs`), so it is not commandless the way the remaining discards are. Those
/// genuinely have no Terminal command behind them: Reload Book/Chart, Spy, Show Charts, Fit
/// Sells, Broadcast, Sell +/-, and Make Shot BOT (sending a chart image to a Telegram bot rather
/// than the clipboard, never a Terminal feature to begin with).
fn slot_for_action(action: CoreHotkeyAction) -> Option<KeySlot> {
    use CoreHotkeyAction as A;
    Some(match action {
        A::CancelBuy => KeySlot::CancelBuy,
        A::PanicSell => KeySlot::PanicSell,
        A::PanicSellOne => KeySlot::PanicSellOne,
        A::CancelAllBuys => KeySlot::CancelAllBuys,
        A::JoinSells => KeySlot::JoinSells,
        A::SwitchCharts => KeySlot::SwitchCharts,
        A::NewLong => KeySlot::NewLong,
        A::NewShort => KeySlot::NewShort,
        A::SplitOrder => KeySlot::SplitOrder,
        A::SplitOrderX => KeySlot::SplitOrderX,
        A::ShiftBuyUp => KeySlot::ShiftBuyUp,
        A::ShiftBuyDown => KeySlot::ShiftBuyDown,
        A::ShiftSellUp => KeySlot::ShiftSellUp,
        A::ShiftSellDown => KeySlot::ShiftSellDown,
        A::ScalePlus => KeySlot::ScalePlus,
        A::ScaleMinus => KeySlot::ScaleMinus,
        A::SwitchFigure => KeySlot::SwitchFigure,
        A::MakeShot => KeySlot::ChartShot,
        A::ReloadBook
        | A::MakeShotBot
        | A::ReloadChart
        | A::SellPlus
        | A::SellMinus
        | A::SpyMode
        | A::ShowCharts
        | A::FitSells
        | A::Broadcast => return None,
    })
}

/// Every press the terminal's file already binds, for the conflict gate.
///
/// Built ONCE per preview: a per-row scan re-parsed all ~48 stored keys for each of ~40 rows, which
/// is nineteen hundred parses and twice as many allocations to answer a question with one answer.
fn presses_in_use(hotkeys: &HotkeysConfig) -> HashSet<BindingId> {
    hotkeys
        .bound_keys()
        .iter()
        .filter_map(|held| binding_id(held))
        .collect()
}

fn build_row(
    hotkeys: &HotkeysConfig,
    slot: KeySlot,
    core_raw: u16,
    in_use: &HashSet<BindingId>,
) -> PullRow {
    let current = hotkeys.key(slot).to_string();
    let core_decoded = shortcut::decode(core_raw);
    let new_key = shortcut::to_gpui_keystroke(core_decoded);
    // Compared as PRESSES, never as strings, and the two sides of every comparison below come
    // from different producers: `current` is `Keystroke::unparse`'s spelling of a recorded key (or
    // whatever a hand-edited or pasted `hotkeys.toml` holds), while `new_key` is
    // `shortcut::to_gpui_keystroke`'s. Those two agree since 2026-09-10, but a file written by an
    // older build still carries `ctrl-alt-shift-cmd-k` where this one writes `ctrl-alt-win-shift-k`
    // for the same press, `hotkeys.toml` is hand-editable and pasteable, and `Keystroke::parse` is
    // case-insensitive besides. A literal compare therefore reported "free"
    // for a key another slot already holds and applied it, silently double-binding the terminal:
    // the dispatcher then answers the FIRST branch and the loser dies with nothing said. The
    // settings page's clash captions have compared presses since they were written, so this is also
    // where the pull stopped disagreeing with the page about the same file.
    let verdict = match &new_key {
        None if core_raw == 0 => PullVerdict::Empty,
        None => PullVerdict::Unsupported,
        // The same press already: nothing to write, and nothing worth showing as a change even when
        // the two spell it differently.
        Some(k) if same_binding(k, &current) => PullVerdict::Unchanged,
        Some(k) => {
            // This slot's own key cannot be the match: `same_binding` above already ruled it out.
            // So any binding in the file naming this press belongs to ANOTHER slot — the same
            // reading as `HotkeysConfig::bound_keys()`'s own "a key held by two slots appears
            // twice" doc, with the comparison the dispatcher uses.
            let held_elsewhere = binding_id(k).is_some_and(|id| in_use.contains(&id));
            if held_elsewhere {
                PullVerdict::Conflict
            } else {
                PullVerdict::WillApply
            }
        }
    };
    PullRow {
        slot,
        current,
        core_decoded,
        new_key,
        verdict,
    }
}

/// Builds the full preview: the six order-size slots, the six sell-preset slots, the ten
/// manual-strategy slots, and every named action with a terminal counterpart, each compared
/// against the terminal's CURRENT `hotkeys`.
///
/// Pure and arg-taking — no GPUI, no locking, no I/O — so every property above is directly
/// assertable without constructing any UI state.
///
/// Args:
///     hotkeys: Terminal's current hotkey set to compare against.
///     layout: Core's incoming order-size/sell-preset/named-action keys
///         (`ManualSettings::core_hotkeys`).
///     manual_strategy_keys: Core's incoming manual-strategy slot keys
///         (`ManualSettings::strat_buttons::hot_keys`) — a separate field from `layout` since it
///         travels on a different wire section.
pub(super) fn preview_core_hotkeys(
    hotkeys: &HotkeysConfig,
    layout: &CoreHotkeyLayout,
    manual_strategy_keys: &[u16; MANUAL_STRATEGY_SLOTS],
) -> Vec<PullRow> {
    let mut rows: Vec<PullRow> = Vec::with_capacity(
        ORDER_SIZE_SLOTS + SELL_PRESET_SLOTS + MANUAL_STRATEGY_SLOTS + layout.named.len(),
    );
    let in_use = presses_in_use(hotkeys);
    for i in 0..ORDER_SIZE_SLOTS {
        rows.push(build_row(
            hotkeys,
            KeySlot::OrderSize(i),
            layout.order_size[i],
            &in_use,
        ));
    }
    for i in 0..SELL_PRESET_SLOTS {
        rows.push(build_row(
            hotkeys,
            KeySlot::SellPreset(i),
            layout.sell_preset[i],
            &in_use,
        ));
    }
    for (i, &raw) in manual_strategy_keys.iter().enumerate() {
        rows.push(build_row(hotkeys, KeySlot::ManualStrategy(i), raw, &in_use));
    }
    for &(action, raw) in layout.named.iter() {
        if let Some(slot) = slot_for_action(action) {
            rows.push(build_row(hotkeys, slot, raw, &in_use));
        }
    }

    // A core layout can itself hold the same key twice (two slots both `f1`, say). Checking each
    // row only against the terminal's PRE-EXISTING bindings would let both through as `WillApply`
    // and silently double-bind the terminal on apply — a same-batch collision is a conflict too.
    //
    // Keyed by PRESS, like every other comparison in this module. Both sides here do come from
    // `to_gpui_keystroke`, so strings would agree today — but "today" is a property of one producer,
    // and a second one feeding this batch later would break the only half nothing points at.
    let mut counts: std::collections::HashMap<BindingId, usize> = std::collections::HashMap::new();
    for row in rows.iter().filter(|r| r.verdict == PullVerdict::WillApply) {
        if let Some(id) = row.new_key.as_deref().and_then(binding_id) {
            *counts.entry(id).or_insert(0) += 1;
        }
    }
    for row in rows.iter_mut() {
        if row.verdict == PullVerdict::WillApply
            && let Some(id) = row.new_key.as_deref().and_then(binding_id)
            && counts.get(&id).copied().unwrap_or(0) > 1
        {
            row.verdict = PullVerdict::Conflict;
        }
    }
    rows
}

/// Writes every [`PullVerdict::WillApply`] row's `new_key` into `hotkeys`, leaving every other
/// row (`Empty`, `Unsupported`, `Unchanged`, `Conflict`) untouched. Returns whether anything
/// actually changed, so the caller knows whether a save is needed at all.
///
/// Called ONLY from the confirm action — never from [`preview_core_hotkeys`] itself, which is
/// what makes "cancellation writes nothing" true by construction: a preview that is built and
/// then discarded never reaches this function.
pub(super) fn apply_core_hotkeys(hotkeys: &mut HotkeysConfig, rows: &[PullRow]) -> bool {
    let mut changed = false;
    for row in rows {
        if row.verdict != PullVerdict::WillApply {
            continue;
        }
        if let Some(new_key) = row.new_key.clone() {
            changed |= hotkeys.set_key(row.slot, new_key);
        }
    }
    changed
}

#[cfg(test)]
mod tests;
