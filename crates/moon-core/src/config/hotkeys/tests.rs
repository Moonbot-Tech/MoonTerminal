use super::*;

/// Pins the one-time backfill of slots that shipped unbound.
///
/// Plausible breakage: running the fill on every load hands back a hotkey the user deliberately
/// cleared on the next launch; running it never leaves everyone who already has a `hotkeys.toml`
/// without the Moonbot keys this build ships.
#[test]
fn unbound_slots_are_filled_once_and_user_choices_survive() {
    let shipped = HotkeysConfig::default();
    // A file from the build before these defaults: no generation, empty slots, and one key the
    // user chose for themselves.
    let mut old = HotkeysConfig {
        schema: 0,
        cancel_buy: String::new(),
        join_sells: String::new(),
        sells_to_rect: String::new(),
        new_long: "f9".into(),
        ..shipped.clone()
    };
    old.fill_unbound_slots();
    assert_eq!(old.cancel_buy, shipped.cancel_buy);
    assert_eq!(old.join_sells, shipped.join_sells);
    assert_eq!(old.sells_to_rect, shipped.sells_to_rect);
    assert_eq!(
        old.new_long, "f9",
        "a key the user set is never overwritten"
    );
    assert_eq!(old.schema, SCHEMA);

    // Clearing a slot after the migration ran must STAY cleared across loads.
    old.cancel_buy = String::new();
    old.fill_unbound_slots();
    assert!(old.cancel_buy.is_empty());
}

/// Pins `hotkeys.rs::clear_generation_2_collisions` against an off-by-one duplicate threshold.
///
/// Plausible breakage: treating the two holders of Ctrl+F10 as non-colliding leaves the new chart
/// shot above the user's existing panic-sell binding, so that keystroke captures a chart instead
/// of sending the order it used to send.
#[test]
fn generation_2_yields_chart_shot_to_an_existing_binding() {
    let mut existing_file = HotkeysConfig {
        schema: 1,
        panic_sell: "ctrl-f10".into(),
        ..HotkeysConfig::default()
    };

    existing_file.fill_unbound_slots();

    assert_eq!(existing_file.panic_sell, "ctrl-f10");
    assert!(
        existing_file.chart_shot.is_empty(),
        "the new chart-shot default must yield to the user's existing binding"
    );
}

/// Pins `hotkeys.rs::clear_generation_3_collisions`: the arriving Ctrl+Z default must yield to a
/// user who had already given that keystroke away.
///
/// Plausible breakage: the figure layer resolves ABOVE the trading actions, so a duplicated Ctrl+Z
/// would silently turn an order-sending key into "delete the last figure" — and the file's own
/// generation gate would never look at it again.
#[test]
fn generation_3_yields_fig_undo_to_an_existing_binding() {
    let mut existing_file = HotkeysConfig {
        schema: 2,
        new_long: "ctrl-z".into(),
        ..HotkeysConfig::default()
    };

    existing_file.fill_unbound_slots();

    assert_eq!(existing_file.new_long, "ctrl-z");
    assert!(
        existing_file.fig_undo.is_empty(),
        "the new figure-undo default must yield to the user's existing binding"
    );
    assert_eq!(existing_file.schema, SCHEMA);
}

/// A file that has NOT given Ctrl+Z away keeps the shipped default, so the feature is not
/// switched off for everybody by the collision check that exists for the few.
#[test]
fn generation_3_keeps_fig_undo_on_a_file_that_never_used_it() {
    let mut existing_file = HotkeysConfig {
        schema: 2,
        ..HotkeysConfig::default()
    };

    existing_file.fill_unbound_slots();

    assert_eq!(existing_file.fig_undo, "ctrl-z");
}

/// Every tool takes part in the switch-figure cycle until one is switched off, including in a file
/// written before the exclusion list existed.
///
/// Plausible breakage: shipping an INCLUSION list would read an old file — and a fresh install —
/// as "no tool participates", which is a hotkey that silently does nothing.
#[test]
fn no_tool_is_excluded_from_the_cycle_by_default() {
    assert!(HotkeysConfig::default().switch_figure_skip.is_empty());
    let old_file: HotkeysConfig =
        toml::from_str("").expect("a hotkeys.toml with no keys at all must still load");
    assert!(old_file.switch_figure_skip.is_empty());
}

