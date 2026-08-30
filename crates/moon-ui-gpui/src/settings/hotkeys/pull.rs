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

use super::{HotkeySlot, set_slot_value, slot_value};

const ORDER_SIZE_SLOTS: usize = moon_core::config::ORDER_SIZE_KEYS;
const SELL_PRESET_SLOTS: usize = moon_core::config::SELL_PRESET_KEYS;
const MANUAL_STRATEGY_SLOTS: usize = moon_core::config::MANUAL_STRATEGY_KEYS;

/// One row of the pull preview: one terminal [`HotkeySlot`] compared against the core's incoming
/// key for that same slot.
#[derive(Clone)]
pub(super) struct PullRow {
    pub slot: HotkeySlot,
    /// Terminal's current stored value for `slot` (`gpui::Keystroke::parse` format, "" = unbound).
    pub current: String,
    pub core_decoded: DecodedShortcut,
    /// `core_decoded` converted to the terminal's storage format, or `None` for
    /// [`PullVerdict::Empty`]/[`PullVerdict::Unsupported`] — there is nothing to write.
    pub new_key: Option<String>,
    pub verdict: PullVerdict,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PullVerdict {
    /// The core reports no key for this slot (`core_raw == 0`).
    Empty,
    /// The core's key does not decode to anything `gpui::Keystroke::parse` accepts.
    Unsupported,
    /// The core's key already matches the terminal's own binding for this slot.
    Unchanged,
    /// The core's key differs and is free — applying will write it.
    WillApply,
    /// The core's key differs and is already bound elsewhere (another terminal slot, or another
    /// slot in this SAME incoming batch) — shown, but excluded from apply.
    Conflict,
}

/// Every [`CoreHotkeyAction`] this terminal has a matching hotkey for. `None` for an action with
/// no Terminal command behind it at all; there is nothing here to preview or apply.
///
/// `MakeShot` maps to `HotkeySlot::ChartShot`: `chart_shot` needs no protocol command at all,
/// only a way to read the chart's own pixels (`HotkeysConfig`'s own "no command" note,
/// `config/hotkeys.rs`), so it is not commandless the way the remaining discards are. Those
/// genuinely have no Terminal command behind them: Reload Book/Chart, Spy, Show Charts, Fit
/// Sells, Broadcast, Sell +/-, and Make Shot BOT (sending a chart image to a Telegram bot rather
/// than the clipboard, never a Terminal feature to begin with).
fn slot_for_action(action: CoreHotkeyAction) -> Option<HotkeySlot> {
    use CoreHotkeyAction as A;
    Some(match action {
        A::CancelBuy => HotkeySlot::CancelBuy,
        A::PanicSell => HotkeySlot::PanicSell,
        A::PanicSellOne => HotkeySlot::PanicSellOne,
        A::CancelAllBuys => HotkeySlot::CancelAllBuys,
        A::JoinSells => HotkeySlot::JoinSells,
        A::SwitchCharts => HotkeySlot::SwitchCharts,
        A::NewLong => HotkeySlot::NewLong,
        A::NewShort => HotkeySlot::NewShort,
        A::SplitOrder => HotkeySlot::SplitOrder,
        A::SplitOrderX => HotkeySlot::SplitOrderX,
        A::ShiftBuyUp => HotkeySlot::ShiftBuyUp,
        A::ShiftBuyDown => HotkeySlot::ShiftBuyDown,
        A::ShiftSellUp => HotkeySlot::ShiftSellUp,
        A::ShiftSellDown => HotkeySlot::ShiftSellDown,
        A::ScalePlus => HotkeySlot::ScalePlus,
        A::ScaleMinus => HotkeySlot::ScaleMinus,
        A::SwitchFigure => HotkeySlot::SwitchFigure,
        A::MakeShot => HotkeySlot::ChartShot,
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

fn build_row(hotkeys: &HotkeysConfig, slot: HotkeySlot, core_raw: u16) -> PullRow {
    let current = slot_value(hotkeys, slot).to_string();
    let core_decoded = shortcut::decode(core_raw);
    let new_key = shortcut::to_gpui_keystroke(core_decoded);
    let verdict = match &new_key {
        None if core_raw == 0 => PullVerdict::Empty,
        None => PullVerdict::Unsupported,
        Some(k) if *k == current => PullVerdict::Unchanged,
        Some(k) => {
            // `current` (this slot's own key) is excluded by construction since `k != current`
            // here, so ANY occurrence in `bound_keys()` means another slot already holds it —
            // matching `HotkeysConfig::bound_keys()`'s own "a key held by two slots appears
            // twice" doc.
            let held_elsewhere = hotkeys.bound_keys().iter().any(|held| held == k);
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
    for i in 0..ORDER_SIZE_SLOTS {
        rows.push(build_row(
            hotkeys,
            HotkeySlot::OrderSize(i),
            layout.order_size[i],
        ));
    }
    for i in 0..SELL_PRESET_SLOTS {
        rows.push(build_row(
            hotkeys,
            HotkeySlot::SellPreset(i),
            layout.sell_preset[i],
        ));
    }
    for (i, &raw) in manual_strategy_keys.iter().enumerate() {
        rows.push(build_row(hotkeys, HotkeySlot::ManualStrategy(i), raw));
    }
    for &(action, raw) in layout.named.iter() {
        if let Some(slot) = slot_for_action(action) {
            rows.push(build_row(hotkeys, slot, raw));
        }
    }

    // A core layout can itself hold the same key twice (two slots both `f1`, say). Checking each
    // row only against the terminal's PRE-EXISTING bindings would let both through as `WillApply`
    // and silently double-bind the terminal on apply — a same-batch collision is a conflict too.
    // Owned `String` keys, not `&str` borrowed from `rows`: the lookup below needs `rows.iter_mut()`
    // at the same time, and a map borrowing from `rows` would conflict with that mutable pass.
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in rows.iter().filter(|r| r.verdict == PullVerdict::WillApply) {
        if let Some(k) = row.new_key.as_ref() {
            *counts.entry(k.clone()).or_insert(0) += 1;
        }
    }
    for row in rows.iter_mut() {
        if row.verdict == PullVerdict::WillApply
            && let Some(k) = row.new_key.as_ref()
            && counts.get(k).copied().unwrap_or(0) > 1
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
            changed |= set_slot_value(hotkeys, row.slot, new_key);
        }
    }
    changed
}