/// Pins `hotkeys.rs::fill_unbound_slots` so generation 1 cannot re-run on a generation-1 file.
///
/// Plausible breakage: collapsing the generation gates restores a deliberately cleared Cancel Buy
/// key while upgrading the chart-shot slot, so a user can accidentally send an order they disabled.
#[test]
fn generation_2_preserves_slots_deliberately_cleared_after_generation_1() {
    let mut existing_file = HotkeysConfig {
        schema: 1,
        cancel_buy: String::new(),
        panic_sell: "ctrl-f10".into(),
        ..HotkeysConfig::default()
    };

    existing_file.fill_unbound_slots();

    assert!(
        existing_file.cancel_buy.is_empty(),
        "generation 1 must not restore a key cleared after its migration"
    );
    assert!(
        existing_file.chart_shot.is_empty(),
        "generation 2 must still clear the chart-shot collision"
    );
}

/// Pins that the shipped keyboard defaults are Moonbot's own, so a user switching over finds
/// their keys where they left them.
///
/// Plausible breakage: an invented default silently diverges from Moonbot and nothing says so.
#[test]
fn keyboard_defaults_match_moonbot() {
    let h = HotkeysConfig::default();
    for (actual, expected, name) in [
        (&h.cancel_buy, "alt-z", "Cancel Buy"),
        (&h.panic_sell, "alt-6", "Panic Sell"),
        (&h.panic_sell_one, "alt-5", "Panic Sell 1 order"),
        (&h.cancel_all_buys, "alt-a", "Cancel ALL buys"),
        (&h.join_sells, "alt-e", "Объединить Sell"),
        (&h.switch_charts, "alt-f", "Переключ. графиков"),
        (&h.switch_figure, "alt-d", "Switch Chart Figure"),
        (&h.new_long, "alt-1", "New Long"),
        (&h.new_short, "alt-3", "New Short"),
        (&h.split_order, "alt-c", "Split Order"),
        (&h.split_order_x, "ctrl-x", "Split to N"),
        (&h.sells_to_rect, "ctrl-s", "Sells to rectangle"),
        (&h.shift_buy_up, "shift-up", "Shift buys +1%"),
        (&h.shift_buy_down, "shift-down", "Shift buys -1%"),
        (&h.shift_sell_up, "alt-up", "Shift sells +1%"),
        (&h.shift_sell_down, "alt-down", "Shift sells -1%"),
        (&h.scale_plus, "ctrl-q", "Scale +"),
        (&h.scale_minus, "ctrl-w", "Scale -"),
    ] {
        assert_eq!(actual, expected, "{name} must ship Moonbot's key");
    }
}

/// Pins the order-shift step as WHOLE percent, which is the unit the protocol takes everywhere.
///
/// Plausible breakage: reading Moonbot's "±1%" as a fraction and shipping 0.01 would move live
/// orders by a hundredth of a percent, which looks like the hotkey doing nothing; shipping 100.0
/// would move them by a factor of two. Neither is visible in a type.
#[test]
fn order_shift_is_one_whole_percent() {
    assert_eq!(SHIFT_PERCENT, 1.0);
}

/// Pins the `Split N` count against a hand-edited or imported file.
///
/// Plausible breakage: reading the raw field sends a live split into one part, or into 200.
#[test]
fn split_n_parts_stays_inside_its_range() {
    let clamp = |split_parts| {
        HotkeysConfig {
            split_parts,
            ..HotkeysConfig::default()
        }
        .split_n_parts()
    };
    assert_eq!(clamp(0), i32::from(SPLIT_PARTS_MIN));
    assert_eq!(clamp(1), i32::from(SPLIT_PARTS_MIN));
    assert_eq!(clamp(4), 4);
    assert_eq!(clamp(200), i32::from(SPLIT_PARTS_MAX));
}

/// Pins which command a recognised move gesture becomes.
///
/// Plausible breakage: reading the slot's twin — the entry kind for an exit gesture, the primary
/// kind for the secondary slot. Both send a live bulk move built from the wrong half of the
/// settings page, and the orders that come back are not the ones the user was looking at.
#[test]
fn a_move_gesture_resolves_to_its_own_side_and_kind() {
    let hk = HotkeysConfig {
        buy_move_click: MouseGestureBinding::LeftShift,
        sell_move_click: MouseGestureBinding::LeftCtrl,
        buy_move_click2: MouseGestureBinding::MiddleShift,
        sell_move_click2: MouseGestureBinding::MiddleCtrl,
        buy_move_kind: MoveKind::AllToOnePrice,
        sell_move_kind: MoveKind::ParallelShift,
        buy_move_kind2: MoveKind::TopVolume,
        sell_move_kind2: MoveKind::LastMoved,
        same_hotkeys_for_move: false,
        short_buy_move_click: MouseGestureBinding::None,
        short_sell_move_click: MouseGestureBinding::None,
        short_buy_move_click2: MouseGestureBinding::None,
        short_sell_move_click2: MouseGestureBinding::None,
        ..HotkeysConfig::default()
    };
    let resolve = |pressed: MouseGestureBinding| hk.resolve_move_gesture(|b| b == pressed);

    assert_eq!(
        resolve(MouseGestureBinding::LeftShift),
        Some(MoveGestureCommand {
            sell: false,
            kind: MoveKind::AllToOnePrice,
            side: MoveSide::Long,
        })
    );
    assert_eq!(
        resolve(MouseGestureBinding::LeftCtrl),
        Some(MoveGestureCommand {
            sell: true,
            kind: MoveKind::ParallelShift,
            side: MoveSide::Long,
        })
    );
    assert_eq!(
        resolve(MouseGestureBinding::MiddleShift),
        Some(MoveGestureCommand {
            sell: false,
            kind: MoveKind::TopVolume,
            side: MoveSide::Long,
        })
    );
    assert_eq!(
        resolve(MouseGestureBinding::MiddleCtrl),
        Some(MoveGestureCommand {
            sell: true,
            kind: MoveKind::LastMoved,
            side: MoveSide::Long,
        })
    );
    assert_eq!(resolve(MouseGestureBinding::LeftAlt), None);
}

/// Pins the side a press addresses, which is what the mirror flag decides.
///
/// Plausible breakage: sending `Long` for a gesture the user shares between both sides. On a hedged
/// market that moves one side's orders and silently leaves the other where it was, which reads as
/// "half the command worked".
#[test]
fn a_shared_gesture_addresses_both_sides() {
    let mirrored = HotkeysConfig {
        sell_move_click: MouseGestureBinding::LeftCtrl,
        same_hotkeys_for_move: true,
        ..HotkeysConfig::default()
    };
    assert_eq!(
        mirrored
            .resolve_move_gesture(|b| b == MouseGestureBinding::LeftCtrl)
            .map(|c| c.side),
        Some(MoveSide::Both),
        "the mirror makes one press mean both sides"
    );

    let split = HotkeysConfig {
        sell_move_click: MouseGestureBinding::LeftCtrl,
        short_sell_move_click: MouseGestureBinding::LeftAlt,
        same_hotkeys_for_move: false,
        ..HotkeysConfig::default()
    };
    assert_eq!(
        split
            .resolve_move_gesture(|b| b == MouseGestureBinding::LeftCtrl)
            .map(|c| c.side),
        Some(MoveSide::Long)
    );
    assert_eq!(
        split
            .resolve_move_gesture(|b| b == MouseGestureBinding::LeftAlt)
            .map(|c| c.side),
        Some(MoveSide::Short)
    );
}

/// Pins that a bound gesture with no kind sends nothing.
///
/// Plausible breakage: treating `MoveKind::None` as a kind and putting it on the wire. Moonbot uses
/// it to switch a gesture off without clearing the binding, and the core has no arrangement to
/// apply — so what a "none" move actually did to live orders would be anyone's guess.
#[test]
fn a_gesture_without_a_kind_is_inert() {
    let hk = HotkeysConfig {
        sell_move_click: MouseGestureBinding::LeftCtrl,
        sell_move_kind: MoveKind::None,
        same_hotkeys_for_move: true,
        ..HotkeysConfig::default()
    };
    assert_eq!(
        hk.resolve_move_gesture(|b| b == MouseGestureBinding::LeftCtrl),
        None
    );

    // And an inert slot must not shadow a later one holding the same binding: the row the user
    // switched off would silence the row they are still using.
    let shared = HotkeysConfig {
        sell_move_click: MouseGestureBinding::LeftCtrl,
        sell_move_kind: MoveKind::None,
        sell_move_click2: MouseGestureBinding::LeftCtrl,
        sell_move_kind2: MoveKind::AllToOnePrice,
        same_hotkeys_for_move: true,
        ..HotkeysConfig::default()
    };
    assert_eq!(
        shared
            .resolve_move_gesture(|b| b == MouseGestureBinding::LeftCtrl)
            .map(|c| c.kind),
        Some(MoveKind::AllToOnePrice)
    );
}

/// Pins the shipped kind, which is what a fresh install and every old `hotkeys.toml` get.
///
/// Plausible breakage: leaving the derived `None` as the default. Every move gesture would be
/// recognised and do nothing, which is indistinguishable from the gesture being broken.
#[test]
fn the_shipped_move_kind_is_moonbots_parallel_shift() {
    let shipped = HotkeysConfig::default();
    for kind in [
        shipped.buy_move_kind,
        shipped.sell_move_kind,
        shipped.buy_move_kind2,
        shipped.sell_move_kind2,
    ] {
        assert_eq!(kind, MoveKind::ParallelShift);
    }
    // A file written before this field existed loads with the same value rather than an inert one.
    let old: HotkeysConfig = toml::from_str("schema = 2\n").expect("an old file still loads");
    assert_eq!(old.sell_move_kind, MoveKind::ParallelShift);
    assert_eq!(old.buy_move_kind2, MoveKind::ParallelShift);
}

/// The POSITION of a variant in these two lists is a wire value.
///
/// `feed::GestureSettings` carries Moonbot's mouse gestures and move kinds as the raw bytes the
/// safe-share config holds, and TWO surfaces turn a byte into a value by indexing `ALL`: the expert
/// window's Hotkeys page, and `settings::hotkeys::pull_gestures`, which decodes the same bytes into
/// this terminal's own layout. Nothing else pins the lists to the wire, so reordering either — a
/// harmless-looking edit, since both are "just a display order" — would silently rewrite every
/// core's stored gestures on the next OK and mis-import every pulled gesture.
///
/// The anchors are moonproto's own annotated defaults (`shared_config/sections.rs`:
/// `buy_set_click: 1, // Dbl_Click`, `sell_move_click: 2, // CTRL_Click`) and its
/// `ReplaceMultiKind` constants (`commands/trade/enums.rs`, `TReplaceMultiKind` at Vars.pas:37),
/// which run None=0, Shift=1, TopVol=2, LowVol=3, TopProfit=4, All=5, LastSet=6, LastMoved=7.
#[test]
fn wire_ordinals_are_the_positions_in_these_lists() {
    // Every position, not a handful: a reorder in the middle of the list is exactly as damaging
    // as one at its ends, and pinning only the ends would let it through.
    assert_eq!(
        MouseGestureBinding::ALL,
        [
            MouseGestureBinding::None,
            MouseGestureBinding::LeftDouble,
            MouseGestureBinding::LeftCtrl,
            MouseGestureBinding::LeftShift,
            MouseGestureBinding::LeftAlt,
            MouseGestureBinding::Middle,
            MouseGestureBinding::MiddleCtrl,
            MouseGestureBinding::MiddleShift,
            MouseGestureBinding::MiddleAlt,
            MouseGestureBinding::RightDouble,
            MouseGestureBinding::RightCtrl,
            MouseGestureBinding::RightShift,
            MouseGestureBinding::RightAlt,
            MouseGestureBinding::LeftCtrlDouble,
            MouseGestureBinding::LeftShiftDouble,
            MouseGestureBinding::LeftAltDouble,
        ]
    );
    assert_eq!(
        MoveKind::ALL,
        [
            MoveKind::None,
            MoveKind::ParallelShift,
            MoveKind::TopVolume,
            MoveKind::LowVolume,
            MoveKind::TopProfit,
            MoveKind::AllToOnePrice,
            MoveKind::LastSet,
            MoveKind::LastMoved,
        ]
    );
}

/// The figure-delete gesture is a SETTING with a shipped default, not a hardcoded button.
///
/// Plausible breakage: a bare serde default lands `None` on every file written before the field
/// existed, so the gesture the user has been using silently stops working after an update — and
/// nothing in the UI says why, because the row shows exactly what the file said.
#[test]
fn the_shipped_figure_delete_gesture_is_the_middle_click() {
    assert_eq!(
        HotkeysConfig::default().fig_delete_click,
        MouseGestureBinding::Middle
    );
    let old: HotkeysConfig = toml::from_str("schema = 2\n").expect("an old file still loads");
    assert_eq!(old.fig_delete_click, MouseGestureBinding::Middle);

    // And a file that names it keeps what it names, `None` (gesture off) included.
    let off: HotkeysConfig =
        toml::from_str("fig_delete_click = \"none\"\n").expect("an explicit gesture loads");
    assert_eq!(off.fig_delete_click, MouseGestureBinding::None);
    let right: HotkeysConfig =
        toml::from_str("fig_delete_click = \"right-ctrl\"\n").expect("an explicit gesture loads");
    assert_eq!(right.fig_delete_click, MouseGestureBinding::RightCtrl);
}

/// Fields of `HotkeysConfig` that hold a string but not a KEYSTROKE, and so name no slot.
///
/// Written out because the check below cannot tell a gesture's `"middle"` from a keystroke by
/// looking — and because being explicit is the point: a new keystroke field has nowhere to hide.
/// Adding a gesture without adding it here fails the test too, which is the right direction to
/// fail in.
const NOT_KEYSTROKES: [&str; 18] = [
    // Not a binding at all: the tools excluded from the switch cycle.
    "switch_figure_skip",
    "fig_delete_click",
    "buy_set_click",
    "short_set_click",
    "pending_long_click",
    "pending_short_click",
    "buy_move_click",
    "sell_move_click",
    "buy_move_click2",
    "sell_move_click2",
    "short_buy_move_click",
    "short_sell_move_click",
    "short_buy_move_click2",
    "short_sell_move_click2",
    "buy_move_kind",
    "sell_move_kind",
    "buy_move_kind2",
    "sell_move_kind2",
];

/// Every keystroke the file stores is reachable through a [`KeySlot`] — checked against the STRUCT,
/// not against the slot list.
///
/// The distinction is the whole test. Walking `KeySlot::all()` and asking `bound_keys()` what it
/// found proves nothing: `bound_keys()` is now derived from that same list, so a field no slot names
/// is invisible to both, and the first version of this test passed exactly that way. It is checked
/// through serialization instead — every slot is written a marker, and any string left in the file
/// that is not a marker has to be a declared non-keystroke.
///
/// What a missed field costs: `bound_keys()` never sees it, so the migration's collision check and
/// the core pull's conflict gate both read that keystroke as free, and a shipped default takes a key
/// the user is already using — silently, because nothing else looks.
#[test]
fn no_stored_keystroke_is_missing_from_the_registry() {
    const MARKER: &str = "zz-registry-marker";

    let mut cfg = HotkeysConfig::default();
    for slot in KeySlot::all() {
        cfg.set_key(slot, MARKER.to_string());
    }
    let text = toml::to_string(&cfg).expect("serialize hotkeys");
    let table: toml::Table = text.parse().expect("re-read hotkeys");

    let mut unreached = Vec::new();
    let mut nested = Vec::new();
    for (name, value) in &table {
        if NOT_KEYSTROKES.contains(&name.as_str()) {
            continue;
        }
        let strings: Vec<&str> = match value {
            toml::Value::String(s) => vec![s.as_str()],
            toml::Value::Array(items) => items.iter().filter_map(toml::Value::as_str).collect(),
            // The config is FLAT, and this walk only looks one level down. A nested table would
            // carry its keystrokes past the check unseen, so it fails here rather than passing
            // quietly — extend the walk when one arrives.
            toml::Value::Table(_) => {
                nested.push(name.clone());
                continue;
            }
            _ => continue,
        };
        // An EMPTY array counts as unreached too: every slot was just written a marker, so a
        // keystroke field holding none of them is one no slot addresses.
        if strings.is_empty() || strings.iter().any(|held| *held != MARKER) {
            unreached.push(name.clone());
        }
    }

    assert!(
        nested.is_empty(),
        "this walk reads only top-level values; extend it for {nested:?}"
    );
    assert!(
        unreached.is_empty(),
        "these stored keystrokes have no KeySlot, so nothing sees them bound: {unreached:?}"
    );
}

/// No two slots address the same field.
///
/// Every value is written BEFORE any is read back, which is what makes the check work: asserting
/// right after each write lets an alias pass, because the later slot's own read still returns what
/// it just wrote while the earlier slot's value is already gone.
#[test]
fn each_slot_addresses_storage_of_its_own() {
    let mut cfg = HotkeysConfig::default();
    let slots = KeySlot::all();
    // Deliberately NOT `f1`, `f2`...: those are the order-size defaults, so writing one would be a
    // no-op and `set_key` would rightly answer false.
    let value = |n: usize| format!("zz-{n}");
    for (n, slot) in slots.iter().enumerate() {
        assert!(cfg.set_key(*slot, value(n)), "{slot:?} stores nothing");
    }
    for (n, slot) in slots.iter().enumerate() {
        assert_eq!(
            cfg.key(*slot),
            value(n),
            "{slot:?} shares a field with another slot"
        );
    }
}

/// Writing the value a slot already holds is not a change, and an index outside its family is
/// neither a change nor a panic.
///
/// Plausible breakage: bare indexing in the accessors, which is what the settings page's macro did
/// before this. The families are arrays, the page builds their indices in loops, and `hotkeys.toml`
/// is hand-editable.
#[test]
fn a_no_op_write_and_an_impossible_index_are_both_quiet() {
    let mut cfg = HotkeysConfig::default();
    let slot = KeySlot::CancelBuy;
    let held = cfg.key(slot).to_string();

    assert!(!cfg.set_key(slot, held), "rewriting the same value");
    assert!(
        cfg.set_key(slot, "ctrl-q".into()),
        "a real edit reports true"
    );

    for out_of_range in [
        KeySlot::OrderSize(ORDER_SIZE_KEYS),
        KeySlot::SellPreset(999),
        KeySlot::ManualStrategy(MANUAL_STRATEGY_KEYS),
    ] {
        assert_eq!(cfg.key(out_of_range), "");
        assert!(!cfg.set_key(out_of_range, "alt-1".into()));
    }
}

/// The two pending gestures are cleared ONCE, when they stop being inert.
///
/// They shipped saved but unsent, with the settings row saying so; a value set under that promise
/// is not a decision to place live pending orders, which is what the same value does now. Everything
/// else in the file is left exactly as it was.
///
/// Plausible breakage: clearing on every load takes the gesture back from someone who set it
/// deliberately AFTER the change, which is the same mistake the keyboard generations are gated
/// against.
#[test]
fn the_pending_gestures_are_cleared_once_when_they_go_live() {
    let mut old = HotkeysConfig {
        schema: 3,
        pending_long_click: MouseGestureBinding::Middle,
        pending_short_click: MouseGestureBinding::LeftAlt,
        // A neighbour that must survive untouched: this one always did something.
        buy_set_click: MouseGestureBinding::LeftDouble,
        ..HotkeysConfig::default()
    };

    assert!(old.fill_unbound_slots());
    assert_eq!(old.pending_long_click, MouseGestureBinding::None);
    assert_eq!(old.pending_short_click, MouseGestureBinding::None);
    assert_eq!(
        old.buy_set_click,
        MouseGestureBinding::LeftDouble,
        "only the two that changed meaning are touched"
    );

    // Set again on a file that has already been through it, and it stays.
    old.pending_long_click = MouseGestureBinding::MiddleShift;
    assert!(!old.fill_unbound_slots(), "the generation has already run");
    assert_eq!(old.pending_long_click, MouseGestureBinding::MiddleShift);
}
